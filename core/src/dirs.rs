//! Ordneransicht: Unterordner auflisten, anlegen, umbenennen, verschieben – Bibliothek bleibt synchron.

use crate::{is_audio, paths};
use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DirEntry {
    pub name: String,
    pub path: String,
    pub has_children: bool,
    /// Titel der Bibliothek in diesem Ordner inkl. Unterordnern
    pub tracks: usize,
}

fn with_sep(p: &str) -> String {
    if p.ends_with(['/', '\\']) {
        p.to_string()
    } else {
        format!("{p}{MAIN_SEPARATOR}")
    }
}

fn has_subdirs(p: &Path) -> bool {
    std::fs::read_dir(p)
        .map(|r| {
            r.filter_map(|e| e.ok())
                .any(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false) && !hidden(&e.path()))
        })
        .unwrap_or(false)
}

fn hidden(p: &Path) -> bool {
    p.file_name()
        .map(|n| {
            n.to_string_lossy().starts_with('.')
                || n == "$RECYCLE.BIN"
                || n == "System Volume Information"
        })
        .unwrap_or(false)
}

/// Unterordner von `path` mit Titelanzahl (alphabetisch).
/// Unterordner samt „hat selbst Unterordner“ – nur Dateisystem, ohne Datenbank (darf langsam sein, z. B. NAS)
pub fn list_fs(path: &Path) -> Result<Vec<(PathBuf, bool)>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| !hidden(p))
        .collect();
    dirs.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    Ok(dirs.into_iter().map(|d| {
        let sub = has_subdirs(&d);
        (d, sub)
    }).collect())
}

/// Zu `list_fs` die Titelzahlen aus der Datenbank (schnell)
pub fn with_counts(conn: &Connection, path: &Path, dirs: Vec<(PathBuf, bool)>) -> Result<Vec<DirEntry>> {
    let all: Vec<String> = {
        let prefix = with_sep(&path.to_string_lossy());
        let mut st = conn.prepare("SELECT path FROM tracks WHERE substr(path, 1, ?2) = ?1")?;
        st.query_map(params![prefix, prefix.chars().count() as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    Ok(dirs
        .into_iter()
        .map(|(d, has_children)| {
            let p = d.to_string_lossy().to_string();
            let pre = with_sep(&p);
            DirEntry {
                name: d.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                tracks: all.iter().filter(|t| t.starts_with(&pre)).count(),
                has_children,
                path: p,
            }
        })
        .collect())
}

pub fn list(conn: &Connection, path: &Path) -> Result<Vec<DirEntry>> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| !hidden(p))
        .collect();
    dirs.sort_by_key(|p| {
        p.file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    });
    let all: Vec<String> = {
        let prefix = with_sep(&path.to_string_lossy());
        let mut st = conn.prepare("SELECT path FROM tracks WHERE substr(path, 1, ?2) = ?1")?;
        st.query_map(params![prefix, prefix.chars().count() as i64], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    Ok(dirs
        .into_iter()
        .map(|d| {
            let p = d.to_string_lossy().to_string();
            let pre = with_sep(&p);
            DirEntry {
                name: d
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default(),
                tracks: all.iter().filter(|t| t.starts_with(&pre)).count(),
                has_children: has_subdirs(&d),
                path: p,
            }
        })
        .collect())
}

/// Audiodateien direkt in einem Ordner, die (noch) nicht in der Bibliothek sind.
/// Audiodateien direkt in einem Ordner (nur Dateisystem)
pub fn audio_files(path: &Path) -> Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file() || t.is_symlink()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| is_audio(p))
        .collect())
}

pub fn untracked_files(conn: &Connection, path: &Path) -> Result<Vec<PathBuf>> {
    let mut st = conn.prepare("SELECT 1 FROM tracks WHERE path = ?1")?;
    let mut out = Vec::new();
    for e in std::fs::read_dir(path)?.filter_map(|e| e.ok()) {
        let p = e.path();
        if p.is_file() && is_audio(&p) && !st.exists([p.to_string_lossy().as_ref()])? {
            out.push(p);
        }
    }
    Ok(out)
}

pub fn create(parent: &Path, name: &str) -> Result<PathBuf> {
    let p = paths::unique_path(&parent.join(paths::sanitize(name)), &|p| p.exists());
    std::fs::create_dir_all(&p)?;
    Ok(p)
}

/// Pfade in der Datenbank nach Umbenennen/Verschieben eines Ordners anpassen.
pub fn rewrite_prefix(conn: &Connection, old: &Path, new: &Path) -> Result<usize> {
    let o = with_sep(&old.to_string_lossy());
    let n = with_sep(&new.to_string_lossy());
    Ok(conn.execute(
        "UPDATE tracks SET path = ?2 || substr(path, ?3 + 1) WHERE substr(path, 1, ?3) = ?1",
        params![o, n, o.chars().count() as i64],
    )?)
}

pub fn move_dir_raw(conn: &Connection, from: &Path, to: &Path) -> Result<PathBuf> {
    if !from.is_dir() {
        bail!("Ordner nicht gefunden: {}", from.display());
    }
    let to_sep = with_sep(&to.to_string_lossy()).to_lowercase();
    if to_sep.starts_with(&with_sep(&from.to_string_lossy()).to_lowercase()) {
        bail!("Ein Ordner kann nicht in sich selbst verschoben werden");
    }
    let case_only = paths::same_path(from, to);
    if to.exists() && !case_only {
        bail!(
            "Es gibt dort schon einen Ordner „{}“",
            to.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    if case_only {
        let tmp = to.with_file_name(format!(
            "{}.mltmp",
            to.file_name().unwrap_or_default().to_string_lossy()
        ));
        std::fs::rename(from, &tmp)?;
        std::fs::rename(&tmp, to)?;
    } else {
        std::fs::rename(from, to).map_err(|e| anyhow!("Verschieben nicht möglich ({e}). Ordner über Laufwerksgrenzen bitte im Explorer verschieben."))?;
    }
    rewrite_prefix(conn, from, to)?;
    Ok(to.to_path_buf())
}

pub fn rename(conn: &Connection, path: &Path, new_name: &str) -> Result<PathBuf> {
    let name = paths::sanitize(new_name.trim());
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("Musikordner selbst kann hier nicht umbenannt werden"))?;
    move_dir_raw(conn, path, &parent.join(name))
}

pub fn move_into(conn: &Connection, path: &Path, target_parent: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .ok_or_else(|| anyhow!("Ungültiger Ordner"))?;
    if path
        .parent()
        .is_some_and(|p| paths::same_path(p, target_parent))
    {
        return Ok(path.to_path_buf());
    }
    move_dir_raw(conn, path, &target_parent.join(name))
}

/// Ordner samt Inhalt in den Papierkorb; Titel darin verlassen die Bibliothek.
/// Liegt `path` in einem der Musikordner?
pub fn under_any(path: &str, roots: &[PathBuf]) -> bool {
    let norm = |s: &str| s.replace('\\', "/").to_lowercase();
    let p = norm(path);
    roots.iter().any(|r| {
        let r = norm(&r.to_string_lossy());
        let r = r.trim_end_matches('/');
        !r.is_empty() && p.starts_with(&format!("{r}/"))
    })
}

/// Nur Musikordner zählen: alles außerhalb der eingestellten Ordner aus der Bibliothek entfernen
/// (samt Playlist-Einträgen, Analyse und Cues). Gibt die Anzahl entfernter Titel zurück.
pub fn prune_outside(conn: &Connection, roots: &[PathBuf]) -> Result<usize> {
    let rows: Vec<(i64, String)> = {
        let mut st = conn.prepare("SELECT id, path FROM tracks")?;
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let gone: Vec<i64> = rows
        .into_iter()
        .filter(|(_, p)| !under_any(p, roots))
        .map(|(id, _)| id)
        .collect();
    if gone.is_empty() {
        return Ok(0);
    }
    conn.execute_batch("BEGIN")?;
    // gemerkt: kommt der Musikordner zurück, behalten die Titel ID und Playlist-Plätze
    crate::db::forget_tracks(conn, &gone)?;
    conn.execute_batch(
        "DELETE FROM playlist_items WHERE track_id NOT IN (SELECT id FROM tracks); COMMIT",
    )?;
    Ok(gone.len())
}

pub fn remove_tracks_under(conn: &Connection, path: &Path) -> Result<usize> {
    let p = with_sep(&path.to_string_lossy());
    Ok(conn.execute(
        "DELETE FROM tracks WHERE substr(path, 1, ?2) = ?1",
        params![p, p.chars().count() as i64],
    )?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn prune_keeps_only_roots() {
        let c = crate::db::open(
            &std::env::temp_dir().join(format!("mlprune{}.db", std::process::id())),
        )
        .unwrap();
        for p in ["/m/a/1.mp3", "/m/ab/2.mp3", "/x/3.mp3", "C:\\Musik\\4.mp3"] {
            c.execute("INSERT INTO tracks(path, size) VALUES(?1, 1)", [p])
                .unwrap();
        }
        let n = prune_outside(&c, &[PathBuf::from("/m/a"), PathBuf::from("C:\\Musik\\")]).unwrap();
        assert_eq!(n, 2);
        let left: Vec<String> = c
            .prepare("SELECT path FROM tracks ORDER BY path")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .map(|r| r.unwrap())
            .collect();
        assert_eq!(
            left,
            vec!["/m/a/1.mp3".to_string(), "C:\\Musik\\4.mp3".to_string()]
        );
    }

    #[test]
    fn rename_and_move() {
        let base = std::env::temp_dir().join(format!("mldirs{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("A").join("Sub")).unwrap();
        std::fs::create_dir_all(base.join("B")).unwrap();
        std::fs::write(base.join("A").join("Sub").join("x.mp3"), b"x").unwrap();
        let conn = crate::db::open(&base.join("t.db")).unwrap();
        conn.execute(
            "INSERT INTO tracks(path) VALUES(?1)",
            [base.join("A").join("Sub").join("x.mp3").to_string_lossy()],
        )
        .unwrap();
        let l = list(&conn, &base).unwrap();
        assert_eq!(l.iter().find(|d| d.name == "A").unwrap().tracks, 1);
        let new = rename(&conn, &base.join("A"), "Alt").unwrap();
        let moved = move_into(&conn, &new.join("Sub"), &base.join("B")).unwrap();
        let p: String = conn
            .query_row("SELECT path FROM tracks", [], |r| r.get(0))
            .unwrap();
        assert_eq!(Path::new(&p), moved.join("x.mp3"));
        assert!(move_into(&conn, &base.join("B"), &base.join("B/Sub")).is_err());
        let _ = std::fs::remove_dir_all(&base);
    }
}

//! Dateien nach einem Muster in Ordner einsortieren, mit Vorschau und Rückgängig.

use crate::db::Track;
use crate::paths::{self, sanitize};
use crate::{Progress, now};
use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

pub const DEFAULT_PATTERN: &str = "{albumartist}/{album}/{disc-}{track} - {title}";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedMove {
    pub track_id: i64,
    pub from: String,
    pub to: String,
    /// Ziel existiert schon (anderer Titel) → wurde mit (2) usw. umbenannt
    pub renamed: bool,
    pub unchanged: bool,
}

fn field(t: &Track, name: &str) -> String {
    let unknown_artist = "Unbekannter Künstler";
    let v = match name {
        "artist" => t.artist.clone().unwrap_or_else(|| unknown_artist.into()),
        "albumartist" => t
            .album_artist
            .clone()
            .or_else(|| t.artist.clone())
            .unwrap_or_else(|| unknown_artist.into()),
        "album" => t.album.clone().unwrap_or_else(|| "Unbekanntes Album".into()),
        "title" => t.title.clone().unwrap_or_else(|| {
            Path::new(&t.path).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
        }),
        "genre" => t.genre.clone().unwrap_or_else(|| "Unbekanntes Genre".into()),
        "year" => t.year.as_deref().map(|y| y.chars().take(4).collect()).unwrap_or_else(|| "0000".into()),
        "track" => t.track_no.map(|n| format!("{n:02}")).unwrap_or_default(),
        "disc" => t.disc_no.map(|n| n.to_string()).unwrap_or_else(|| "1".into()),
        // nur wenn Disc > 1 vorhanden: "2-"
        "disc-" => match t.disc_no {
            Some(n) if n > 1 => format!("{n}-"),
            _ => String::new(),
        },
        "bpm" => t.bpm.map(|b| format!("{}", b.round() as i64)).unwrap_or_default(),
        "key" => t.key.clone().unwrap_or_default(),
        "initial" => {
            let a = t.album_artist.clone().or_else(|| t.artist.clone()).unwrap_or_else(|| "#".into());
            let a = a.strip_prefix("The ").unwrap_or(&a).to_string();
            a.chars()
                .next()
                .filter(|c| c.is_alphabetic())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "#".into())
        }
        _ => format!("{{{name}}}"),
    };
    v.replace(['/', '\\'], "_")
}

/// Muster anwenden: "{albumartist}/{album}/{track} - {title}" → relativer Pfad ohne Endung.
pub fn render(pattern: &str, t: &Track) -> PathBuf {
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(a) = rest.find('{') {
        out.push_str(&rest[..a]);
        match rest[a..].find('}') {
            Some(b) => {
                out.push_str(&field(t, &rest[a + 1..a + b]));
                rest = &rest[a + b + 1..];
            }
            None => {
                out.push_str(&rest[a..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    let mut p = PathBuf::new();
    for seg in out.split(['/', '\\']) {
        if !seg.trim().is_empty() {
            let seg = seg.trim().trim_start_matches(['-', ' ']).trim();
            if !seg.is_empty() {
                p.push(sanitize(seg));
            }
        }
    }
    p
}

pub fn plan(tracks: &[Track], target_root: &Path, pattern: &str) -> Vec<PlannedMove> {
    let mut taken: HashSet<String> = HashSet::new();
    let key = |p: &Path| p.to_string_lossy().to_lowercase();
    let sources: HashSet<String> = tracks.iter().map(|t| t.path.to_lowercase()).collect();
    let mut out = Vec::new();
    for t in tracks {
        let src = Path::new(&t.path);
        let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let rel = render(pattern, t);
        let mut dst = target_root.join(rel);
        let fname = format!("{}.{}", dst.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default(), ext);
        dst.set_file_name(fname);
        if paths::same_path(src, &dst) {
            taken.insert(key(&dst));
            out.push(PlannedMove { track_id: t.id, from: t.path.clone(), to: t.path.clone(), renamed: false, unchanged: true });
            continue;
        }
        let is_taken = |p: &Path| {
            let k = key(p);
            taken.contains(&k) || (p.exists() && !sources.contains(&k))
        };
        let final_dst = paths::unique_path(&dst, &is_taken);
        let renamed = final_dst != dst;
        taken.insert(key(&final_dst));
        out.push(PlannedMove {
            track_id: t.id,
            from: t.path.clone(),
            to: final_dst.to_string_lossy().to_string(),
            renamed,
            unchanged: false,
        });
    }
    out
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub batch_id: i64,
    pub moved: usize,
    pub errors: Vec<String>,
}

/// Führt den Plan aus. `copy = true` kopiert statt zu verschieben (Bibliothek zeigt dann auf die Kopie).
pub fn apply(
    conn: &std::sync::Mutex<Connection>,
    plan: &[PlannedMove],
    copy: bool,
    description: &str,
    stop_dirs: &[PathBuf],
    progress: Progress,
) -> Result<ApplyReport> {
    let batch_id = {
        let c = conn.lock().unwrap();
        c.execute(
            "INSERT INTO move_batches(created, description) VALUES(?1, ?2)",
            params![now(), description],
        )?;
        c.last_insert_rowid()
    };
    let mut report = ApplyReport { batch_id, moved: 0, errors: vec![] };
    let todo: Vec<&PlannedMove> = plan.iter().filter(|m| !m.unchanged).collect();
    // Zwei-Phasen bei Kollisionen (A→B und B→A): erst auf temporäre Namen, wenn Ziel noch belegt ist
    for (i, m) in todo.iter().enumerate() {
        progress(i, todo.len(), &m.to);
        let (from, to) = (Path::new(&m.from), Path::new(&m.to));
        if !from.exists() {
            report.errors.push(format!("Fehlt: {}", m.from));
            continue;
        }
        if to.exists() && !paths::same_path(from, to) {
            report.errors.push(format!("Ziel belegt: {}", m.to));
            continue;
        }
        let res = if copy {
            to.parent().map(std::fs::create_dir_all).transpose().and_then(|_| std::fs::copy(from, to).map(|_| ()))
        } else if paths::same_path(from, to) {
            // nur Groß-/Kleinschreibung → über Zwischennamen
            let tmp = to.with_extension("mltmp");
            std::fs::rename(from, &tmp).and_then(|_| std::fs::rename(&tmp, to))
        } else {
            paths::move_file(from, to)
        };
        match res {
            Ok(()) => {
                let c = conn.lock().unwrap();
                c.execute(
                    "INSERT INTO moves(batch_id, track_id, src, dst, copied) VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![batch_id, m.track_id, m.from, m.to, copy as i64],
                )?;
                c.execute("UPDATE tracks SET path = ?2 WHERE id = ?1", params![m.track_id, m.to])?;
                report.moved += 1;
                if !copy {
                    if let Some(d) = from.parent() {
                        paths::remove_empty_dirs(d.to_path_buf(), stop_dirs);
                    }
                }
            }
            Err(e) => report.errors.push(format!("{}: {e}", m.from)),
        }
    }
    {
        let c = conn.lock().unwrap();
        c.execute("UPDATE move_batches SET count = ?2 WHERE id = ?1", params![batch_id, report.moved as i64])?;
        if report.moved == 0 {
            c.execute("DELETE FROM move_batches WHERE id = ?1", [batch_id])?;
        }
    }
    progress(todo.len(), todo.len(), "Fertig");
    Ok(report)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub id: i64,
    pub created: i64,
    pub description: String,
    pub count: i64,
    pub undone: bool,
}

pub fn batches(conn: &Connection) -> Result<Vec<Batch>> {
    let mut st = conn.prepare("SELECT id, created, description, count, undone FROM move_batches ORDER BY id DESC LIMIT 30")?;
    let rows = st.query_map([], |r| {
        Ok(Batch { id: r.get(0)?, created: r.get(1)?, description: r.get(2)?, count: r.get(3)?, undone: r.get::<_, i64>(4)? != 0 })
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// Macht einen Vorgang rückgängig (in umgekehrter Reihenfolge).
pub fn undo(conn: &std::sync::Mutex<Connection>, batch_id: i64, stop_dirs: &[PathBuf]) -> Result<ApplyReport> {
    let moves: Vec<(Option<i64>, String, String, bool)> = {
        let c = conn.lock().unwrap();
        let undone: i64 = c.query_row("SELECT undone FROM move_batches WHERE id = ?1", [batch_id], |r| r.get(0))?;
        if undone != 0 {
            return Err(anyhow!("Schon rückgängig gemacht"));
        }
        let mut st = c.prepare("SELECT track_id, src, dst, copied FROM moves WHERE batch_id = ?1 ORDER BY rowid DESC")?;
        st.query_map([batch_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get::<_, i64>(3)? != 0)))?
            .collect::<rusqlite::Result<_>>()?
    };
    let mut report = ApplyReport { batch_id, moved: 0, errors: vec![] };
    for (tid, src, dst, copied) in moves {
        let (s, d) = (Path::new(&src), Path::new(&dst));
        let res = if copied {
            std::fs::remove_file(d)
        } else if s.exists() && !paths::same_path(s, d) {
            Err(std::io::Error::other("Ursprungsort ist wieder belegt"))
        } else {
            paths::move_file(d, s)
        };
        match res {
            Ok(()) => {
                if let Some(id) = tid {
                    let c = conn.lock().unwrap();
                    c.execute("UPDATE tracks SET path = ?2 WHERE id = ?1", params![id, src])?;
                }
                if let Some(p) = d.parent() {
                    paths::remove_empty_dirs(p.to_path_buf(), stop_dirs);
                }
                report.moved += 1;
            }
            Err(e) => report.errors.push(format!("{dst}: {e}")),
        }
    }
    conn.lock().unwrap().execute("UPDATE move_batches SET undone = 1 WHERE id = ?1", [batch_id])?;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn render_pattern() {
        let t = Track {
            path: "/x/a.mp3".into(),
            artist: Some("AC/DC".into()),
            album: Some("Back in Black".into()),
            title: Some("Hells Bells".into()),
            track_no: Some(1),
            ..Default::default()
        };
        assert_eq!(render(DEFAULT_PATTERN, &t), PathBuf::from("AC_DC/Back in Black/01 - Hells Bells"));
        assert_eq!(render("{initial}/{artist}", &t), PathBuf::from("A/AC_DC"));
    }
}

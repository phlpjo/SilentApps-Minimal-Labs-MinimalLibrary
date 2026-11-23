//! Verlauf + Rückgängig für alle Änderungen: Verschieben/Umbenennen, Tags, Cover, Papierkorb, Ordner.

use crate::db::{self, Track};
use crate::{dirs, now, organize, paths, scanner, tags};
use anyhow::{Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Ältere Einträge werden beim Anlegen neuer automatisch gelöscht.
const KEEP: i64 = 300;

pub fn begin(conn: &Connection, kind: &str, description: &str) -> Result<i64> {
    conn.execute(
        "INSERT INTO move_batches(created, description, kind) VALUES(?1, ?2, ?3)",
        params![now(), description, kind],
    )?;
    let id = conn.last_insert_rowid();
    conn.execute("DELETE FROM move_batches WHERE id <= ?1", [id - KEEP])?;
    Ok(id)
}

pub fn finish(conn: &Connection, batch: i64, count: usize) -> Result<()> {
    if count == 0 {
        conn.execute("DELETE FROM move_batches WHERE id = ?1", [batch])?;
    } else {
        conn.execute(
            "UPDATE move_batches SET count = ?2 WHERE id = ?1",
            params![batch, count as i64],
        )?;
    }
    Ok(())
}

/// Playlist-Änderung beginnen: sichert die betroffenen Playlisten (nach der Änderung `finish(…, 1)` aufrufen).
/// Neu angelegte Playlisten danach mit `playlists::backup` vermerken (sie existierten vorher nicht).
pub fn begin_playlist(conn: &Connection, description: &str, ids: &[i64]) -> Result<i64> {
    let b = begin(conn, "playlist", description)?;
    for id in ids {
        crate::playlists::backup(conn, b, *id)?;
    }
    Ok(b)
}

/// Reihenfolge der Playlisten ändern – mit Rückgängig
pub fn playlist_order(conn: &Connection, ids: &[i64]) -> Result<()> {
    let b = begin(conn, "playlist", "Playlisten umsortiert")?;
    crate::playlists::backup_order(conn, b)?;
    crate::playlists::set_order(conn, ids)?;
    finish(conn, b, 1)
}

/// Stand der Tags vor einer Änderung
#[derive(Debug, Serialize, Deserialize)]
struct Snapshot {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    genre: Option<String>,
    year: Option<String>,
    track_no: Option<u32>,
    disc_no: Option<u32>,
    comment: Option<String>,
    bpm: Option<f64>,
    key: Option<String>,
}

/// Vor dem Schreiben aufrufen: sichert Tags (und optional das Cover) der Datei.
pub fn backup_tags(conn: &Connection, batch: i64, t: &Track, with_cover: bool) -> Result<()> {
    let p = Path::new(&t.path);
    let d = tags::read(p)?;
    let snap = Snapshot {
        title: d.title,
        artist: d.artist,
        album: d.album,
        album_artist: d.album_artist,
        genre: d.genre,
        year: d.year,
        track_no: d.track_no,
        disc_no: d.disc_no,
        comment: d.comment,
        bpm: d.bpm,
        key: d.key,
    };
    let cover = if with_cover {
        tags::cover(p).ok().flatten().map(|(_, b)| b)
    } else {
        None
    };
    conn.execute(
        "INSERT INTO tag_backups(batch_id, track_id, tags, cover_changed, cover) VALUES(?1, ?2, ?3, ?4, ?5)",
        params![batch, t.id, serde_json::to_string(&snap)?, with_cover as i64, cover],
    )?;
    Ok(())
}

/// Vor dem Löschen: Pfad und Playlist-Plätze merken.
pub fn backup_trash(
    conn: &Connection,
    batch: i64,
    path: &Path,
    is_dir: bool,
    track_ids: &[i64],
) -> Result<()> {
    let mut pls: Vec<(i64, i64, String)> = Vec::new(); // (playlist, pos, pfad)
    let mut st = conn.prepare("SELECT playlist_id, pos FROM playlist_items WHERE track_id = ?1")?;
    for id in track_ids {
        let tp: Option<String> = conn
            .query_row("SELECT path FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .optional()?;
        let Some(tp) = tp else { continue };
        for row in st.query_map([id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))? {
            let (pl, pos) = row?;
            pls.push((pl, pos, tp.clone()));
        }
    }
    conn.execute(
        "INSERT INTO trash_backups(batch_id, path, is_dir, playlists) VALUES(?1, ?2, ?3, ?4)",
        params![
            batch,
            path.to_string_lossy(),
            is_dir as i64,
            serde_json::to_string(&pls)?
        ],
    )?;
    Ok(())
}

/// Ordner umbenannt/verschoben: als Verschiebung (ohne Titel-ID) protokollieren.
pub fn log_dir_move(conn: &Connection, from: &Path, to: &Path, description: &str) -> Result<i64> {
    let b = begin(conn, "dir", description)?;
    conn.execute(
        "INSERT INTO moves(batch_id, track_id, src, dst, copied) VALUES(?1, NULL, ?2, ?3, 0)",
        params![b, from.to_string_lossy(), to.to_string_lossy()],
    )?;
    finish(conn, b, 1)?;
    Ok(b)
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UndoReport {
    pub restored: usize,
    pub errors: Vec<String>,
    /// Wieder aufgenommene Titel (müssen neu analysiert werden)
    pub new_ids: Vec<i64>,
}

pub fn undo(conn: &Mutex<Connection>, batch: i64, stop_dirs: &[PathBuf]) -> Result<UndoReport> {
    let (kind, undone): (String, i64) = conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT kind, undone FROM move_batches WHERE id = ?1",
            [batch],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| anyhow!("Eintrag nicht gefunden"))?;
    if undone != 0 {
        return Err(anyhow!("Schon rückgängig gemacht"));
    }
    let report = match kind.as_str() {
        "move" => {
            let r = organize::undo(conn, batch, stop_dirs)?;
            return Ok(UndoReport {
                restored: r.moved,
                errors: r.errors,
                new_ids: vec![],
            });
        }
        "tags" => undo_tags(conn, batch)?,
        "trash" => undo_trash(conn, batch)?,
        "dir" => undo_dir(conn, batch)?,
        "playlist" => undo_playlist(conn, batch)?,
        "grid" => UndoReport {
            restored: crate::pioneer::undo(&conn.lock().unwrap(), batch)?,
            ..Default::default()
        },
        k => return Err(anyhow!("Unbekannte Art: {k}")),
    };
    conn.lock()
        .unwrap()
        .execute("UPDATE move_batches SET undone = 1 WHERE id = ?1", [batch])?;
    Ok(report)
}

fn undo_playlist(conn: &Mutex<Connection>, batch: i64) -> Result<UndoReport> {
    let mut c = conn.lock().unwrap();
    type Row = (i64, i64, String, Option<String>, String, i64);
    let rows: Vec<Row> = {
        let mut st = c.prepare("SELECT playlist_id, existed, name, rules, items, pos FROM playlist_backups WHERE batch_id = ?1 ORDER BY rowid DESC")?;
        st.query_map([batch], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut n = 0;
    for (id, existed, name, rules, items, pos) in rows {
        let items: Vec<i64> = serde_json::from_str(&items).unwrap_or_default();
        if id == 0 {
            crate::playlists::set_order(&c, &items)?;
        } else if existed == 0 {
            c.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
        } else {
            let exists: bool =
                c.query_row("SELECT COUNT(*) FROM playlists WHERE id = ?1", [id], |r| {
                    r.get::<_, i64>(0)
                })? > 0;
            if exists {
                c.execute(
                    "UPDATE playlists SET name = ?2, rules = ?3, pos = ?4 WHERE id = ?1",
                    params![id, name, rules, pos],
                )?;
            } else {
                c.execute(
                    "INSERT INTO playlists(id, name, rules, created, pos) VALUES(?1, ?2, ?3, ?4, ?5)",
                    params![id, name, rules, now(), pos],
                )?;
            }
            if rules.is_none() {
                // nur Titel, die es noch gibt
                let keep: Vec<i64> = items
                    .into_iter()
                    .filter(|t| {
                        c.query_row("SELECT COUNT(*) FROM tracks WHERE id = ?1", [t], |r| {
                            r.get::<_, i64>(0)
                        })
                        .unwrap_or(0)
                            > 0
                    })
                    .collect();
                crate::playlists::set_items(&mut c, id, &keep)?;
            }
        }
        n += 1;
    }
    Ok(UndoReport {
        restored: n,
        ..Default::default()
    })
}

/// Was ein Verlaufseintrag betroffen hat (zum Aufklappen in der Liste)
pub fn items(conn: &Connection, batch: i64) -> Result<Vec<String>> {
    let kind: String = conn.query_row(
        "SELECT kind FROM move_batches WHERE id = ?1",
        [batch],
        |r| r.get(0),
    )?;
    let q = |sql: &str| -> Result<Vec<String>> {
        let mut st = conn.prepare(sql)?;
        let v = st
            .query_map([batch], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    };
    Ok(match kind.as_str() {
        "move" | "dir" => q(
            "SELECT src || '  →  ' || dst FROM moves WHERE batch_id = ?1 ORDER BY rowid LIMIT 1000",
        )?,
        "trash" => {
            q("SELECT path FROM trash_backups WHERE batch_id = ?1 ORDER BY rowid LIMIT 1000")?
        }
        "tags" => q(
            "SELECT COALESCE(t.path, '#' || b.track_id) FROM tag_backups b LEFT JOIN tracks t ON t.id = b.track_id WHERE b.batch_id = ?1 ORDER BY b.rowid LIMIT 1000",
        )?,
        "grid" => q(
            "SELECT COALESCE(t.path, '#' || b.track_id) FROM grid_backups b LEFT JOIN tracks t ON t.id = b.track_id WHERE b.batch_id = ?1 LIMIT 1000",
        )?,
        "playlist" => {
            let mut st = conn.prepare("SELECT playlist_id, existed, name, items FROM playlist_backups WHERE batch_id = ?1 ORDER BY rowid")?;
            let rows: Vec<(i64, i64, String, String)> = st
                .query_map([batch], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
                })?
                .collect::<rusqlite::Result<_>>()?;
            let mut out = Vec::new();
            for (id, existed, name, items) in rows {
                let ids: Vec<i64> = serde_json::from_str(&items).unwrap_or_default();
                if id == 0 {
                    out.push(format!("Reihenfolge vorher: {} Playlisten", ids.len()));
                    continue;
                }
                if existed == 0 {
                    out.push(format!(
                        "„{}“ neu angelegt",
                        crate::playlists::name(conn, id).unwrap_or_default()
                    ));
                    continue;
                }
                out.push(format!("„{name}“ vorher: {} Titel", ids.len()));
                let mut st = conn.prepare("SELECT path FROM tracks WHERE id = ?1")?;
                for t in ids.iter().take(200) {
                    if let Ok(p) = st.query_row([t], |r| r.get::<_, String>(0)) {
                        out.push(format!("   {p}"));
                    }
                }
            }
            out
        }
        _ => vec![],
    })
}

fn undo_tags(conn: &Mutex<Connection>, batch: i64) -> Result<UndoReport> {
    let rows: Vec<(i64, String, bool, Option<Vec<u8>>)> = {
        let c = conn.lock().unwrap();
        let mut st = c.prepare("SELECT track_id, tags, cover_changed, cover FROM tag_backups WHERE batch_id = ?1 ORDER BY rowid DESC")?;
        st.query_map([batch], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let mut rep = UndoReport::default();
    for (id, json, cover_changed, cover) in rows {
        let Some(t) = db::track(&conn.lock().unwrap(), id)? else {
            rep.errors
                .push(format!("Titel {id} ist nicht mehr in der Bibliothek"));
            continue;
        };
        let p = Path::new(&t.path);
        let s: Snapshot = serde_json::from_str(&json)?;
        let f = |v: Option<String>| Some(v.unwrap_or_default());
        let u = tags::TagUpdate {
            title: f(s.title),
            artist: f(s.artist),
            album: f(s.album),
            album_artist: f(s.album_artist),
            genre: f(s.genre),
            year: f(s.year),
            track_no: f(s.track_no.map(|n| n.to_string())),
            disc_no: f(s.disc_no.map(|n| n.to_string())),
            comment: f(s.comment),
            bpm: f(s.bpm.map(|b| b.to_string())),
            key: f(s.key),
        };
        let res = tags::write(p, &u).and_then(|_| {
            if cover_changed {
                match cover {
                    Some(c) => tags::set_cover(p, c),
                    None => tags::remove_cover(p),
                }
            } else {
                Ok(())
            }
        });
        match res {
            Ok(()) => {
                scanner::refresh(&conn.lock().unwrap(), p)?;
                rep.restored += 1;
            }
            Err(e) => rep.errors.push(format!("{e:#}")),
        }
    }
    Ok(rep)
}

fn undo_dir(conn: &Mutex<Connection>, batch: i64) -> Result<UndoReport> {
    let c = conn.lock().unwrap();
    let (src, dst): (String, String) = c.query_row(
        "SELECT src, dst FROM moves WHERE batch_id = ?1",
        [batch],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    dirs::move_dir_raw(&c, Path::new(&dst), Path::new(&src))?;
    Ok(UndoReport {
        restored: 1,
        ..Default::default()
    })
}

#[cfg(any(
    windows,
    all(
        unix,
        not(target_os = "macos"),
        not(target_os = "ios"),
        not(target_os = "android")
    )
))]
fn restore_from_trash(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    use trash::os_limited;
    let items = os_limited::list()?;
    let mut pick = Vec::new();
    let mut found = Vec::new();
    for p in paths {
        let (parent, name) = (
            p.parent().unwrap_or(Path::new("")),
            p.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
        );
        // jüngsten passenden Eintrag nehmen
        if let Some(it) = items
            .iter()
            .filter(|i| {
                i.name.to_string_lossy() == name && paths::same_path(&i.original_parent, parent)
            })
            .max_by_key(|i| i.time_deleted)
        {
            pick.push(it.clone());
            found.push(p.clone());
        }
    }
    if pick.is_empty() {
        return Err(anyhow!("Nicht mehr im Papierkorb gefunden"));
    }
    os_limited::restore_all(pick).map_err(|e| anyhow!("Wiederherstellen fehlgeschlagen: {e}"))?;
    Ok(found)
}

/// macOS: über den Finder zurücklegen (der Papierkorb-Ordner selbst ist geschützt)
#[cfg(target_os = "macos")]
fn restore_from_trash(paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut found = Vec::new();
    let mut last_err = String::new();
    for p in paths {
        let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else {
            continue;
        };
        let script = format!(
            "tell application \"Finder\"\nset f to (first item of trash whose name is \"{}\")\nmove f to (POSIX file \"{}\" as alias)\nend tell",
            esc(&name.to_string_lossy()),
            esc(&parent.to_string_lossy())
        );
        match std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
        {
            Ok(o) if o.status.success() => found.push(p.clone()),
            Ok(o) => last_err = String::from_utf8_lossy(&o.stderr).trim().to_string(),
            Err(e) => last_err = e.to_string(),
        }
    }
    if found.is_empty() {
        return Err(anyhow!("Nicht mehr im Papierkorb gefunden {last_err}"));
    }
    Ok(found)
}

#[cfg(not(any(
    windows,
    target_os = "macos",
    all(unix, not(target_os = "ios"), not(target_os = "android"))
)))]
fn restore_from_trash(_paths: &[PathBuf]) -> Result<Vec<PathBuf>> {
    Err(anyhow!(
        "Wiederherstellen aus dem Papierkorb wird hier nicht unterstützt"
    ))
}

fn undo_trash(conn: &Mutex<Connection>, batch: i64) -> Result<UndoReport> {
    let rows: Vec<(String, bool, String)> = {
        let c = conn.lock().unwrap();
        let mut st =
            c.prepare("SELECT path, is_dir, playlists FROM trash_backups WHERE batch_id = ?1")?;
        st.query_map([batch], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?
    };
    let paths: Vec<PathBuf> = rows.iter().map(|(p, _, _)| PathBuf::from(p)).collect();
    let restored = restore_from_trash(&paths)?;
    let mut rep = UndoReport {
        restored: restored.len(),
        ..Default::default()
    };
    // Dateien wieder aufnehmen (Ordner: alles darin)
    let ids = scanner::add_files(conn, &restored)?;
    rep.new_ids = ids;
    // Playlist-Plätze wiederherstellen
    let c = conn.lock().unwrap();
    let mut items: Vec<(i64, i64, String)> = Vec::new();
    for (_, _, pl) in &rows {
        items.extend(serde_json::from_str::<Vec<(i64, i64, String)>>(pl).unwrap_or_default());
    }
    items.sort_by_key(|(pl, pos, _)| (*pl, *pos));
    for (pl, pos, path) in items {
        let Some(tid) = c
            .query_row("SELECT id FROM tracks WHERE path = ?1", [&path], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
        else {
            continue;
        };
        let exists: bool = c
            .query_row("SELECT 1 FROM playlists WHERE id = ?1", [pl], |_| Ok(true))
            .optional()?
            .unwrap_or(false);
        if !exists {
            continue;
        }
        c.execute(
            "UPDATE playlist_items SET pos = pos + 1 WHERE playlist_id = ?1 AND pos >= ?2",
            params![pl, pos],
        )?;
        c.execute(
            "INSERT INTO playlist_items(playlist_id, pos, track_id) VALUES(?1, ?2, ?3)",
            params![pl, pos, tid],
        )?;
    }
    Ok(rep)
}

/// Letzten nicht rückgängig gemachten Eintrag finden (für Strg+Z)
pub fn last_open(conn: &Connection) -> Result<Option<i64>> {
    Ok(conn
        .query_row(
            "SELECT id FROM move_batches WHERE undone = 0 ORDER BY id DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_roundtrip() {
        let base = std::env::temp_dir().join(format!("mlhist{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine.mp3");
        if !src.exists() {
            return; // Testdatei fehlt (z. B. CI ohne ffmpeg)
        }
        let f = base.join("a.mp3");
        std::fs::copy(&src, &f).unwrap();
        let conn = Mutex::new(db::open(&base.join("t.db")).unwrap());
        let id = scanner::upsert(&conn.lock().unwrap(), &f, &tags::read(&f).unwrap()).unwrap();
        let t = db::track(&conn.lock().unwrap(), id).unwrap().unwrap();
        let b = begin(&conn.lock().unwrap(), "tags", "Test").unwrap();
        backup_tags(&conn.lock().unwrap(), b, &t, false).unwrap();
        tags::write(
            &f,
            &tags::TagUpdate {
                title: Some("Neu".into()),
                genre: Some("X".into()),
                ..Default::default()
            },
        )
        .unwrap();
        finish(&conn.lock().unwrap(), b, 1).unwrap();
        assert_eq!(tags::read(&f).unwrap().title.as_deref(), Some("Neu"));
        undo(&conn, b, &[]).unwrap();
        let d = tags::read(&f).unwrap();
        assert_eq!(d.title.as_deref(), Some("Original"));
        assert_eq!(d.genre, None);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn playlist_undo() {
        let f = std::env::temp_dir().join(format!("mlplh{}.db", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let conn = Mutex::new(db::open(&f).unwrap());
        let (pl, a, b) = {
            let mut c = conn.lock().unwrap();
            c.execute(
                "INSERT INTO tracks(path, size, mtime, added) VALUES('/a.mp3', 1, 1, 0)",
                [],
            )
            .unwrap();
            let a = c.last_insert_rowid();
            c.execute(
                "INSERT INTO tracks(path, size, mtime, added) VALUES('/b.mp3', 1, 1, 0)",
                [],
            )
            .unwrap();
            let b = c.last_insert_rowid();
            let pl = crate::playlists::create(&c, "P", None).unwrap();
            crate::playlists::set_items(&mut c, pl, &[a, b]).unwrap();
            (pl, a, b)
        };
        // Titel entfernt → rückgängig
        let batch = {
            let mut c = conn.lock().unwrap();
            let x = begin_playlist(&c, "t", &[pl]).unwrap();
            crate::playlists::set_items(&mut c, pl, &[b]).unwrap();
            finish(&c, x, 1).unwrap();
            x
        };
        undo(&conn, batch, &[]).unwrap();
        assert_eq!(
            crate::playlists::track_ids(&conn.lock().unwrap(), pl).unwrap(),
            vec![a, b]
        );
        // gelöscht → rückgängig
        let batch = {
            let c = conn.lock().unwrap();
            let x = begin_playlist(&c, "d", &[pl]).unwrap();
            crate::playlists::delete(&c, pl).unwrap();
            finish(&c, x, 1).unwrap();
            x
        };
        undo(&conn, batch, &[]).unwrap();
        let c = conn.lock().unwrap();
        assert_eq!(crate::playlists::name(&c, pl).unwrap(), "P");
        assert_eq!(crate::playlists::track_ids(&c, pl).unwrap(), vec![a, b]);
        assert!(!items(&c, batch).unwrap().is_empty());
    }
}

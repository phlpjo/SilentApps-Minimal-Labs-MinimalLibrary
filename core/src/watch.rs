//! Musikordner überwachen: neue, geänderte, verschobene und gelöschte Dateien sofort übernehmen.

use crate::{dirs, is_audio, scanner};
use anyhow::Result;
use notify_debouncer_full::notify::event::{ModifyKind, RenameMode};
use notify_debouncer_full::notify::{EventKind, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{DebounceEventResult, Debouncer, RecommendedCache, new_debouncer};
use rusqlite::{Connection, OptionalExtension, params};
use std::collections::BTreeSet;
use std::path::{MAIN_SEPARATOR, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Watcher = Debouncer<RecommendedWatcher, RecommendedCache>;

/// Überwachte Musikordner (für die Prüfung „Laufwerk weg ≠ Dateien gelöscht“)
static ROOTS: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

#[derive(Debug, Default, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Change {
    /// Neu aufgenommene Titel (zum Analysieren)
    pub added: Vec<i64>,
    pub updated: usize,
    pub moved: usize,
    pub removed: usize,
}

impl Change {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.updated == 0 && self.moved == 0 && self.removed == 0
    }
}

fn ignored(p: &Path) -> bool {
    let s = p.to_string_lossy();
    s.ends_with(".mltmp")
        || p.components().any(|c| {
            let n = c.as_os_str().to_string_lossy();
            n.starts_with('.') || n == "$RECYCLE.BIN"
        })
}

fn track_id(c: &Connection, p: &Path) -> Option<i64> {
    c.query_row(
        &format!("SELECT id FROM tracks WHERE {}", crate::paths::PATH_EQ),
        [p.to_string_lossy()],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

fn stat(p: &Path) -> Option<(i64, i64)> {
    let m = std::fs::metadata(p).ok()?;
    let mt = m
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_secs() as i64;
    Some((m.len() as i64, mt))
}

/// Neue Dateien aufnehmen. Gibt es in der Bibliothek einen Titel mit gleicher Größe + Änderungszeit,
/// dessen Datei verschwunden ist, wurde er verschoben: Pfad übernehmen (ID, Analyse, Playlisten bleiben).
fn adopt_or_add(conn: &Mutex<Connection>, files: Vec<PathBuf>, ch: &mut Change) {
    let mut new = Vec::new();
    {
        let c = conn.lock().unwrap();
        for f in files {
            let Some((size, mtime)) = stat(&f) else {
                continue;
            };
            let lost: Option<i64> = c
                .prepare("SELECT id, path FROM tracks WHERE size = ?1 AND mtime = ?2")
                .and_then(|mut st| {
                    let rows: Vec<(i64, String)> = st
                        .query_map(params![size, mtime], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .filter_map(|r| r.ok())
                        .collect();
                    let mut cache = std::collections::HashMap::new();
                    Ok(rows
                        .into_iter()
                        .find(|(_, p)| scanner::really_gone(Path::new(p), &mut cache))
                        .map(|(id, _)| id))
                })
                .ok()
                .flatten();
            match lost {
                Some(id) => {
                    if c.execute(
                        "UPDATE tracks SET path = ?2 WHERE id = ?1",
                        params![id, f.to_string_lossy()],
                    )
                    .is_ok()
                    {
                        ch.moved += 1;
                    }
                }
                None => match crate::db::revive_lost(&c, &f, size, mtime) {
                    // früher verschwundener Titel (z. B. über Laufwerke verschoben): alte ID + Playlisten zurück
                    Ok(Some(_)) => ch.moved += 1,
                    _ => new.push(f),
                },
            }
        }
    }
    if !new.is_empty() {
        if let Ok(ids) = scanner::add_files(conn, &new) {
            ch.added.extend(ids);
        }
    }
}

/// Datei oder Ordner auf den Stand der Platte bringen.
fn sync_path(conn: &Mutex<Connection>, p: &Path, ch: &mut Change) {
    if ignored(p) {
        return;
    }
    if p.is_dir() {
        // neuer/hineinkopierter Ordner: alle unbekannten Audiodateien aufnehmen
        // Erst ohne Datenbank-Sperre auflisten (Cloud-Ordner können langsam sein), dann abgleichen
        let found: Vec<PathBuf> = walkdir::WalkDir::new(p)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file() && is_audio(e.path()) && !ignored(e.path()))
            .map(|e| e.into_path())
            .collect();
        let new: Vec<PathBuf> = {
            let c = conn.lock().unwrap();
            found
                .into_iter()
                .filter(|f| track_id(&c, f).is_none())
                .collect()
        };
        adopt_or_add(conn, new, ch);
        return;
    }
    if p.exists() {
        if !is_audio(p) {
            return;
        }
        let c = conn.lock().unwrap();
        match track_id(&c, p) {
            None => {
                drop(c);
                adopt_or_add(conn, vec![p.to_path_buf()], ch);
            }
            Some(id) => {
                let known: Option<(i64, i64)> = c
                    .query_row("SELECT size, mtime FROM tracks WHERE id = ?1", [id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .ok();
                drop(c);
                // Tags ohne Datenbank-Sperre lesen – bei Cloud-Dateien kann das dauern (Download)
                if stat(p) != known {
                    if let Ok(t) = crate::tags::read(p) {
                        if scanner::upsert(&conn.lock().unwrap(), p, &t).is_ok() {
                            ch.updated += 1;
                        }
                    }
                }
            }
        }
    } else {
        // gelöscht: Datei oder ganzer Ordner – aber nur, wenn wirklich weg (nicht bloß Netzlaufwerk/NAS kurz weg)
        // Musikordner selbst nicht erreichbar (NAS/Laufwerk getrennt) → nichts entfernen
        let roots = ROOTS.lock().unwrap().clone();
        if roots.iter().any(|r| p.starts_with(r) && !r.is_dir()) || roots.iter().any(|r| r.starts_with(p)) {
            return;
        }
        let mut cache = std::collections::HashMap::new();
        if !scanner::really_gone(p, &mut cache) {
            return;
        }
        std::thread::sleep(Duration::from_millis(300));
        cache.clear();
        if !scanner::really_gone(p, &mut cache) {
            return;
        }
        let c = conn.lock().unwrap();
        let s = p.to_string_lossy().to_string();
        let pre = format!("{s}{MAIN_SEPARATOR}");
        let ids: Vec<i64> = c
            .prepare("SELECT id FROM tracks WHERE path = ?1 OR substr(path, 1, ?3) = ?2")
            .and_then(|mut st| {
                st.query_map(params![s, pre, pre.chars().count() as i64], |r| r.get(0))?
                    .collect::<rusqlite::Result<Vec<i64>>>()
            })
            .unwrap_or_default();
        // gemerkt statt endgültig gelöscht: Verschieben über Laufwerke/Musikordner = Löschen + Neu
        if let Ok(n) = crate::db::forget_tracks(&c, &ids) {
            ch.removed += n;
        }
    }
}

/// Umbenennen/Verschieben (Explorer): Titel-IDs, Analyse und Playlist-Plätze bleiben erhalten.
fn rename(conn: &Mutex<Connection>, from: &Path, to: &Path, ch: &mut Change) {
    if ignored(to) {
        return;
    }
    let c = conn.lock().unwrap();
    if to.is_dir() {
        if let Ok(n) = dirs::rewrite_prefix(&c, from, to) {
            ch.moved += n;
        }
        drop(c);
        sync_path(conn, to, ch); // evtl. noch unbekannte Dateien darin
        return;
    }
    match track_id(&c, from) {
        Some(id) if is_audio(to) => {
            if c.execute(
                "UPDATE tracks SET path = ?2 WHERE id = ?1",
                params![id, to.to_string_lossy()],
            )
            .is_ok()
            {
                ch.moved += 1;
            }
        }
        Some(id) => {
            // Endung geändert zu etwas anderem als Audio → raus
            let _ = c.execute("DELETE FROM tracks WHERE id = ?1", [id]);
            ch.removed += 1;
        }
        None => {
            drop(c);
            sync_path(conn, to, ch);
        }
    }
}

/// Startet die Überwachung. `on_change` bekommt gebündelte Änderungen (nie leer).
pub fn start(
    conn: Arc<Mutex<Connection>>,
    roots: &[PathBuf],
    on_change: impl Fn(Change) + Send + 'static,
) -> Result<Watcher> {
    let handler = move |res: DebounceEventResult| {
        let Ok(events) = res else { return };
        let mut ch = Change::default();
        let mut touched: BTreeSet<PathBuf> = BTreeSet::new();
        for ev in events {
            match ev.event.kind {
                EventKind::Modify(ModifyKind::Name(RenameMode::Both))
                    if ev.event.paths.len() == 2 =>
                {
                    rename(&conn, &ev.event.paths[0], &ev.event.paths[1], &mut ch);
                }
                EventKind::Access(_) => {}
                _ => touched.extend(ev.event.paths.iter().cloned()),
            }
        }
        for p in touched {
            sync_path(&conn, &p, &mut ch);
        }
        if !ch.is_empty() {
            on_change(ch);
        }
    };
    *ROOTS.lock().unwrap() = roots.to_vec();
    // 1,5 s warten: eigene Änderungen der App sind bis dahin in der Datenbank
    let mut deb = new_debouncer(Duration::from_millis(1500), None, handler)?;
    for r in roots {
        if r.is_dir() {
            let _ = deb.watch(r, RecursiveMode::Recursive);
        }
    }
    Ok(deb)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_new_moved_removed() {
        let base = std::env::temp_dir().join(format!("mlwatch{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("m/A")).unwrap();
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sine.mp3");
        if !fixture.exists() {
            return;
        }
        let conn = Arc::new(Mutex::new(crate::db::open(&base.join("t.db")).unwrap()));
        let (tx, rx) = std::sync::mpsc::channel();
        let _w = start(conn.clone(), &[base.join("m")], move |c| {
            let _ = tx.send(c);
        })
        .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        std::fs::copy(&fixture, base.join("m/A/x.mp3")).unwrap();
        let c = rx.recv_timeout(Duration::from_secs(8)).expect("neue Datei");
        assert_eq!(c.added.len(), 1);
        let id = c.added[0];
        std::fs::create_dir_all(base.join("m/B")).unwrap();
        std::fs::rename(base.join("m/A/x.mp3"), base.join("m/B/y.mp3")).unwrap();
        let mut path = String::new();
        for _ in 0..3 {
            if rx.recv_timeout(Duration::from_secs(8)).is_err() {
                break;
            }
            path = conn
                .lock()
                .unwrap()
                .query_row("SELECT path FROM tracks WHERE id = ?1", [id], |r| r.get(0))
                .unwrap_or_default();
            if path.ends_with("y.mp3") {
                break;
            }
        }
        assert!(
            path.ends_with("y.mp3"),
            "ID bleibt beim Verschieben erhalten: {path}"
        );
        std::fs::remove_file(base.join("m/B/y.mp3")).unwrap();
        let c = rx.recv_timeout(Duration::from_secs(8)).expect("gelöscht");
        assert_eq!(c.removed, 1);
        let _ = std::fs::remove_dir_all(&base);
    }
}

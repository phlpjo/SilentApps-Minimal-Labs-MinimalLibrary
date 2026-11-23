use crate::{Progress, is_audio, now, tags};
use anyhow::Result;
use rayon::prelude::*;
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ScanReport {
    pub found: usize,
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub errors: Vec<String>,
}

fn file_stat(p: &Path) -> (i64, i64) {
    match std::fs::metadata(p) {
        Ok(m) => {
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (m.len() as i64, mtime)
        }
        Err(_) => (0, 0),
    }
}

/// Schreibt gelesene Tags + Dateiinfos in die Datenbank (neu oder aktualisiert).
pub fn upsert(conn: &Connection, path: &Path, t: &tags::TagData) -> Result<i64> {
    let (size, mtime) = file_stat(path);
    let p = path.to_string_lossy().to_string();
    conn.execute(
        "INSERT INTO tracks (path, size, mtime, added, title, artist, album, album_artist, genre, year, track_no, disc_no, comment, duration, bitrate, sample_rate, channels, has_cover, tag_bpm, tag_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
         ON CONFLICT(path) DO UPDATE SET size=excluded.size, mtime=excluded.mtime, title=excluded.title, artist=excluded.artist,
           album=excluded.album, album_artist=excluded.album_artist, genre=excluded.genre, year=excluded.year,
           track_no=excluded.track_no, disc_no=excluded.disc_no, comment=excluded.comment, duration=excluded.duration,
           bitrate=excluded.bitrate, sample_rate=excluded.sample_rate, channels=excluded.channels,
           has_cover=excluded.has_cover, tag_bpm=excluded.tag_bpm, tag_key=excluded.tag_key",
        params![
            p, size, mtime, now(), t.title, t.artist, t.album, t.album_artist, t.genre, t.year, t.track_no,
            t.disc_no, t.comment, t.duration, t.bitrate, t.sample_rate, t.channels, t.has_cover as i64, t.bpm, t.key
        ],
    )?;
    Ok(conn.query_row("SELECT id FROM tracks WHERE path = ?1", [p], |r| r.get(0))?)
}

/// Tags einer bekannten Datei neu einlesen (z. B. nach dem Bearbeiten). Analyse bleibt erhalten.
pub fn refresh(conn: &Connection, path: &Path) -> Result<i64> {
    let t = tags::read(path)?;
    upsert(conn, path, &t)
}

pub fn scan(conn: &Mutex<Connection>, roots: &[PathBuf], progress: Progress) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    progress(0, 0, "Dateien suchen …");
    let mut files: Vec<PathBuf> = Vec::new();
    for root in roots {
        for e in walkdir::WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if e.file_type().is_file() && is_audio(e.path()) {
                files.push(e.into_path());
                if files.len() % 500 == 0 {
                    progress(0, 0, &format!("{} Dateien gefunden …", files.len()));
                }
            }
        }
    }
    report.found = files.len();

    let existing: HashMap<String, (i64, i64)> = {
        let c = conn.lock().unwrap();
        let mut st = c.prepare("SELECT path, size, mtime FROM tracks")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let todo: Vec<&PathBuf> = files
        .iter()
        .filter(|f| match existing.get(f.to_string_lossy().as_ref()) {
            Some(&(size, mtime)) => file_stat(f) != (size, mtime),
            None => true,
        })
        .collect();

    let total = todo.len();
    let done = AtomicUsize::new(0);
    let results: Vec<(PathBuf, Result<tags::TagData>)> = todo
        .par_iter()
        .map(|p| {
            let r = tags::read(p);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 25 == 0 || n == total {
                progress(n, total, "Tags lesen …");
            }
            ((*p).clone(), r)
        })
        .collect();

    {
        let mut c = conn.lock().unwrap();
        let tx = c.transaction()?;
        for (path, r) in results {
            let key = path.to_string_lossy().to_string();
            let is_new = !existing.contains_key(&key);
            let data = match r {
                Ok(d) => d,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", path.display()));
                    tags::TagData::default()
                }
            };
            upsert(&tx, &path, &data)?;
            if is_new {
                report.added += 1;
            } else {
                // Geänderte Datei → Analyse neu
                tx.execute("UPDATE tracks SET analyzed = 0 WHERE path = ?1", [&key])?;
                report.updated += 1;
            }
        }
        // Verschwundene Dateien innerhalb der Musikordner entfernen
        let found: HashSet<String> = files
            .iter()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        for path in existing.keys() {
            let inside = roots.iter().any(|r| Path::new(path).starts_with(r));
            if inside && !found.contains(path) {
                // gemerkt: taucht die Datei woanders auf, behält sie ID und Playlist-Plätze
                let ids: Vec<i64> = {
                    let mut st = tx.prepare("SELECT id FROM tracks WHERE path = ?1")?;
                    st.query_map([path], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?
                };
                crate::db::forget_tracks(&tx, &ids)?;
                report.removed += 1;
            }
        }
        tx.commit()?;
    }
    progress(total, total, "Fertig");
    Ok(report)
}

/// Einzelne Dateien hinzufügen (Drag & Drop).
pub fn add_files(conn: &Mutex<Connection>, paths: &[PathBuf]) -> Result<Vec<i64>> {
    let mut files = Vec::new();
    for p in paths {
        if p.is_dir() {
            for e in walkdir::WalkDir::new(p).into_iter().filter_map(|e| e.ok()) {
                if e.file_type().is_file() && is_audio(e.path()) {
                    files.push(e.into_path());
                }
            }
        } else if is_audio(p) {
            files.push(p.clone());
        }
    }
    let read: Vec<(PathBuf, tags::TagData)> = files
        .into_par_iter()
        .map(|p| {
            let t = tags::read(&p).unwrap_or_default();
            (p, t)
        })
        .collect();
    let c = conn.lock().unwrap();
    let mut ids = Vec::new();
    for (p, t) in read {
        ids.push(upsert(&c, &p, &t)?);
    }
    Ok(ids)
}

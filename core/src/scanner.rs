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
/// Ist die Datei wirklich weg – oder nur gerade nicht erreichbar (NAS schläft, Netzwerk hakt, Cloud lädt)?
/// Weg ist sie nur, wenn ein darüberliegender Ordner sich lesen lässt und der Eintrag darin fehlt.
/// `cache` merkt sich Ordnerinhalte (für viele Prüfungen in einem Rutsch).
pub fn really_gone(p: &Path, cache: &mut HashMap<PathBuf, Option<HashSet<String>>>) -> bool {
    if p.exists() {
        return false;
    }
    let mut child = p;
    let mut cur = match p.parent() {
        Some(x) => x,
        None => return false,
    };
    loop {
        let listing = cache
            .entry(cur.to_path_buf())
            .or_insert_with(|| {
                std::fs::read_dir(cur).ok().map(|rd| {
                    rd.flatten()
                        .map(|e| e.file_name().to_string_lossy().to_lowercase())
                        .collect()
                })
            })
            .clone();
        match listing {
            Some(names) => {
                let name = child.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
                // Eintrag noch da (nur tiefer nicht erreichbar) → nicht weg
                return !names.contains(&name);
            }
            None => {
                // Ordner nicht lesbar: weiter nach oben – ist bis ganz oben nichts lesbar, ist das Laufwerk weg
                child = cur;
                cur = match cur.parent() {
                    Some(x) => x,
                    None => return false,
                };
            }
        }
    }
}

/// Datei inzwischen gelöscht oder verschoben? Dann darf ein Scan oder die Ordnerüberwachung, die noch den alten
/// Stand kennt, sie nicht wieder in die Bibliothek holen (sonst tauchen gelöschte Titel wieder auf).
pub fn gone(path: &Path) -> bool {
    !path.exists()
}

pub fn upsert(conn: &Connection, path: &Path, t: &tags::TagData) -> Result<i64> {
    if gone(path) {
        anyhow::bail!("{}: nicht mehr vorhanden", path.display());
    }
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

/// Alle Audiodateien unter `root` – auch in Unterordnern, über Verknüpfungen hinweg.
/// Ordner, die sich nicht lesen lassen (Cloud-Laufwerk lädt noch, kurz gesperrt …), werden einmal neu versucht;
/// was dann noch fehlt, landet in `bad` (dort wird später nichts als „verschwunden“ gelöscht).
fn walk(root: &Path, files: &mut Vec<PathBuf>, bad: &mut Vec<(PathBuf, String)>, progress: Progress) {
    let mut retry: Vec<PathBuf> = Vec::new();
    let visit = |dir: &Path, files: &mut Vec<PathBuf>, failed: &mut Vec<(PathBuf, String)>| {
        for e in walkdir::WalkDir::new(dir).follow_links(true).into_iter() {
            match e {
                Ok(e) => {
                    if e.file_type().is_file() && is_audio(e.path()) {
                        files.push(e.into_path());
                        if files.len() % 500 == 0 {
                            progress(0, 0, &format!("{} Dateien gefunden …", files.len()));
                        }
                    }
                }
                Err(err) => {
                    // Schleife durch Verknüpfungen: einfach überspringen
                    if err.loop_ancestor().is_some() {
                        continue;
                    }
                    if let Some(p) = err.path() {
                        failed.push((p.to_path_buf(), err.to_string()));
                    }
                }
            }
        }
    };
    let mut failed = Vec::new();
    visit(root, files, &mut failed);
    for (p, _) in &failed {
        if p.is_dir() || !p.exists() {
            retry.push(p.clone());
        }
    }
    if failed.is_empty() {
        return;
    }
    std::thread::sleep(std::time::Duration::from_millis(400));
    let mut still = Vec::new();
    for (p, e) in failed {
        if retry.contains(&p) && p.is_dir() {
            let before = still.len();
            visit(&p, files, &mut still);
            if still.len() == before {
                continue; // beim zweiten Versuch geklappt
            }
        } else {
            still.push((p, e));
        }
    }
    bad.extend(still);
}

pub fn scan(conn: &Mutex<Connection>, roots: &[PathBuf], progress: Progress) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    progress(0, 0, "Dateien suchen …");
    let mut files: Vec<PathBuf> = Vec::new();
    let mut bad: Vec<(PathBuf, String)> = Vec::new();
    for root in roots {
        // Musikordner gerade nicht erreichbar (NAS aus, Laufwerk getrennt): nichts davon als gelöscht werten
        if !root.is_dir() {
            bad.push((root.clone(), "nicht erreichbar".into()));
            continue;
        }
        walk(root, &mut files, &mut bad, progress);
    }
    // doppelte (über Verknüpfungen zweimal gefunden) entfernen, in Ordner-Reihenfolge
    files.sort();
    files.dedup();
    report.found = files.len();
    for (p, e) in &bad {
        report.errors.push(format!("Ordner nicht lesbar: {}: {e}", p.display()));
    }

    let existing: HashMap<String, (i64, i64)> = {
        let c = conn.lock().unwrap();
        let mut st = c.prepare("SELECT path, size, mtime FROM tracks")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?))))?;
        rows.collect::<rusqlite::Result<_>>()?
    };

    let mut todo: Vec<&PathBuf> = files
        .iter()
        .filter(|f| match existing.get(f.to_string_lossy().as_ref()) {
            Some(&(size, mtime)) => file_stat(f) != (size, mtime),
            None => true,
        })
        .collect();
    // Früher verschwundene Titel (z. B. als das NAS kurz weg war) mit alter ID, Analyse und Playlist-Plätzen zurückholen
    {
        let c = conn.lock().unwrap();
        todo.retain(|f| {
            if existing.contains_key(f.to_string_lossy().as_ref()) {
                return true;
            }
            let (size, mtime) = file_stat(f);
            match crate::db::revive_lost(&c, f, size, mtime) {
                Ok(Some(_)) => {
                    report.updated += 1;
                    false
                }
                _ => true,
            }
        });
    }

    // In Paketen lesen und gleich speichern: neue Titel erscheinen schon während des Scans
    let total = todo.len();
    let done = AtomicUsize::new(0);
    for chunk in todo.chunks(300) {
        let results: Vec<(PathBuf, Result<tags::TagData>)> = chunk
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
        let mut c = conn.lock().unwrap();
        let tx = c.transaction()?;
        for (path, r) in results {
            // Während des Scans gelöscht oder verschoben → nicht wieder aufnehmen
            if gone(&path) {
                continue;
            }
            let key = path.to_string_lossy().to_string();
            let is_new = !existing.contains_key(&key);
            let old_dur: Option<f64> = if is_new {
                None
            } else {
                tx.query_row("SELECT duration FROM tracks WHERE path = ?1", [&key], |r| r.get(0)).ok()
            };
            let data = match r {
                Ok(d) => d,
                Err(e) => {
                    report.errors.push(format!("{}: {e}", path.display()));
                    tags::TagData::default()
                }
            };
            let new_dur = data.duration;
            upsert(&tx, &path, &data)?;
            if is_new {
                report.added += 1;
            } else {
                // Nur Tags geändert (gleiche Länge) → Analyse bleibt. Neu analysieren nur, wenn sich der Ton geändert hat.
                let audio_changed = match old_dur {
                    Some(o) => o <= 0.0 || new_dur <= 0.0 || (o - new_dur).abs() > 0.5,
                    None => true,
                };
                if audio_changed {
                    tx.execute("UPDATE tracks SET analyzed = 0 WHERE path = ?1", [&key])?;
                }
                report.updated += 1;
            }
        }
        tx.commit()?;
        drop(c);
        // Oberfläche: neue Titel nachladen
        progress(done.load(Ordering::Relaxed), total, "Titel gespeichert …");
    }

    {
        let mut c = conn.lock().unwrap();
        let tx = c.transaction()?;
        // Verschwundene Dateien innerhalb der Musikordner entfernen – nicht in Ordnern, die gerade nicht lesbar waren
        let found: HashSet<String> = files
            .iter()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        let mut listing = HashMap::new();
        let mut kept = 0usize;
        for path in existing.keys() {
            let inside = roots.iter().any(|r| Path::new(path).starts_with(r));
            let unreadable = bad.iter().any(|(b, _)| Path::new(path).starts_with(b));
            if inside && !unreadable && !found.contains(path) {
                // Nicht gefunden ≠ gelöscht: nur entfernen, wenn der Ordner lesbar ist und die Datei darin fehlt
                // (Netzlaufwerk/NAS kurz weg → Titel bleiben)
                if !really_gone(Path::new(path), &mut listing) {
                    kept += 1;
                    continue;
                }
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
        if kept > 0 {
            report.errors.push(format!("Ordner nicht lesbar: {kept} Titel nicht erreichbar – bleiben in der Bibliothek"));
        }
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
        if gone(&p) {
            continue;
        }
        // schon mal da gewesen (verschwunden) → alte ID samt Analyse zurück
        let (size, mtime) = file_stat(&p);
        if let Ok(Some(id)) = crate::db::revive_lost(&c, &p, size, mtime) {
            ids.push(id);
            continue;
        }
        ids.push(upsert(&c, &p, &t)?);
    }
    Ok(ids)
}

#[cfg(test)]
mod walk_tests {
    use super::*;

    #[test]
    fn finds_files_in_all_subfolders() {
        let dir = std::env::temp_dir().join(format!("ml-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let deep = dir.join("a").join("b c").join("d.e").join("ä ö");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::create_dir_all(dir.join(".versteckt")).unwrap();
        for (i, p) in [dir.join("1.mp3"), dir.join("a").join("2.FLAC"), deep.join("3.m4a"), dir.join(".versteckt").join("4.mp3"), dir.join("a").join("x.txt")]
            .iter()
            .enumerate()
        {
            std::fs::write(p, [i as u8]).unwrap();
        }
        let mut files = Vec::new();
        let mut bad = Vec::new();
        walk(&dir, &mut files, &mut bad, &|_, _, _| {});
        assert_eq!(files.len(), 4, "{files:?}");
        assert!(bad.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod gone_tests {
    use super::*;

    #[test]
    fn missing_vs_unreachable() {
        let dir = std::env::temp_dir().join(format!("ml-gone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("da")).unwrap();
        std::fs::write(dir.join("da").join("x.mp3"), [0]).unwrap();
        let mut c = HashMap::new();
        assert!(!really_gone(&dir.join("da").join("x.mp3"), &mut c), "vorhanden");
        assert!(really_gone(&dir.join("da").join("weg.mp3"), &mut c), "fehlt in lesbarem Ordner");
        assert!(really_gone(&dir.join("geloescht").join("y.mp3"), &mut c), "ganzer Ordner gelöscht");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

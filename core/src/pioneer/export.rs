//! Export auf USB-Stick/SD-Karte für Pioneer DJ / AlphaTheta-Player (OneLibrary).
//!
//! Ablauf: Titel der gewählten Playlisten → Audiodateien nach /Contents kopieren (nur Fehlendes) →
//! Analysedateien (ANLZ) schreiben → Datenbank ergänzen/aktualisieren → Playlisten anlegen.
//! Was rekordbox oder andere Programme auf dem Laufwerk angelegt haben, wird nie verändert:
//! Welche Einträge von uns stammen, steht in einer eigenen Merkliste neben der Datenbank.

use super::{anlz, onelibrary as ol};
use crate::db::{self, Track};
use crate::{Progress, now, paths, playlists, tags};
use anyhow::{Result, anyhow, bail};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// OFFEN (nicht in der erlaubten Referenz dokumentiert): Ordner für Audio, Analysedateien, Cover.
pub const CONTENTS: &str = "Contents";
pub const ANLZ_DIR: &str = "PIONEER/USBANLZ";
pub const ARTWORK_DIR: &str = "PIONEER/Artwork";
/// Eigene Merkliste: was dieser Export angelegt hat
pub const MANIFEST: &str = "PIONEER/rekordbox/minimal-library-export.json";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum Unsupported {
    /// nicht abspielbare Formate weglassen
    #[default]
    Skip,
    /// mit ffmpeg nach MP3 320 kbit/s umwandeln
    Convert,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    pub dest: String,
    pub playlists: Vec<i64>,
    #[serde(default)]
    pub unsupported: Unsupported,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub tracks: usize,
    pub bytes: u64,
    pub to_copy: usize,
    pub copy_bytes: u64,
    pub missing_analysis: Vec<i64>,
    /// (Titel-ID, Anzeige, Endung)
    pub unsupported: Vec<(i64, String, String)>,
    pub free: Option<u64>,
    /// Auf dem Laufwerk liegt schon eine Bibliothek (z. B. von rekordbox)
    pub existing_library: bool,
    /// Titel, die in einer Playlist doppelt vorkommen (die Datenbank erlaubt das nicht)
    pub duplicates_in_playlists: usize,
    pub ffmpeg: bool,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub tracks: usize,
    pub copied: usize,
    pub converted: usize,
    pub skipped: usize,
    pub playlists: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Manifest {
    /// unsere Titel-ID → Eintrag auf dem Laufwerk
    tracks: BTreeMap<i64, Entry>,
    /// unsere Playlist-ID → playlist_id in der Datenbank
    playlists: BTreeMap<i64, i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
struct Entry {
    content_id: i64,
    /// Pfad auf dem Gerät, z. B. "/Contents/Band/Album/Titel.mp3"
    path: String,
    anlz: String,
    size: u64,
}

fn ext_of(p: &str) -> String {
    Path::new(p)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn label(t: &Track) -> String {
    format!(
        "{} – {}",
        t.artist.as_deref().unwrap_or("?"),
        t.title.clone().unwrap_or_else(|| Path::new(&t.path)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string())
    )
}

fn on_disk(root: &Path, device_path: &str) -> PathBuf {
    root.join(device_path.trim_start_matches('/'))
}

fn read_manifest(root: &Path) -> Manifest {
    std::fs::read(root.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Titel der gewählten Playlisten (Reihenfolge der ersten Fundstelle) und Playlist-Inhalte ohne Doppelte
fn collect(
    c: &Connection,
    pls: &[i64],
) -> Result<(Vec<Track>, Vec<(i64, String, Vec<i64>)>, usize)> {
    let mut seen = HashSet::new();
    let mut order = Vec::new();
    let mut lists = Vec::new();
    let mut dups = 0;
    for &pid in pls {
        let name = playlists::name(c, pid)?;
        let ids = playlists::track_ids(c, pid)?;
        let mut inner = HashSet::new();
        let mut uniq = Vec::new();
        for id in ids {
            if inner.insert(id) {
                uniq.push(id);
            } else {
                dups += 1;
            }
            if seen.insert(id) {
                order.push(id);
            }
        }
        lists.push((pid, name, uniq));
    }
    Ok((db::tracks_by_ids(c, &order)?, lists, dups))
}

pub fn preview(c: &Connection, o: &Options, ffmpeg: bool) -> Result<Preview> {
    let root = Path::new(&o.dest);
    let (tracks, _, dups) = collect(c, &o.playlists)?;
    let man = read_manifest(root);
    let mut p = Preview {
        tracks: tracks.len(),
        duplicates_in_playlists: dups,
        ffmpeg,
        ..Default::default()
    };
    let analysed: HashSet<i64> = {
        let mut st = c.prepare("SELECT track_id FROM pioneer_analysis WHERE frames IS NOT NULL")?;
        st.query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?
    };
    for t in &tracks {
        let size = t.size.max(0) as u64;
        p.bytes += size;
        let ext = ext_of(&t.path);
        if ol::file_type(&ext).is_none() {
            p.unsupported.push((t.id, label(t), ext));
        }
        if !analysed.contains(&t.id) {
            p.missing_analysis.push(t.id);
        }
        let present = man
            .tracks
            .get(&t.id)
            .is_some_and(|e| std::fs::metadata(on_disk(root, &e.path)).is_ok());
        if !present {
            p.to_copy += 1;
            p.copy_bytes += size;
        }
    }
    if root.is_dir() {
        p.free = fs4::available_space(root).ok();
    }
    p.existing_library = root.join(ol::DB_REL).exists();
    Ok(p)
}

/// Freien Pfad auf dem Gerät finden – nie eine fremde Datei überschreiben
fn free_device_path(root: &Path, wanted: &str, ours: &HashSet<String>) -> String {
    let p = PathBuf::from(wanted);
    let taken = |q: &Path| {
        let s = q.to_string_lossy().replace('\\', "/");
        on_disk(root, &s).exists() && !ours.contains(&s)
    };
    paths::unique_path(&p, &taken)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Kurzer, stabiler Ordnername für die Analysedateien (wie rekordbox: P + 3 Hex / 8 Hex)
fn anlz_path(device_path: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in device_path.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!(
        "/{ANLZ_DIR}/P{:03X}/{:08X}/ANLZ0000.DAT",
        (h >> 52) & 0xFFF,
        h as u32
    )
}

fn copy_file(src: &Path, dst: &Path) -> std::io::Result<()> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = dst.with_extension("part");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dst)
}

fn convert(ffmpeg: &Path, src: &Path, dst: &Path) -> Result<()> {
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = dst.with_extension("part.mp3");
    let mut cmd = std::process::Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(src)
        .args([
            "-map",
            "0:a:0",
            "-map_metadata",
            "0",
            "-codec:a",
            "libmp3lame",
            "-b:a",
            "320k",
            "-id3v2_version",
            "3",
        ])
        .arg(&tmp);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output()?;
    if !out.status.success() {
        let _ = std::fs::remove_file(&tmp);
        bail!(
            "Umwandlung fehlgeschlagen: {}",
            String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("")
        );
    }
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

/// Tonart in der Schreibweise der Player (z. B. "Am", "F#")
fn key_name(k: &str) -> String {
    k.trim().to_string()
}

/// Export ausführen. Fehlende Pioneer-Analysen müssen vorher nachgeholt sein (macht die App).
pub fn run(
    conn: &Mutex<Connection>,
    o: &Options,
    ffmpeg: Option<&Path>,
    encrypted: bool,
    cancel: &AtomicBool,
    progress: Progress,
) -> Result<Report> {
    let root = PathBuf::from(&o.dest);
    if !root.is_dir() {
        bail!("Ziel „{}“ ist nicht erreichbar", o.dest);
    }
    let (tracks, lists, _) = collect(&conn.lock().unwrap(), &o.playlists)?;
    let mut man = read_manifest(&root);
    let ours_paths: HashSet<String> = man
        .tracks
        .values()
        .flat_map(|e| [e.path.clone(), e.anlz.clone()])
        .collect();
    // Platz prüfen (ungefähr: alles, was noch nicht da ist)
    let need: u64 = tracks
        .iter()
        .filter(|t| !man.tracks.contains_key(&t.id))
        .map(|t| t.size.max(0) as u64)
        .sum();
    if let Ok(free) = fs4::available_space(&root) {
        if need > free + 1024 * 1024 {
            bail!(
                "Nicht genug Platz: {:.1} GB nötig, {:.1} GB frei",
                need as f64 / 1e9,
                free as f64 / 1e9
            );
        }
    }
    let (db, _) = ol::open(&root.join(ol::DB_REL), encrypted)?;
    let mut rep = Report::default();
    let total = tracks.len() + lists.len();
    let stamp = ol::timestamp(now());
    let mut new_paths: HashSet<String> = HashSet::new();

    db.execute_batch("BEGIN")?;
    let result = (|| -> Result<()> {
        for (i, t) in tracks.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                rep.errors.push("Abgebrochen".into());
                break;
            }
            progress(i, total, &label(t));
            let src = PathBuf::from(&t.path);
            let ext = ext_of(&t.path);
            let playable = ol::file_type(&ext).is_some();
            if !playable && (o.unsupported == Unsupported::Skip || ffmpeg.is_none()) {
                rep.skipped += 1;
                continue;
            }
            let out_ext = if playable { ext.clone() } else { "mp3".into() };
            // Pfad auf dem Gerät: bisheriger (von uns) oder neuer freier
            let prev = man.tracks.get(&t.id).cloned();
            let device_path = match &prev {
                Some(e) if ext_of(&e.path) == out_ext => e.path.clone(),
                _ => {
                    let rel =
                        crate::organize::render("{albumartist}/{album}/{artist} - {title}", t);
                    let wanted = format!(
                        "/{CONTENTS}/{}.{out_ext}",
                        rel.to_string_lossy().replace('\\', "/")
                    );
                    let mut allowed = ours_paths.clone();
                    allowed.retain(|p| !new_paths.contains(p));
                    free_device_path(&root, &wanted, &allowed)
                }
            };
            new_paths.insert(device_path.clone());
            let dst = on_disk(&root, &device_path);
            let size_now = std::fs::metadata(&dst).map(|m| m.len()).ok();
            let need_copy = match (&prev, size_now) {
                (Some(e), Some(s)) => {
                    playable && s != t.size.max(0) as u64 || e.path != device_path
                }
                (_, None) => true,
                (None, Some(_)) => false, // fremde Datei: free_device_path verhindert das – zur Sicherheit nicht anfassen
            };
            if need_copy {
                let r = if playable {
                    copy_file(&src, &dst).map_err(anyhow::Error::from)
                } else {
                    convert(ffmpeg.unwrap(), &src, &dst)
                };
                if let Err(e) = r {
                    rep.errors.push(format!("{}: {e}", label(t)));
                    continue;
                }
                if playable {
                    rep.copied += 1
                } else {
                    rep.converted += 1
                }
                rep.bytes += std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);
            }
            let file_size = std::fs::metadata(&dst).map(|m| m.len()).unwrap_or(0);

            // Analysedateien
            let (grid, frames, cues) = {
                let c = conn.lock().unwrap();
                (
                    super::grid(&c, t.id)?,
                    super::frames(&c, t.id)?.unwrap_or_default(),
                    super::cues(&c, t.id)?,
                )
            };
            let anlz_dev = prev.as_ref().map(|e| e.anlz.clone()).unwrap_or_else(|| {
                let mut p = anlz_path(&device_path);
                let mut n = 1;
                while on_disk(&root, &p).exists() && !ours_paths.contains(&p) {
                    p = anlz_path(&format!("{device_path}#{n}"));
                    n += 1;
                }
                p
            });
            let duration_ms = t.duration * 1000.0;
            let a = anlz::TrackAnalysis {
                device_path: &device_path,
                duration_ms,
                grid,
                frames: &frames,
                cues: &cues,
            };
            let dat = on_disk(&root, &anlz_dev);
            if let Some(d) = dat.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(&dat, anlz::build_dat(&a))?;
            std::fs::write(dat.with_extension("EXT"), anlz::build_ext(&a))?;

            // Cover
            let image_id = match tags::cover(&src).ok().flatten() {
                Some((mime, bytes)) if mime.contains("jpeg") || mime.contains("jpg") => {
                    // Eigener Unterordner, damit nie Cover anderer Programme getroffen werden.
                    // OFFEN: ob die Player bestimmte Ordner/Namen oder verkleinerte Fassungen erwarten.
                    let img = format!("/{ARTWORK_DIR}/minimal-library/a{}.jpg", t.id);
                    let f = on_disk(&root, &img);
                    if let Some(d) = f.parent() {
                        std::fs::create_dir_all(d)?;
                    }
                    std::fs::write(&f, &bytes)?;
                    Some(ol::id_for(&db, "image", "path", &img)?)
                }
                _ => None,
            };

            // Datenbank: Stammdaten
            let opt_id = |table: &str, v: &Option<String>| -> Result<Option<i64>> {
                match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    Some(s) => Ok(Some(ol::id_for(&db, table, "name", s)?)),
                    None => Ok(None),
                }
            };
            let artist = opt_id("artist", &t.artist)?;
            let album = opt_id("album", &t.album)?;
            let genre = opt_id("genre", &t.genre)?;
            let key = match &t.key {
                Some(k) if !k.is_empty() => Some(ol::id_for(&db, "\"key\"", "name", &key_name(k))?),
                _ => None,
            };
            let bpm = grid.map(|g| g.bpm).or(t.bpm).unwrap_or(0.0);
            let title = t.title.clone().unwrap_or_else(|| {
                Path::new(&t.path)
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            });
            let file_name = Path::new(&device_path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let year: Option<i64> = t
                .year
                .as_deref()
                .and_then(|y| y.get(0..4))
                .and_then(|y| y.parse().ok());
            let ftype = ol::file_type(&out_ext).unwrap_or(1);
            let bitrate = if playable {
                t.bitrate.unwrap_or(0) as i64
            } else {
                320
            };
            let rate = t.sample_rate.unwrap_or(44_100) as i64;
            // OFFEN: Bittiefe ist bei verlustbehafteten Formaten ohne Bedeutung; 16 als üblicher Wert
            let bits = 16i64;
            let existing: Option<i64> = match &prev {
                Some(e) => db
                    .query_row(
                        "SELECT content_id FROM content WHERE content_id = ?1",
                        [e.content_id],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            let content_id = match existing {
                Some(id) => {
                    db.execute(
                        "UPDATE content SET title=?2, titleForSearch=?3, bpmx100=?4, length=?5, trackNo=?6, discNo=?7, artist_id_artist=?8, album_id=?9, genre_id=?10,
                         key_id=?11, image_id=?12, djComment=?13, releaseYear=?14, path=?15, fileName=?16, fileSize=?17, fileType=?18, bitrate=?19, bitDepth=?20,
                         samplingRate=?21, analysisDataFilePath=?22, hasModified=0,
                         cueUpdateCount=COALESCE(cueUpdateCount,0)+1, analysisDataUpdateCount=COALESCE(analysisDataUpdateCount,0)+1,
                         informationUpdateCount=COALESCE(informationUpdateCount,0)+1 WHERE content_id=?1",
                        params![id, title, ol::for_search(&title), (bpm * 100.0).round() as i64, duration_ms.round() as i64, t.track_no, t.disc_no, artist, album, genre,
                            key, image_id, t.comment, year, device_path, file_name, file_size as i64, ftype, bitrate, bits, rate, anlz_dev],
                    )?;
                    id
                }
                None => {
                    db.execute(
                        "INSERT INTO content(title, titleForSearch, bpmx100, length, trackNo, discNo, artist_id_artist, album_id, genre_id, key_id, image_id,
                         djComment, releaseYear, dateCreated, dateAdded, path, fileName, fileSize, fileType, bitrate, bitDepth, samplingRate,
                         analysisDataFilePath, hasModified, cueUpdateCount, analysisDataUpdateCount, informationUpdateCount)
                         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?14,?15,?16,?17,?18,?19,?20,?21,?22,0,0,0,0)",
                        params![title, ol::for_search(&title), (bpm * 100.0).round() as i64, duration_ms.round() as i64, t.track_no, t.disc_no, artist, album, genre,
                            key, image_id, t.comment, year, stamp, device_path, file_name, file_size as i64, ftype, bitrate, bits, rate, anlz_dev],
                    )?;
                    db.last_insert_rowid()
                }
            };
            // Cues: unsere Einträge ersetzen
            db.execute("DELETE FROM cue WHERE content_id = ?1", [content_id])?;
            for q in &cues {
                // OFFEN: Bedeutung von „kind“ (hier 0 = Memory Cue, 1–8 = Hot Cue A–H wie die Nummer in ANLZ)
                db.execute(
                    "INSERT INTO cue(content_id, kind, colorTableIndex, cueComment, isActiveLoop, inUsec, outUsec, in150FramePerSec, out150FramePerSec)
                     VALUES(?1, ?2, ?3, ?4, 0, ?5, ?6, ?7, ?8)",
                    params![content_id, q.kind, q.color.unwrap_or(0), q.comment, (q.start_ms * 1000.0).round() as i64,
                        q.end_ms.map(|e| (e * 1000.0).round() as i64), (q.start_ms * 0.15).round() as i64, q.end_ms.map(|e| (e * 0.15).round() as i64)],
                )?;
            }
            man.tracks.insert(
                t.id,
                Entry {
                    content_id,
                    path: device_path,
                    anlz: anlz_dev,
                    size: file_size,
                },
            );
            rep.tracks += 1;
        }

        // Playlisten
        for (pid, name, ids) in &lists {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            progress(tracks.len() + rep.playlists, total, name);
            let existing: Option<i64> = match man.playlists.get(pid) {
                Some(id) => db
                    .query_row(
                        "SELECT playlist_id FROM playlist WHERE playlist_id = ?1",
                        [id],
                        |r| r.get(0),
                    )
                    .optional()?,
                None => None,
            };
            let plid = match existing {
                Some(id) => {
                    db.execute(
                        "UPDATE playlist SET name = ?2 WHERE playlist_id = ?1",
                        params![id, name],
                    )?;
                    id
                }
                None => {
                    let seq: i64 = db.query_row("SELECT COALESCE(MAX(sequenceNo), 0) + 1 FROM playlist WHERE COALESCE(playlist_id_parent, 0) = 0", [], |r| r.get(0))?;
                    db.execute("INSERT INTO playlist(sequenceNo, name, attribute, playlist_id_parent) VALUES(?1, ?2, 0, 0)", params![seq, name])?;
                    db.last_insert_rowid()
                }
            };
            db.execute(
                "DELETE FROM playlist_content WHERE playlist_id = ?1",
                [plid],
            )?;
            let mut seq = 1;
            for id in ids {
                if let Some(e) = man.tracks.get(id) {
                    db.execute("INSERT OR IGNORE INTO playlist_content(playlist_id, content_id, sequenceNo) VALUES(?1, ?2, ?3)", params![plid, e.content_id, seq])?;
                    seq += 1;
                }
            }
            man.playlists.insert(*pid, plid);
            rep.playlists += 1;
        }
        let device = root
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "USB".into());
        ol::set_property(&db, &device, now())?;
        Ok(())
    })();
    match result {
        Ok(()) => db.execute_batch("COMMIT")?,
        Err(e) => {
            let _ = db.execute_batch("ROLLBACK");
            return Err(e);
        }
    }
    let mf = root.join(MANIFEST);
    if let Some(d) = mf.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&mf, serde_json::to_vec_pretty(&man)?)?;
    progress(total, total, "Fertig");
    if rep.tracks == 0 && !tracks.is_empty() && rep.errors.is_empty() {
        return Err(anyhow!(
            "Kein Titel exportiert – alle Formate nicht abspielbar?"
        ));
    }
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pioneer::{Cue, set_cue, set_grid};

    #[test]
    fn export_twice_keeps_foreign_files() {
        let base = std::env::temp_dir().join(format!("mlpio{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("lib")).unwrap();
        std::fs::create_dir_all(base.join("stick/Contents/Band/Platte")).unwrap();
        // fremde Datei am Wunschort → darf nicht überschrieben werden
        std::fs::write(
            base.join("stick/Contents/Band/Platte/Band - Eins.mp3"),
            b"fremd",
        )
        .unwrap();
        let conn = Mutex::new(db::open(&base.join("t.db")).unwrap());
        let mk = |name: &str, title: &str| {
            let f = base.join("lib").join(name);
            std::fs::write(&f, vec![7u8; 2000]).unwrap();
            let c = conn.lock().unwrap();
            c.execute("INSERT INTO tracks(path, size, title, artist, album, duration, musical_key) VALUES(?1, 2000, ?2, 'Band', 'Platte', 180.0, 'Am')", params![f.to_string_lossy(), title]).unwrap();
            c.last_insert_rowid()
        };
        let a = mk("a.mp3", "Eins");
        let b = mk("b.ogg", "Zwei");
        {
            let c = conn.lock().unwrap();
            set_grid(&c, a, 128.0, 100.0, "t").unwrap();
            set_cue(
                &c,
                a,
                &Cue {
                    id: 0,
                    kind: 1,
                    start_ms: 1000.0,
                    end_ms: None,
                    color: Some(2),
                    comment: None,
                },
            )
            .unwrap();
        }
        let pid = {
            let mut c = conn.lock().unwrap();
            let p = playlists::create(&c, "Set", None).unwrap();
            playlists::set_items(&mut c, p, &[a, b, a]).unwrap();
            p
        };
        let o = Options {
            dest: base.join("stick").to_string_lossy().into(),
            playlists: vec![pid],
            unsupported: Unsupported::Skip,
        };
        let pv = preview(&conn.lock().unwrap(), &o, false).unwrap();
        assert_eq!(pv.tracks, 2);
        assert_eq!(pv.unsupported.len(), 1);
        assert_eq!(pv.duplicates_in_playlists, 1);
        let cancel = AtomicBool::new(false);
        let r = run(&conn, &o, None, false, &cancel, &|_, _, _| {}).unwrap();
        assert_eq!((r.tracks, r.copied, r.skipped, r.playlists), (1, 1, 1, 1));
        assert_eq!(
            std::fs::read(base.join("stick/Contents/Band/Platte/Band - Eins.mp3")).unwrap(),
            b"fremd"
        );
        assert!(
            base.join("stick/Contents/Band/Platte/Band - Eins (2).mp3")
                .exists()
        );
        // zweiter Lauf: nichts neu kopiert, Datenbank aktualisiert
        let r2 = run(&conn, &o, None, false, &cancel, &|_, _, _| {}).unwrap();
        assert_eq!(r2.copied, 0);
        let (db, created) = ol::open(&base.join("stick").join(ol::DB_REL), false).unwrap();
        assert!(!created);
        assert_eq!(
            db.query_row("SELECT count(*) FROM content", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT count(*) FROM playlist_content", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            db.query_row("SELECT bpmx100 FROM content", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            12800
        );
        assert_eq!(
            db.query_row("SELECT inUsec FROM cue", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1_000_000
        );
        let anlz: String = db
            .query_row("SELECT analysisDataFilePath FROM content", [], |r| r.get(0))
            .unwrap();
        let dat =
            anlz::parse(&std::fs::read(on_disk(&base.join("stick"), &anlz)).unwrap()).unwrap();
        assert_eq!(dat.path, "/Contents/Band/Platte/Band - Eins (2).mp3");
        assert_eq!(dat.beats[0].1, 12800);
        let _ = std::fs::remove_dir_all(&base);
    }
}

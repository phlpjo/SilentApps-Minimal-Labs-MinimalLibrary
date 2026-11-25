//! Playlisten mitsamt Original-Dateien exportieren (z. B. auf USB-Stick oder Handy-Speicher).

use crate::db::{self, Track};
use crate::{Progress, paths, playlists};
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Layout {
    /// <Ziel>/<Playlist>/001 - Künstler - Titel.mp3 + <Playlist>.m3u8 im selben Ordner
    Numbered,
    /// <Ziel>/<Playlist>/Künstler - Titel.mp3 (Reihenfolge nur über die M3U8)
    Flat,
    /// <Ziel>/Musik/Künstler/Album/… gemeinsam für alle Playlisten, M3U8 in <Ziel>
    Structured,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExportReport {
    pub copied: usize,
    pub skipped: usize,
    pub bytes: u64,
    pub playlists: Vec<String>,
    pub errors: Vec<String>,
}

pub fn label(t: &Track) -> String {
    match (&t.artist, &t.title) {
        (Some(a), Some(ti)) => format!("{a} - {ti}"),
        (None, Some(ti)) => ti.clone(),
        _ => Path::new(&t.path)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

fn ext(t: &Track) -> String {
    Path::new(&t.path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

/// Gesamtgröße für die Anzeige vorab.
pub fn size_of(conn: &Connection, playlist_ids: &[i64]) -> Result<u64> {
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    for &p in playlist_ids {
        for t in db::tracks_by_ids(conn, &playlists::track_ids(conn, p)?)? {
            if seen.insert(t.id) {
                total += t.size.max(0) as u64;
            }
        }
    }
    Ok(total)
}

fn copy_if_needed(src: &Path, dst: &Path) -> std::io::Result<bool> {
    if let (Ok(a), Ok(b)) = (std::fs::metadata(src), std::fs::metadata(dst)) {
        if a.len() == b.len() {
            return Ok(false); // schon da (erneuter Export)
        }
    }
    if let Some(d) = dst.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::copy(src, dst)?;
    // Songtext mitnehmen
    let (ls, ld) = (crate::lyrics::sidecar(src), crate::lyrics::sidecar(dst));
    if ls.exists() {
        let _ = std::fs::copy(ls, ld);
    }
    Ok(true)
}

pub fn export(
    conn: &std::sync::Mutex<Connection>,
    playlist_ids: &[i64],
    dest: &Path,
    layout: Layout,
    cancel: &AtomicBool,
    progress: Progress,
) -> Result<ExportReport> {
    let mut report = ExportReport::default();
    // Zuerst alles einsammeln (kurze Sperre), dann kopieren.
    // Hauptplayliste: eigene Titel in ihren Ordner, Titel der Unter-Playlisten in je einen Unterordner.
    let mut jobs: Vec<(String, Vec<(Option<String>, Vec<Track>)>)> = Vec::new();
    {
        let c = conn.lock().unwrap();
        for &p in playlist_ids {
            // liegt in einer ebenfalls exportierten Hauptplayliste → kommt dort als Unterordner mit
            let parent: Option<i64> = c
                .query_row("SELECT parent_id FROM playlists WHERE id = ?1", [p], |r| r.get(0))
                .unwrap_or(None);
            if parent.is_some_and(|pp| playlist_ids.contains(&pp)) {
                continue;
            }
            let name = playlists::name(&c, p)?;
            let kids = playlists::children(&c, p)?;
            let mut segs = Vec::new();
            if kids.is_empty() {
                segs.push((None, db::tracks_by_ids(&c, &playlists::track_ids(&c, p)?)?));
            } else {
                let mut seen = std::collections::HashSet::new();
                let mut kid_segs = Vec::new();
                for k in &kids {
                    let ids: Vec<i64> = playlists::own_ids(&c, *k)?.into_iter().filter(|i| seen.insert(*i)).collect();
                    kid_segs.push((Some(playlists::name(&c, *k)?), db::tracks_by_ids(&c, &ids)?));
                }
                let own: Vec<i64> = playlists::own_ids(&c, p)?.into_iter().filter(|i| !seen.contains(i)).collect();
                segs.push((None, db::tracks_by_ids(&c, &own)?));
                segs.extend(kid_segs);
            }
            jobs.push((name, segs));
        }
    }
    let total: usize = jobs.iter().flat_map(|(_, s)| s.iter()).map(|(_, t)| t.len()).sum();
    let mut n = 0;
    for (name, segs) in jobs {
        let pl_name = paths::sanitize(&name);
        let m3u_path = match layout {
            Layout::Structured => dest.join(format!("{pl_name}.m3u8")),
            _ => dest.join(&pl_name).join(format!("{pl_name}.m3u8")),
        };
        let mut exported: Vec<Track> = Vec::new();
        let mut used = std::collections::HashSet::new();
        for (sub, tracks) in segs {
            let sub_name = sub.as_deref().map(paths::sanitize);
            let folder = match &sub_name {
                Some(s) => dest.join(&pl_name).join(s),
                None => dest.join(&pl_name),
            };
            let digits = tracks.len().to_string().len().max(2);
            let mut seg_out: Vec<Track> = Vec::new();
            for (i, t) in tracks.iter().enumerate() {
                if cancel.load(Ordering::Relaxed) {
                    report.errors.push("Abgebrochen".into());
                    return Ok(report);
                }
                n += 1;
                progress(n, total, &label(t));
                let src = Path::new(&t.path);
                let mut dst = match layout {
                    Layout::Numbered => folder.join(paths::sanitize(&format!(
                        "{:0width$} - {}.{}",
                        i + 1,
                        label(t),
                        ext(t),
                        width = digits
                    ))),
                    Layout::Flat => folder.join(paths::sanitize(&format!("{}.{}", label(t), ext(t)))),
                    Layout::Structured => {
                        let rel = crate::organize::render("{albumartist}/{album}/{disc-}{track} - {title}", t);
                        let mut p = dest.join("Musik").join(rel);
                        let f = format!("{}.{}", p.file_name().unwrap_or_default().to_string_lossy(), ext(t));
                        p.set_file_name(f);
                        p
                    }
                };
                if layout != Layout::Structured {
                    dst = paths::unique_path(&dst, &|p| used.contains(&p.to_string_lossy().to_lowercase()));
                    used.insert(dst.to_string_lossy().to_lowercase());
                }
                match copy_if_needed(src, &dst) {
                    Ok(true) => {
                        report.copied += 1;
                        report.bytes += t.size.max(0) as u64;
                    }
                    Ok(false) => report.skipped += 1,
                    Err(e) => {
                        report.errors.push(format!("{}: {e}", t.path));
                        continue;
                    }
                }
                let mut copy = t.clone();
                copy.path = dst.to_string_lossy().to_string();
                seg_out.push(copy);
            }
            // Unter-Playlist bekommt zusätzlich ihre eigene M3U8
            if let Some(s) = &sub_name {
                let m = match layout {
                    Layout::Structured => dest.join(format!("{pl_name} - {s}.m3u8")),
                    _ => folder.join(format!("{s}.m3u8")),
                };
                let dir = m.parent().map(Path::to_path_buf).unwrap_or_default();
                std::fs::create_dir_all(&dir)?;
                std::fs::write(&m, playlists::m3u8(&seg_out, &dir))?;
                report.playlists.push(m.to_string_lossy().to_string());
            }
            exported.extend(seg_out);
        }
        let dir = m3u_path.parent().map(Path::to_path_buf).unwrap_or_default();
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&m3u_path, playlists::m3u8(&exported, &dir))?;
        report.playlists.push(m3u_path.to_string_lossy().to_string());
    }
    Ok(report)
}

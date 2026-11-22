//! Songtexte: als .lrc-Datei neben dem Titel („Lied.mp3“ → „Lied.lrc“), mit Zeitmarken für die
//! mitlaufende Anzeige. So lesen sie auch Minimal Player, Minimal Player Connected und viele andere Player.
//! Ohne .lrc wird ein im Tag gespeicherter (ungetakteter) Text angezeigt. Holen: LRCLIB (lrclib.net, frei, ohne Konto).

use anyhow::{Result, anyhow};
use lofty::file::TaggedFileExt;
use lofty::tag::ItemKey;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;

const UA: &str = "MinimalLibrary/1.0 ( apps@silentapps-minimal-labs.com )";

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Lyrics {
    pub text: String,
    /// Mit Zeitmarken ([mm:ss.xx])
    pub synced: bool,
    /// "lrc" (Datei neben dem Titel) oder "tag" (im Titel gespeichert, nur lesen)
    pub source: String,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct Fetched {
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
}

/// „…/Lied.mp3“ → „…/Lied.lrc“
pub fn sidecar(audio: &Path) -> PathBuf {
    audio.with_extension("lrc")
}

/// Text hat Zeitmarken?
pub fn is_synced(text: &str) -> bool {
    text.lines().any(|l| {
        let l = l.trim_start();
        l.starts_with('[')
            && l[1..].chars().next().is_some_and(|c| c.is_ascii_digit())
            && l.contains(':')
    })
}

pub fn read(audio: &Path) -> Option<Lyrics> {
    if let Ok(b) = std::fs::read(sidecar(audio)) {
        let text = String::from_utf8_lossy(&b)
            .trim_start_matches('\u{feff}')
            .replace("\r\n", "\n");
        if !text.trim().is_empty() {
            return Some(Lyrics {
                synced: is_synced(&text),
                text,
                source: "lrc".into(),
            });
        }
    }
    let tf = lofty::read_from_path(audio).ok()?;
    for tag in tf.primary_tag().into_iter().chain(tf.tags().iter()) {
        if let Some(t) = tag
            .get_string(ItemKey::Lyrics)
            .or_else(|| tag.get_string(ItemKey::UnsyncLyrics))
        {
            if !t.trim().is_empty() {
                let text = t.replace("\r\n", "\n");
                return Some(Lyrics {
                    synced: is_synced(&text),
                    text,
                    source: "tag".into(),
                });
            }
        }
    }
    None
}

/// Als .lrc speichern (leer = löschen)
pub fn save(audio: &Path, text: &str) -> Result<()> {
    let f = sidecar(audio);
    let t = text.replace("\r\n", "\n");
    if t.trim().is_empty() {
        let _ = std::fs::remove_file(&f);
        return Ok(());
    }
    std::fs::write(&f, t.trim_end().to_string() + "\n")?;
    Ok(())
}

/// Beim Verschieben/Umbenennen eines Titels die .lrc mitnehmen
pub fn move_with(from: &Path, to: &Path) {
    let (a, b) = (sidecar(from), sidecar(to));
    if a.exists() && a != b && !b.exists() {
        let _ = crate::paths::move_file(&a, &b);
    }
}

/// Beim Kopieren eines Titels die .lrc mitkopieren
pub fn copy_with(from: &Path, to: &Path) {
    let (a, b) = (sidecar(from), sidecar(to));
    if a.exists() && !b.exists() {
        let _ = std::fs::copy(&a, &b);
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(UA)
        .http_status_as_error(false)
        .build()
        .into()
}

fn parse(v: &serde_json::Value) -> Fetched {
    let s = |k: &str| {
        v[k].as_str()
            .map(str::to_string)
            .filter(|x| !x.trim().is_empty())
    };
    Fetched {
        synced: s("syncedLyrics"),
        plain: s("plainLyrics"),
        instrumental: v["instrumental"].as_bool().unwrap_or(false),
    }
}

/// Bei LRCLIB suchen: erst genau (Interpret, Titel, Album, Länge), sonst Suche mit passender Länge
pub fn fetch(artist: &str, title: &str, album: &str, duration: f64) -> Result<Option<Fetched>> {
    if title.trim().is_empty() {
        return Err(anyhow!(
            "Kein Titel im Tag – bitte erst Titel/Interpret eintragen"
        ));
    }
    let a = agent();
    let mut req = a
        .get("https://lrclib.net/api/get")
        .query("track_name", title)
        .query("artist_name", artist);
    if !album.is_empty() {
        req = req.query("album_name", album);
    }
    if duration > 0.0 {
        req = req.query("duration", format!("{}", duration.round() as i64));
    }
    let mut r = req
        .call()
        .map_err(|e| anyhow!("LRCLIB nicht erreichbar: {e}"))?;
    if r.status().as_u16() == 200 {
        let v: serde_json::Value = r.body_mut().read_json()?;
        let f = parse(&v);
        if f.synced.is_some() || f.plain.is_some() || f.instrumental {
            return Ok(Some(f));
        }
    }
    // Suche: beste Länge, synchronisierte bevorzugt
    let mut r = a
        .get("https://lrclib.net/api/search")
        .query("track_name", title)
        .query("artist_name", artist)
        .call()
        .map_err(|e| anyhow!("LRCLIB nicht erreichbar: {e}"))?;
    if r.status().as_u16() != 200 {
        return Ok(None);
    }
    let list: Vec<serde_json::Value> = r.body_mut().read_json().unwrap_or_default();
    let best = list
        .iter()
        .filter(|v| {
            duration <= 0.0 || (v["duration"].as_f64().unwrap_or(0.0) - duration).abs() < 4.0
        })
        .max_by_key(|v| {
            (
                v["syncedLyrics"].is_string() as i32,
                -((v["duration"].as_f64().unwrap_or(0.0) - duration).abs() * 10.0) as i64,
            )
        });
    Ok(best
        .map(parse)
        .filter(|f| f.synced.is_some() || f.plain.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_sync() {
        assert!(is_synced("[ar:x]\n[00:12.30]Hallo"));
        assert!(!is_synced("Hallo\nWelt"));
        assert!(!is_synced("[ar:Band]\nHallo"));
    }
    #[test]
    fn sidecar_roundtrip() {
        let d = std::env::temp_dir().join(format!("mllrc{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("Lied.mp3");
        std::fs::write(&a, b"x").unwrap();
        save(&a, "[00:01.00]Eins\n[00:02.50]Zwei").unwrap();
        let l = read(&a).unwrap();
        assert!(l.synced && l.source == "lrc" && l.text.contains("Zwei"));
        let b = d.join("Neu.mp3");
        std::fs::rename(&a, &b).unwrap();
        move_with(&a, &b);
        assert!(d.join("Neu.lrc").exists());
        save(&b, "  ").unwrap();
        assert!(!d.join("Neu.lrc").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}

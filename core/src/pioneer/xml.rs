//! rekordbox-XML („rekordbox xml“) als Rückfallebene: Titel mit Beatgrid (TEMPO) und Cues
//! (POSITION_MARK) sowie Playlisten. In rekordbox unter Einstellungen → Erweitert → rekordbox xml einbinden.
//! Aufbau nach pyrekordbox/rbxml.py (MIT, © Dylan Jones).

use super::{anlz, cues, grid};
use crate::db::{self, Track};
use crate::playlists;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashSet;
use std::fmt::Write;
use std::path::Path;

fn esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' => {}
            c => o.push(c),
        }
    }
    o
}

/// Dateipfad als „file://localhost/…“-Adresse (Windows: C:/…, macOS: /Users/… → file://localhost/Users/…)
pub fn location(path: &str) -> String {
    let p = path.replace('\\', "/");
    let p = p.trim_start_matches('/');
    let mut o = String::from("file://localhost/");
    for b in p.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b':' => o.push(b as char),
            _ => {
                let _ = write!(o, "%{b:02X}");
            }
        }
    }
    o
}

fn kind(path: &str) -> &'static str {
    match Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref() {
        Some("mp3") => "MP3 File",
        Some("m4a" | "aac" | "mp4") => "M4A File",
        Some("flac") => "FLAC File",
        Some("wav") => "WAV File",
        Some("aif" | "aiff") => "AIFF File",
        _ => "Audio File",
    }
}

fn date(unix: i64) -> String {
    super::onelibrary::timestamp(unix)[..10].to_string()
}

/// XML für die gewählten Playlisten (bzw. alle Titel, wenn keine gewählt)
pub fn build(c: &Connection, playlist_ids: &[i64], version: &str) -> Result<String> {
    let mut lists = Vec::new();
    let mut order = Vec::new();
    let mut seen = HashSet::new();
    for &p in playlist_ids {
        let ids = playlists::track_ids(c, p)?;
        for id in &ids {
            if seen.insert(*id) {
                order.push(*id);
            }
        }
        lists.push((playlists::name(c, p)?, ids));
    }
    let tracks: Vec<Track> = if playlist_ids.is_empty() { db::all_tracks(c)? } else { db::tracks_by_ids(c, &order)? };
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<DJ_PLAYLISTS Version=\"1.0.0\">\n");
    let _ = writeln!(x, "  <PRODUCT Name=\"Minimal Library\" Version=\"{}\" Company=\"SilentApps Minimal Labs\"/>", esc(version));
    let _ = writeln!(x, "  <COLLECTION Entries=\"{}\">", tracks.len());
    for t in &tracks {
        let name = t.title.clone().unwrap_or_else(|| Path::new(&t.path).file_stem().unwrap_or_default().to_string_lossy().to_string());
        let g = grid(c, t.id)?;
        let bpm = g.map(|g| g.bpm).or(t.bpm).unwrap_or(0.0);
        let _ = write!(
            x,
            "    <TRACK TrackID=\"{}\" Name=\"{}\" Artist=\"{}\" Composer=\"\" Album=\"{}\" Grouping=\"\" Genre=\"{}\" Kind=\"{}\" Size=\"{}\" TotalTime=\"{}\" DiscNumber=\"{}\" TrackNumber=\"{}\" Year=\"{}\" AverageBpm=\"{:.2}\" DateAdded=\"{}\" BitRate=\"{}\" SampleRate=\"{}\" Comments=\"{}\" PlayCount=\"0\" Rating=\"0\" Location=\"{}\" Remixer=\"\" Tonality=\"{}\" Label=\"\" Mix=\"\"",
            t.id,
            esc(&name),
            esc(t.artist.as_deref().unwrap_or("")),
            esc(t.album.as_deref().unwrap_or("")),
            esc(t.genre.as_deref().unwrap_or("")),
            kind(&t.path),
            t.size.max(0),
            t.duration.round() as i64,
            t.disc_no.unwrap_or(0),
            t.track_no.unwrap_or(0),
            esc(t.year.as_deref().map(|y| y.get(0..4).unwrap_or(y)).unwrap_or("")),
            bpm,
            date(t.added),
            t.bitrate.unwrap_or(0),
            t.sample_rate.unwrap_or(0),
            esc(t.comment.as_deref().unwrap_or("")),
            location(&t.path),
            esc(t.key.as_deref().unwrap_or("")),
        );
        let cs = cues(c, t.id)?;
        if g.is_none() && cs.is_empty() {
            x.push_str("/>\n");
            continue;
        }
        x.push_str(">\n");
        if let Some(g) = g {
            // Konstantes Tempo: ein TEMPO-Eintrag am ersten Schlag, Battito = Taktposition
            if let Some((ms, beat)) = g.beats(t.duration * 1000.0).first() {
                let _ = writeln!(x, "      <TEMPO Inizio=\"{:.3}\" Bpm=\"{:.2}\" Metro=\"4/4\" Battito=\"{}\"/>", ms / 1000.0, g.bpm, beat);
            }
        }
        for q in &cs {
            let typ = if q.end_ms.is_some() { 4 } else { 0 };
            let num: i32 = if q.kind == 0 { -1 } else { q.kind as i32 - 1 };
            let _ = write!(x, "      <POSITION_MARK Name=\"{}\" Type=\"{typ}\" Start=\"{:.3}\"", esc(q.comment.as_deref().unwrap_or("")), q.start_ms / 1000.0);
            if let Some(e) = q.end_ms {
                let _ = write!(x, " End=\"{:.3}\"", e / 1000.0);
            }
            let _ = write!(x, " Num=\"{num}\"");
            if q.kind > 0 {
                let (r, gg, b) = anlz::color_rgb(q.color.unwrap_or(0));
                if q.color.unwrap_or(0) > 0 {
                    let _ = write!(x, " Red=\"{r}\" Green=\"{gg}\" Blue=\"{b}\"");
                }
            }
            x.push_str("/>\n");
        }
        x.push_str("    </TRACK>\n");
    }
    x.push_str("  </COLLECTION>\n  <PLAYLISTS>\n");
    let _ = writeln!(x, "    <NODE Type=\"0\" Name=\"ROOT\" Count=\"{}\">", lists.len());
    for (name, ids) in &lists {
        let _ = writeln!(x, "      <NODE Name=\"{}\" Type=\"1\" KeyType=\"0\" Entries=\"{}\">", esc(name), ids.len());
        for id in ids {
            let _ = writeln!(x, "        <TRACK Key=\"{id}\"/>");
        }
        x.push_str("      </NODE>\n");
    }
    x.push_str("    </NODE>\n  </PLAYLISTS>\n</DJ_PLAYLISTS>\n");
    Ok(x)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations() {
        assert_eq!(location(r"C:\Musik\Demo Track ä.mp3"), "file://localhost/C:/Musik/Demo%20Track%20%C3%A4.mp3");
        assert_eq!(location("/Users/phil/Music/a&b.mp3"), "file://localhost/Users/phil/Music/a%26b.mp3");
    }

    #[test]
    fn builds_valid_xml() {
        let c = crate::db::open(&std::env::temp_dir().join(format!("mlxml{}.db", std::process::id()))).unwrap();
        c.execute("INSERT INTO tracks(id, path, size, title, artist, duration) VALUES(5, '/m/a.mp3', 10, 'A & \"B\"', 'X', 200.0)", []).unwrap();
        super::super::set_grid(&c, 5, 125.0, 480.0, "t").unwrap();
        super::super::set_cue(&c, 5, &super::super::Cue { id: 0, kind: 2, start_ms: 1500.0, end_ms: None, color: Some(1), comment: Some("Drop".into()) }).unwrap();
        let p = playlists::create(&c, "Set <1>", None).unwrap();
        let mut c = c;
        playlists::set_items(&mut c, p, &[5]).unwrap();
        let x = build(&c, &[p], "1.0").unwrap();
        let doc = roxmltree::Document::parse(&x).unwrap();
        let t = doc.descendants().find(|n| n.has_tag_name("TRACK") && n.attribute("TrackID").is_some()).unwrap();
        assert_eq!(t.attribute("Name"), Some("A & \"B\""));
        let tempo = t.children().find(|n| n.has_tag_name("TEMPO")).unwrap();
        assert_eq!(tempo.attribute("Bpm"), Some("125.00"));
        assert_eq!(tempo.attribute("Inizio"), Some("0.000"));
        let pm = t.children().find(|n| n.has_tag_name("POSITION_MARK")).unwrap();
        assert_eq!(pm.attribute("Num"), Some("1"));
        assert!(x.contains("Name=\"Set &lt;1&gt;\""));
    }
}

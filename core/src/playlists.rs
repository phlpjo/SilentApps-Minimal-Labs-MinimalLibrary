use crate::db::{self, Track};
use crate::{now, paths};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    /// title, artist, album, albumartist, genre, year, bpm, key, duration, added, path, comment
    pub field: String,
    /// contains, notContains, is, isNot, startsWith, gt, lt, between, inLastDays
    pub op: String,
    pub value: String,
    #[serde(default)]
    pub value2: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SmartRules {
    /// true = alle Regeln müssen passen, false = mindestens eine
    pub all: bool,
    pub rules: Vec<Rule>,
    /// title, artist, album, bpm, key, added, random, duration
    #[serde(default)]
    pub sort: String,
    #[serde(default)]
    pub desc: bool,
    #[serde(default)]
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Playlist {
    pub id: i64,
    pub name: String,
    pub rules: Option<SmartRules>,
    pub count: usize,
    pub duration: f64,
}

fn text(t: &Track, f: &str) -> Option<String> {
    match f {
        "title" => t.title.clone(),
        "artist" => t.artist.clone(),
        "album" => t.album.clone(),
        "albumartist" => t.album_artist.clone().or_else(|| t.artist.clone()),
        "genre" => t.genre.clone(),
        "key" => t.key.clone(),
        "comment" => t.comment.clone(),
        "path" => Some(t.path.clone()),
        "year" => t.year.clone(),
        _ => None,
    }
}

fn number(t: &Track, f: &str) -> Option<f64> {
    match f {
        "bpm" => t.bpm,
        "duration" => Some(t.duration),
        "year" => t.year.as_deref().and_then(|y| y.get(..4)).and_then(|y| y.parse().ok()),
        "added" => Some(t.added as f64),
        "bitrate" => t.bitrate.map(f64::from),
        _ => None,
    }
}

fn parse_num(s: &str) -> Option<f64> {
    // Dauer als "3:30" erlauben
    if let Some((m, s2)) = s.split_once(':') {
        return Some(m.trim().parse::<f64>().ok()? * 60.0 + s2.trim().parse::<f64>().ok()?);
    }
    s.trim().replace(',', ".").parse().ok()
}

pub fn rule_matches(t: &Track, r: &Rule) -> bool {
    let v = r.value.trim().to_lowercase();
    match r.op.as_str() {
        "contains" | "notContains" | "is" | "isNot" | "startsWith" => {
            let s = text(t, &r.field).unwrap_or_default().to_lowercase();
            match r.op.as_str() {
                "contains" => s.contains(&v),
                "notContains" => !s.contains(&v),
                "is" => s == v,
                "isNot" => s != v,
                _ => s.starts_with(&v),
            }
        }
        "gt" | "lt" | "between" => {
            let (Some(n), Some(a)) = (number(t, &r.field), parse_num(&r.value)) else { return false };
            match r.op.as_str() {
                "gt" => n > a,
                "lt" => n < a,
                _ => parse_num(&r.value2).is_some_and(|b| n >= a.min(b) && n <= a.max(b)),
            }
        }
        "inLastDays" => {
            let days = parse_num(&r.value).unwrap_or(0.0);
            (t.added as f64) >= now() as f64 - days * 86400.0
        }
        _ => false,
    }
}

pub fn evaluate(all_tracks: &[Track], rules: &SmartRules) -> Vec<i64> {
    let mut hits: Vec<&Track> = all_tracks
        .iter()
        .filter(|t| {
            if rules.rules.is_empty() {
                return true;
            }
            if rules.all {
                rules.rules.iter().all(|r| rule_matches(t, r))
            } else {
                rules.rules.iter().any(|r| rule_matches(t, r))
            }
        })
        .collect();
    let lc = |o: &Option<String>| o.clone().unwrap_or_default().to_lowercase();
    match rules.sort.as_str() {
        "title" => hits.sort_by_key(|t| lc(&t.title)),
        "artist" => hits.sort_by_key(|t| (lc(&t.artist), lc(&t.album), t.track_no)),
        "album" => hits.sort_by_key(|t| (lc(&t.album), t.disc_no, t.track_no)),
        "bpm" => hits.sort_by(|a, b| a.bpm.unwrap_or(0.0).total_cmp(&b.bpm.unwrap_or(0.0))),
        "key" => hits.sort_by_key(|t| lc(&t.key)),
        "added" => hits.sort_by_key(|t| t.added),
        "duration" => hits.sort_by(|a, b| a.duration.total_cmp(&b.duration)),
        "random" => {
            // deterministisch-zufällig pro Tag
            let seed = (now() / 86400) as u64;
            hits.sort_by_key(|t| (t.id as u64).wrapping_mul(6364136223846793005).wrapping_add(seed).rotate_left(17));
        }
        _ => {}
    }
    if rules.desc && rules.sort != "random" {
        hits.reverse();
    }
    let ids: Vec<i64> = hits.iter().map(|t| t.id).collect();
    match rules.limit {
        Some(l) if l > 0 => ids.into_iter().take(l).collect(),
        _ => ids,
    }
}

pub fn list(conn: &Connection) -> Result<Vec<Playlist>> {
    let all = db::all_tracks(conn)?;
    let dur: std::collections::HashMap<i64, f64> = all.iter().map(|t| (t.id, t.duration)).collect();
    let mut st = conn.prepare("SELECT id, name, rules FROM playlists ORDER BY name COLLATE NOCASE")?;
    let rows: Vec<(i64, String, Option<String>)> =
        st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    let mut out = Vec::new();
    for (id, name, rules) in rows {
        let rules: Option<SmartRules> = rules.and_then(|r| serde_json::from_str(&r).ok());
        let ids = match &rules {
            Some(r) => evaluate(&all, r),
            None => item_ids(conn, id)?,
        };
        out.push(Playlist {
            id,
            name,
            rules,
            count: ids.len(),
            duration: ids.iter().filter_map(|i| dur.get(i)).sum(),
        });
    }
    Ok(out)
}

fn item_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut st = conn.prepare("SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY pos")?;
    Ok(st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Titel-IDs einer Playlist (bei smarten Playlisten frisch ausgewertet).
pub fn track_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let rules: Option<String> = conn
        .query_row("SELECT rules FROM playlists WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .flatten();
    match rules.and_then(|r| serde_json::from_str::<SmartRules>(&r).ok()) {
        Some(r) => Ok(evaluate(&db::all_tracks(conn)?, &r)),
        None => item_ids(conn, id),
    }
}

pub fn name(conn: &Connection, id: i64) -> Result<String> {
    Ok(conn.query_row("SELECT name FROM playlists WHERE id = ?1", [id], |r| r.get(0))?)
}

pub fn create(conn: &Connection, name: &str, rules: Option<&SmartRules>) -> Result<i64> {
    let rules = rules.map(serde_json::to_string).transpose()?;
    conn.execute("INSERT INTO playlists(name, rules, created) VALUES(?1, ?2, ?3)", params![name, rules, now()])?;
    Ok(conn.last_insert_rowid())
}

pub fn update(conn: &Connection, id: i64, name: &str, rules: Option<&SmartRules>) -> Result<()> {
    let rules = rules.map(serde_json::to_string).transpose()?;
    conn.execute("UPDATE playlists SET name = ?2, rules = ?3 WHERE id = ?1", params![id, name, rules])?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
    Ok(())
}

pub fn set_items(conn: &mut Connection, id: i64, ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
    {
        let mut st = tx.prepare("INSERT INTO playlist_items(playlist_id, pos, track_id) VALUES(?1, ?2, ?3)")?;
        for (pos, t) in ids.iter().enumerate() {
            st.execute(params![id, pos as i64, t])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn add_items(conn: &mut Connection, id: i64, ids: &[i64]) -> Result<()> {
    let mut cur = item_ids(conn, id)?;
    cur.extend(ids.iter().copied().filter(|i| !cur.contains(i)).collect::<Vec<_>>());
    set_items(conn, id, &cur)
}

/// M3U8-Text, Pfade relativ zu `base_dir` (Trenner "/").
pub fn m3u8(tracks: &[Track], base_dir: &Path) -> String {
    let mut s = String::from("#EXTM3U\n");
    for t in tracks {
        let label = match (&t.artist, &t.title) {
            (Some(a), Some(ti)) => format!("{a} - {ti}"),
            (None, Some(ti)) => ti.clone(),
            _ => Path::new(&t.path).file_stem().map(|x| x.to_string_lossy().to_string()).unwrap_or_default(),
        };
        s.push_str(&format!("#EXTINF:{},{}\n", t.duration.round() as i64, label.replace('\n', " ")));
        s.push_str(&paths::relative_to(base_dir, Path::new(&t.path)));
        s.push('\n');
    }
    s
}

pub fn write_m3u8(conn: &Connection, id: i64, file: &Path) -> Result<()> {
    let ids = track_ids(conn, id)?;
    let tracks = db::tracks_by_ids(conn, &ids)?;
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir)?;
    std::fs::write(file, m3u8(&tracks, &dir))?;
    Ok(())
}

/// Alle Playlisten als "<Name>.m3u8" in den Playlist-Ordner schreiben.
pub fn sync_folder(conn: &Connection, dir: &Path) -> Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for p in list(conn)? {
        let file = dir.join(format!("{}.m3u8", paths::sanitize(&p.name)));
        write_m3u8(conn, p.id, &file)?;
        written.push(file);
    }
    Ok(written)
}

/// M3U/M3U8 einlesen → Pfade (absolut aufgelöst).
pub fn read_m3u(file: &Path) -> Result<Vec<PathBuf>> {
    let bytes = std::fs::read(file)?;
    let text = match String::from_utf8(bytes.clone()) {
        Ok(s) => s,
        // alte .m3u sind oft Windows-1252 → als Latin-1 lesen
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    };
    let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
    Ok(text
        .lines()
        .map(|l| l.trim().trim_start_matches('\u{feff}'))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| {
            let l = l.strip_prefix("file:///").unwrap_or(l);
            let p = PathBuf::from(if cfg!(windows) { l.replace('/', "\\") } else { l.to_string() });
            if p.is_absolute() { p } else { dir.join(p) }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smart() {
        let t = |id, bpm: f64, genre: &str| Track { id, bpm: Some(bpm), genre: Some(genre.into()), ..Default::default() };
        let all = vec![t(1, 124.0, "House"), t(2, 90.0, "Hip-Hop"), t(3, 128.0, "Deep House")];
        let r = SmartRules {
            all: true,
            rules: vec![
                Rule { field: "genre".into(), op: "contains".into(), value: "house".into(), value2: String::new() },
                Rule { field: "bpm".into(), op: "between".into(), value: "120".into(), value2: "126".into() },
            ],
            sort: "bpm".into(),
            desc: false,
            limit: None,
        };
        assert_eq!(evaluate(&all, &r), vec![1]);
    }
}

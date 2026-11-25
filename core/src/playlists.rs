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
    /// Hauptplayliste, in der diese liegt (eine Ebene)
    pub parent: Option<i64>,
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
        "quality" => Some(match (t.upscaled, t.lossless, t.real_kbps) {
            (true, true, Some(k)) => format!("hochgerechnet aus MP3 ≈ {k} kbps"),
            (true, _, Some(k)) => format!("hochgerechnet ≈ {k} kbps"),
            (_, true, _) => "verlustfrei".into(),
            (_, _, Some(k)) => format!("≈ {k} kbps"),
            _ => "unbekannt".into(),
        }),
        _ => None,
    }
}

fn number(t: &Track, f: &str) -> Option<f64> {
    match f {
        "bpm" => t.bpm,
        "duration" => Some(t.duration),
        "year" => t
            .year
            .as_deref()
            .and_then(|y| y.get(..4))
            .and_then(|y| y.parse().ok()),
        "added" => Some(t.added as f64),
        "bitrate" => t.bitrate.map(f64::from),
        "realkbps" => t.real_kbps.map(f64::from),
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
    let v = amp(&r.value.trim().to_lowercase());
    match r.op.as_str() {
        "contains" | "notContains" | "is" | "isNot" | "startsWith" => {
            let s = amp(&text(t, &r.field).unwrap_or_default().to_lowercase());
            match r.op.as_str() {
                "contains" => s.contains(&v),
                "notContains" => !s.contains(&v),
                "is" => s == v,
                "isNot" => s != v,
                _ => s.starts_with(&v),
            }
        }
        "gt" | "lt" | "between" => {
            let (Some(n), Some(a)) = (number(t, &r.field), parse_num(&r.value)) else {
                return false;
            };
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
            hits.sort_by_key(|t| {
                (t.id as u64)
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(seed)
                    .rotate_left(17)
            });
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
    let mut st = conn.prepare(
        "SELECT id, name, rules, parent_id FROM playlists ORDER BY pos, name COLLATE NOCASE",
    )?;
    let rows: Vec<(i64, String, Option<String>, Option<i64>)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
        .collect::<rusqlite::Result<_>>()?;
    // Eigene Titel je Playlist
    let mut own: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    let mut parsed = Vec::new();
    for (id, name, rules, parent) in rows {
        let rules: Option<SmartRules> = rules.and_then(|r| serde_json::from_str(&r).ok());
        let ids = match &rules {
            Some(r) => evaluate(&all, r),
            None => item_ids(conn, id)?,
        };
        own.insert(id, ids);
        parsed.push((id, name, rules, parent));
    }
    let valid: std::collections::HashSet<i64> = parsed.iter().filter(|p| p.3.is_none()).map(|p| p.0).collect();
    let mut out = Vec::new();
    for (id, name, rules, parent) in &parsed {
        // Nur eine Ebene: Eltern muss es geben und selbst oben liegen
        let parent = parent.filter(|p| valid.contains(p) && p != id);
        let kids: Vec<i64> = if parent.is_none() {
            parsed.iter().filter(|c| c.3 == Some(*id) && c.0 != *id).map(|c| c.0).collect()
        } else {
            Vec::new()
        };
        let ids = combine(own.get(id).cloned().unwrap_or_default(), kids.iter().map(|k| own.get(k).cloned().unwrap_or_default()).collect());
        out.push(Playlist {
            id: *id,
            name: name.clone(),
            rules: rules.clone(),
            count: ids.len(),
            duration: ids.iter().filter_map(|i| dur.get(i)).sum(),
            parent,
        });
    }
    Ok(out)
}

/// Eigene Titel + Titel der Unter-Playlisten (jeder Titel einmal; liegt er in einer Unter-Playlist, zählt er dort)
fn combine(own: Vec<i64>, kids: Vec<Vec<i64>>) -> Vec<i64> {
    let in_kids: std::collections::HashSet<i64> = kids.iter().flatten().copied().collect();
    let mut seen = std::collections::HashSet::new();
    own.into_iter()
        .filter(|i| !in_kids.contains(i))
        .chain(kids.into_iter().flatten())
        .filter(|i| seen.insert(*i))
        .collect()
}

/// Unter-Playlisten einer Hauptplayliste (in Reihenfolge); leer, wenn sie selbst in einer liegt
pub fn children(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let parent: Option<i64> = conn
        .query_row("SELECT parent_id FROM playlists WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .flatten();
    if parent.is_some() {
        return Ok(Vec::new());
    }
    let mut st = conn.prepare(
        "SELECT id FROM playlists WHERE parent_id = ?1 AND id != ?1 ORDER BY pos, name COLLATE NOCASE",
    )?;
    Ok(st.query_map([id], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?)
}

/// Eigene Titel (ohne Unter-Playlisten; smarte frisch ausgewertet)
pub fn own_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let rules: Option<String> = conn
        .query_row("SELECT rules FROM playlists WHERE id = ?1", [id], |r| r.get(0))
        .optional()?
        .flatten();
    match rules.and_then(|r| serde_json::from_str::<SmartRules>(&r).ok()) {
        Some(r) => Ok(evaluate(&db::all_tracks(conn)?, &r)),
        None => item_ids(conn, id),
    }
}

fn is_smart(conn: &Connection, id: i64) -> bool {
    conn.query_row("SELECT rules IS NOT NULL FROM playlists WHERE id = ?1", [id], |r| r.get(0))
        .unwrap_or(false)
}

/// In eine Hauptplayliste legen (`None` = wieder nach oben). Nur eine Ebene.
pub fn set_parent(conn: &Connection, id: i64, parent: Option<i64>) -> Result<()> {
    if let Some(p) = parent {
        if p == id {
            anyhow::bail!("Eine Playlist kann nicht in sich selbst liegen");
        }
        let pp: Option<Option<i64>> = conn
            .query_row("SELECT parent_id FROM playlists WHERE id = ?1", [p], |r| r.get(0))
            .optional()?;
        match pp {
            None => anyhow::bail!("Playlist nicht gefunden"),
            Some(Some(_)) => anyhow::bail!("Playlisten lassen sich nur eine Ebene tief ineinander legen"),
            _ => {}
        }
        let has_kids: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM playlists WHERE parent_id = ?1)",
            [id],
            |r| r.get(0),
        )?;
        if has_kids {
            anyhow::bail!("Diese Playlist enthält selbst Playlisten – nur eine Ebene möglich");
        }
    }
    conn.execute("UPDATE playlists SET parent_id = ?2 WHERE id = ?1", params![id, parent])?;
    Ok(())
}

fn item_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let mut st =
        conn.prepare("SELECT track_id FROM playlist_items WHERE playlist_id = ?1 ORDER BY pos")?;
    Ok(st
        .query_map([id], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Titel-IDs einer Playlist (bei smarten Playlisten frisch ausgewertet; Hauptplayliste mit allen Unter-Playlisten).
pub fn track_ids(conn: &Connection, id: i64) -> Result<Vec<i64>> {
    let own = own_ids(conn, id)?;
    let kids = children(conn, id)?;
    if kids.is_empty() {
        return Ok(own);
    }
    let lists = kids.iter().map(|k| own_ids(conn, *k)).collect::<Result<Vec<_>>>()?;
    Ok(combine(own, lists))
}

pub fn name(conn: &Connection, id: i64) -> Result<String> {
    Ok(
        conn.query_row("SELECT name FROM playlists WHERE id = ?1", [id], |r| {
            r.get(0)
        })?,
    )
}

/// Gleicher Name schon vergeben → „Name (2)“, „Name (3)“ …
pub fn unique_name(conn: &Connection, name: &str, except: Option<i64>) -> Result<String> {
    let name = name.trim();
    let taken = |n: &str| -> Result<bool> {
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM playlists WHERE name = ?1 COLLATE NOCASE AND id != ?2",
            params![n, except.unwrap_or(-1)],
            |r| r.get::<_, i64>(0),
        )? > 0)
    };
    if !taken(name)? {
        return Ok(name.to_string());
    }
    // vorhandene Nummer am Ende weiterzählen
    let base = match name.rfind(" (") {
        Some(i) if name.ends_with(')') && name[i + 2..name.len() - 1].parse::<u32>().is_ok() => {
            &name[..i]
        }
        _ => name,
    };
    for n in 2.. {
        let c = format!("{base} ({n})");
        if !taken(&c)? {
            return Ok(c);
        }
    }
    unreachable!()
}

pub fn create(conn: &Connection, name: &str, rules: Option<&SmartRules>) -> Result<i64> {
    let name = &unique_name(conn, name, None)?;
    let rules = rules.map(serde_json::to_string).transpose()?;
    conn.execute(
        "INSERT INTO playlists(name, rules, created, pos) VALUES(?1, ?2, ?3, (SELECT COALESCE(MAX(pos), -1) + 1 FROM playlists))",
        params![name, rules, now()],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn update(conn: &Connection, id: i64, name: &str, rules: Option<&SmartRules>) -> Result<()> {
    let name = &unique_name(conn, name, Some(id))?;
    let rules = rules.map(serde_json::to_string).transpose()?;
    conn.execute(
        "UPDATE playlists SET name = ?2, rules = ?3 WHERE id = ?1",
        params![id, name, rules],
    )?;
    Ok(())
}

/// Playlist „Stern“ (wie in Minimal Player): gemerkte ID, bei Bedarf neu anlegen
pub fn star(conn: &Connection, create: bool) -> Result<Option<i64>> {
    let id: Option<i64> =
        crate::db::get_setting(conn, "star_playlist")?.and_then(|s| s.parse().ok());
    if let Some(id) = id {
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) FROM playlists WHERE id = ?1 AND rules IS NULL",
            [id],
            |r| r.get::<_, i64>(0),
        )? > 0;
        if exists {
            return Ok(Some(id));
        }
    }
    if !create {
        return Ok(None);
    }
    let id = create_plain(conn, "Stern")?;
    crate::db::set_setting(conn, "star_playlist", &id.to_string())?;
    Ok(Some(id))
}

fn create_plain(conn: &Connection, name: &str) -> Result<i64> {
    create(conn, name, None)
}

/// Eigene Reihenfolge der Playlisten setzen
pub fn set_order(conn: &Connection, ids: &[i64]) -> Result<()> {
    for (i, id) in ids.iter().enumerate() {
        conn.execute(
            "UPDATE playlists SET pos = ?2 WHERE id = ?1",
            params![id, i as i64],
        )?;
    }
    Ok(())
}

/// Stand einer Playlist für „Rückgängig“ sichern (gibt es sie nicht, wird „neu angelegt“ vermerkt)
pub fn backup(conn: &Connection, batch: i64, id: i64) -> Result<()> {
    let row: Option<(String, Option<String>, i64)> = conn
        .query_row(
            "SELECT name, rules, pos FROM playlists WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let (existed, name, rules, pos) = match row {
        Some((n, r, p)) => (1, n, r, p),
        None => (0, String::new(), None, 0),
    };
    let items = serde_json::to_string(&item_ids(conn, id)?)?;
    conn.execute(
        "INSERT INTO playlist_backups(batch_id, playlist_id, existed, name, rules, items, pos) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![batch, id, existed, name, rules, items, pos],
    )?;
    Ok(())
}

/// Reihenfolge aller Playlisten sichern (playlist_id 0)
pub fn backup_order(conn: &Connection, batch: i64) -> Result<()> {
    let mut st = conn.prepare("SELECT id FROM playlists ORDER BY pos, name COLLATE NOCASE")?;
    let ids: Vec<i64> = st
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    conn.execute(
        "INSERT INTO playlist_backups(batch_id, playlist_id, existed, items) VALUES(?1, 0, 1, ?2)",
        params![batch, serde_json::to_string(&ids)?],
    )?;
    Ok(())
}

pub fn delete(conn: &Connection, id: i64) -> Result<()> {
    conn.execute("DELETE FROM playlists WHERE id = ?1", [id])?;
    // Unter-Playlisten bleiben erhalten und rücken nach oben
    conn.execute("UPDATE playlists SET parent_id = NULL WHERE parent_id = ?1", [id])?;
    Ok(())
}

/// Titel einer Playlist setzen. Bei einer Hauptplayliste ist `ids` die ganze sichtbare Liste:
/// Titel aus Unter-Playlisten bleiben dort (in neuer Reihenfolge, entfernte fliegen dort raus), der Rest gehört ihr selbst.
pub fn set_items(conn: &mut Connection, id: i64, ids: &[i64]) -> Result<()> {
    let kids = children(conn, id)?;
    if kids.is_empty() {
        return set_own(conn, id, ids);
    }
    let mut taken = std::collections::HashSet::new();
    for k in &kids {
        let mine: std::collections::HashSet<i64> = own_ids(conn, *k)?.into_iter().collect();
        taken.extend(mine.iter().copied());
        if is_smart(conn, *k) {
            continue;
        }
        let next: Vec<i64> = ids.iter().copied().filter(|i| mine.contains(i)).collect();
        set_own(conn, *k, &next)?;
    }
    if is_smart(conn, id) {
        return Ok(());
    }
    let own: Vec<i64> = ids.iter().copied().filter(|i| !taken.contains(i)).collect();
    set_own(conn, id, &own)
}

/// Gehören die Titel zu Unter-Playlisten dieser Hauptplayliste? (für „Rückgängig“ mitsichern)
pub fn affected(conn: &Connection, id: i64) -> Vec<i64> {
    let mut v = vec![id];
    v.extend(children(conn, id).unwrap_or_default());
    v
}

fn set_own(conn: &mut Connection, id: i64, ids: &[i64]) -> Result<()> {
    let tx = conn.transaction()?;
    tx.execute("DELETE FROM playlist_items WHERE playlist_id = ?1", [id])?;
    {
        let mut st = tx
            .prepare("INSERT INTO playlist_items(playlist_id, pos, track_id) VALUES(?1, ?2, ?3)")?;
        for (pos, t) in ids.iter().enumerate() {
            st.execute(params![id, pos as i64, t])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn add_items(conn: &mut Connection, id: i64, ids: &[i64]) -> Result<()> {
    let mut cur = item_ids(conn, id)?;
    cur.extend(
        ids.iter()
            .copied()
            .filter(|i| !cur.contains(i))
            .collect::<Vec<_>>(),
    );
    set_own(conn, id, &cur)
}

/// M3U8-Text, Pfade relativ zu `base_dir` (Trenner "/").
pub fn m3u8(tracks: &[Track], base_dir: &Path) -> String {
    let mut s = String::from("#EXTM3U\n");
    for t in tracks {
        let label = match (&t.artist, &t.title) {
            (Some(a), Some(ti)) => format!("{a} - {ti}"),
            (None, Some(ti)) => ti.clone(),
            _ => Path::new(&t.path)
                .file_stem()
                .map(|x| x.to_string_lossy().to_string())
                .unwrap_or_default(),
        };
        s.push_str(&format!(
            "#EXTINF:{},{}\n",
            t.duration.round() as i64,
            label.replace('\n', " ")
        ));
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
            let p = PathBuf::from(if cfg!(windows) {
                l.replace('/', "\\")
            } else {
                l.to_string()
            });
            if p.is_absolute() { p } else { dir.join(p) }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn smart() {
        let t = |id, bpm: f64, genre: &str| Track {
            id,
            bpm: Some(bpm),
            genre: Some(genre.into()),
            ..Default::default()
        };
        let all = vec![
            t(1, 124.0, "House"),
            t(2, 90.0, "Hip-Hop"),
            t(3, 128.0, "Deep House"),
        ];
        let r = SmartRules {
            all: true,
            rules: vec![
                Rule {
                    field: "genre".into(),
                    op: "contains".into(),
                    value: "house".into(),
                    value2: String::new(),
                },
                Rule {
                    field: "bpm".into(),
                    op: "between".into(),
                    value: "120".into(),
                    value2: "126".into(),
                },
            ],
            sort: "bpm".into(),
            desc: false,
            limit: None,
        };
        assert_eq!(evaluate(&all, &r), vec![1]);
    }
}

/// "A&B", "A & B", "A  &B" gelten als gleich
fn amp(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.split('&').map(str::trim).collect::<Vec<_>>().join(" & ")
}

#[cfg(test)]
mod unique_tests {
    #[test]
    fn numbers() {
        let f = std::env::temp_dir().join(format!("mlpl{}.db", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let c = crate::db::open(&f).unwrap();
        let a = super::create(&c, "Party", None).unwrap();
        let b = super::create(&c, "Party", None).unwrap();
        let d = super::create(&c, "party", None).unwrap();
        assert_eq!(super::name(&c, b).unwrap(), "Party (2)");
        assert_eq!(super::name(&c, d).unwrap(), "party (3)");
        super::update(&c, a, "Party", None).unwrap();
        assert_eq!(super::name(&c, a).unwrap(), "Party");
    }
}

#[cfg(test)]
mod nested_tests {
    use super::*;

    #[test]
    fn parent_sees_children_and_export_sorts_into_folders() {
        let base = std::env::temp_dir().join(format!("ml-nest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("src")).unwrap();
        let mut c = crate::db::open(&base.join("t.db")).unwrap();
        for i in 1..=4 {
            let p = base.join("src").join(format!("{i}.mp3"));
            std::fs::write(&p, [0u8; 10]).unwrap();
            c.execute(
                "INSERT INTO tracks(id, path, size, mtime, added, title, artist) VALUES(?1, ?2, 10, 0, 0, ?3, 'A')",
                params![i, p.to_string_lossy(), format!("T{i}")],
            )
            .unwrap();
        }
        let main = create(&c, "Set", None).unwrap();
        let warm = create(&c, "Warm-up", None).unwrap();
        let peak = create(&c, "Peak", None).unwrap();
        add_items(&mut c, main, &[1]).unwrap();
        add_items(&mut c, warm, &[2, 3]).unwrap();
        add_items(&mut c, peak, &[4]).unwrap();
        set_order(&c, &[main, warm, peak]).unwrap();
        set_parent(&c, warm, Some(main)).unwrap();
        set_parent(&c, peak, Some(main)).unwrap();
        // nur eine Ebene
        assert!(set_parent(&c, main, Some(warm)).is_err());
        assert_eq!(track_ids(&c, main).unwrap(), vec![1, 2, 3, 4]);
        let l = list(&c).unwrap();
        assert_eq!(l.iter().find(|p| p.id == main).unwrap().count, 4);
        // In der Hauptplayliste umsortieren und entfernen → wirkt in den Unter-Playlisten
        set_items(&mut c, main, &[4, 3, 1]).unwrap();
        assert_eq!(own_ids(&c, warm).unwrap(), vec![3]);
        assert_eq!(own_ids(&c, peak).unwrap(), vec![4]);
        assert_eq!(own_ids(&c, main).unwrap(), vec![1]);
        // Export: Unter-Playlisten als Unterordner
        let m = std::sync::Mutex::new(c);
        let dest = base.join("out");
        let r = crate::export::export(&m, &[main, warm], &dest, crate::export::Layout::Numbered, &std::sync::atomic::AtomicBool::new(false), &|_, _, _| {}).unwrap();
        assert_eq!(r.copied, 3);
        assert!(dest.join("Set").join("01 - A - T1.mp3").exists());
        assert!(dest.join("Set").join("Warm-up").join("01 - A - T3.mp3").exists());
        assert!(dest.join("Set").join("Peak").join("01 - A - T4.mp3").exists());
        assert!(dest.join("Set").join("Set.m3u8").exists());
        assert!(dest.join("Set").join("Peak").join("Peak.m3u8").exists());
        // Löschen der Hauptplayliste: Unter-Playlisten bleiben und rücken nach oben
        let c = m.lock().unwrap();
        delete(&c, main).unwrap();
        assert!(list(&c).unwrap().iter().all(|p| p.parent.is_none()));
    }
}

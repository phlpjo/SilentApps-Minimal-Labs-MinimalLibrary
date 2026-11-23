use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS tracks (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    size INTEGER NOT NULL DEFAULT 0,
    mtime INTEGER NOT NULL DEFAULT 0,
    added INTEGER NOT NULL DEFAULT 0,
    title TEXT, artist TEXT, album TEXT, album_artist TEXT, genre TEXT, year TEXT,
    track_no INTEGER, disc_no INTEGER, comment TEXT,
    duration REAL NOT NULL DEFAULT 0,
    bitrate INTEGER, sample_rate INTEGER, channels INTEGER,
    has_cover INTEGER NOT NULL DEFAULT 0,
    tag_bpm REAL, tag_key TEXT,
    bpm REAL, musical_key TEXT,
    analyzed INTEGER NOT NULL DEFAULT 0,
    analysis_error TEXT,
    fingerprint BLOB,
    peaks BLOB
);
CREATE INDEX IF NOT EXISTS idx_tracks_analyzed ON tracks(analyzed);
CREATE TABLE IF NOT EXISTS playlists (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    rules TEXT,
    created INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    pos INTEGER NOT NULL,
    track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_pl_items ON playlist_items(playlist_id, pos);
CREATE TABLE IF NOT EXISTS move_batches (
    id INTEGER PRIMARY KEY,
    created INTEGER NOT NULL,
    description TEXT NOT NULL,
    count INTEGER NOT NULL DEFAULT 0,
    undone INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS moves (
    batch_id INTEGER NOT NULL REFERENCES move_batches(id) ON DELETE CASCADE,
    track_id INTEGER,
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    copied INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS dup_ignore (
    a INTEGER NOT NULL,
    b INTEGER NOT NULL,
    PRIMARY KEY (a, b)
);
CREATE TABLE IF NOT EXISTS tag_backups (
    batch_id INTEGER NOT NULL REFERENCES move_batches(id) ON DELETE CASCADE,
    track_id INTEGER NOT NULL,
    tags TEXT NOT NULL,
    cover_changed INTEGER NOT NULL DEFAULT 0,
    cover BLOB
);
CREATE TABLE IF NOT EXISTS trash_backups (
    batch_id INTEGER NOT NULL REFERENCES move_batches(id) ON DELETE CASCADE,
    path TEXT NOT NULL,
    is_dir INTEGER NOT NULL DEFAULT 0,
    playlists TEXT NOT NULL DEFAULT '[]'
);
"#;

/// Spalten nachrüsten, die in älteren Datenbanken fehlen.
fn migrate(conn: &Connection) -> Result<()> {
    let has = |table: &str, col: &str| -> bool {
        conn.prepare(&format!("SELECT {col} FROM {table} LIMIT 0"))
            .is_ok()
    };
    // Pioneer-Export: Beatgrid, Pioneer-Wellenform und Cues (nur genutzt, wenn eingeschaltet)
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS pioneer_analysis (
            track_id INTEGER PRIMARY KEY,
            bpm REAL NOT NULL DEFAULT 0,
            downbeat_ms REAL NOT NULL DEFAULT 0,
            grid_edited INTEGER NOT NULL DEFAULT 0,
            frames BLOB,
            updated INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS cues (
            id INTEGER PRIMARY KEY,
            track_id INTEGER NOT NULL,
            kind INTEGER NOT NULL,
            start_ms REAL NOT NULL,
            end_ms REAL,
            color INTEGER,
            comment TEXT
        );
        CREATE INDEX IF NOT EXISTS cues_track ON cues(track_id);
        CREATE TABLE IF NOT EXISTS grid_backups (batch_id INTEGER NOT NULL, track_id INTEGER NOT NULL, grid TEXT, cues TEXT NOT NULL);
        CREATE TRIGGER IF NOT EXISTS tracks_pioneer_cleanup AFTER DELETE ON tracks BEGIN
            DELETE FROM pioneer_analysis WHERE track_id = old.id;
            DELETE FROM cues WHERE track_id = old.id;
        END;",
    )?;
    // Echte Qualität: Grenzfrequenz aus dem Spektrum (erkennt hochgerechnete Dateien)
    if !has("tracks", "cutoff") {
        conn.execute_batch("ALTER TABLE tracks ADD COLUMN cutoff INTEGER")?;
    }
    // Messverfahren verbessert (Kante statt Rauschgrenze) → einmal neu messen
    const CUTOFF_ALGO: &str = "2";
    if get_setting(conn, "cutoff_algo")?.as_deref() != Some(CUTOFF_ALGO) {
        conn.execute("UPDATE tracks SET cutoff = NULL", [])?;
        set_setting(conn, "cutoff_algo", CUTOFF_ALGO)?;
    }
    // Playlisten: eigene Reihenfolge (bisher alphabetisch → so übernehmen)
    if !has("playlists", "pos") {
        conn.execute_batch(
            "ALTER TABLE playlists ADD COLUMN pos INTEGER NOT NULL DEFAULT 0;
             UPDATE playlists SET pos = (SELECT COUNT(*) FROM playlists p2 WHERE p2.name COLLATE NOCASE < playlists.name COLLATE NOCASE OR (p2.name COLLATE NOCASE = playlists.name COLLATE NOCASE AND p2.id < playlists.id));",
        )?;
    }
    // Rückgängig für Playlist-Änderungen
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS playlist_backups (
            batch_id INTEGER NOT NULL REFERENCES move_batches(id) ON DELETE CASCADE,
            playlist_id INTEGER NOT NULL,
            existed INTEGER NOT NULL,
            name TEXT NOT NULL DEFAULT '',
            rules TEXT,
            items TEXT NOT NULL DEFAULT '[]',
            pos INTEGER NOT NULL DEFAULT 0
        );",
    )?;
    // Titel, deren Datei verschwunden ist (z. B. im Explorer verschoben): kurz merken, damit sie beim
    // Wiederauftauchen ihre ID, Analyse und Playlist-Plätze behalten
    if !has("lost_tracks", "id") {
        conn.execute_batch("CREATE TABLE lost_tracks AS SELECT * FROM tracks WHERE 0;")?;
    }
    {
        let cols = |t: &str| -> Result<Vec<String>> {
            let mut st = conn.prepare(&format!("PRAGMA table_info({t})"))?;
            let v = st
                .query_map([], |r| r.get::<_, String>(1))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(v)
        };
        let have = cols("lost_tracks")?;
        for c in cols("tracks")? {
            if !have.contains(&c) {
                conn.execute_batch(&format!("ALTER TABLE lost_tracks ADD COLUMN {c}"))?;
            }
        }
        if !have.iter().any(|c| c == "lost_at") {
            conn.execute_batch("ALTER TABLE lost_tracks ADD COLUMN lost_at INTEGER NOT NULL DEFAULT 0; ALTER TABLE lost_tracks ADD COLUMN lost_playlists TEXT NOT NULL DEFAULT '';")?;
        }
    }
    if !has("move_batches", "kind") {
        conn.execute_batch(
            "ALTER TABLE move_batches ADD COLUMN kind TEXT NOT NULL DEFAULT 'move'",
        )?;
    }
    Ok(())
}

pub fn open(path: &Path) -> Result<Connection> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let conn = Connection::open(path)?;
    conn.execute_batch(SCHEMA)?;
    migrate(&conn)?;
    Ok(conn)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub mtime: i64,
    pub added: i64,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub comment: Option<String>,
    pub duration: f64,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub has_cover: bool,
    /// BPM aus der Analyse, sonst aus dem Tag
    pub bpm: Option<f64>,
    /// Tonart aus der Analyse, sonst aus dem Tag (z. B. "Am", "F#")
    pub key: Option<String>,
    pub analyzed: bool,
    pub analysis_error: Option<String>,
    /// Gemessene obere Grenzfrequenz in Hz (Encoder-Tiefpass); None = noch nicht gemessen, -1 = nicht messbar
    pub cutoff: Option<i64>,
    /// Aus der Grenzfrequenz geschätzte echte Qualität (kbps, MP3-Maßstab); None = unbekannt / voller Frequenzgang bei verlustfrei
    pub real_kbps: Option<u32>,
    /// Wahrscheinlich hochgerechnet (z. B. 128er als 320er oder MP3 als FLAC)
    pub upscaled: bool,
    pub lossless: bool,
}

/// Echte Qualität aus Grenzfrequenz, angegebener Bitrate und Format
pub fn quality(
    path: &str,
    bitrate: Option<u32>,
    rate: Option<u32>,
    cutoff: Option<i64>,
) -> (Option<u32>, bool, bool) {
    let ext = Path::new(path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let lossless = matches!(
        ext.as_str(),
        "flac" | "wav" | "aiff" | "aif" | "ape" | "wv" | "alac"
    ) || (ext == "m4a" && bitrate.is_some_and(|b| b > 500));
    let Some(c) = cutoff.filter(|&c| c > 0) else {
        return (None, false, lossless);
    };
    if rate.is_some_and(|r| r < 40000) {
        return (None, false, lossless);
    }
    let est = crate::analysis::kbps_from_cutoff(c as u32);
    if lossless {
        // voller Frequenzgang ist normal – nur ein typischer MP3-Tiefpass verrät die Herkunft
        return if c < 19_500 {
            (Some(est), true, true)
        } else {
            (None, false, true)
        };
    }
    let flag = matches!(ext.as_str(), "mp3" | "m4a" | "aac" | "wma")
        && bitrate.is_some_and(|b| b as f64 >= est as f64 * 1.4 && b >= est + 60);
    (Some(est), flag, false)
}

pub const TRACK_COLS: &str = "id, path, size, mtime, added, title, artist, album, album_artist, genre, year, track_no, disc_no, comment, duration, bitrate, sample_rate, channels, has_cover, COALESCE(bpm, tag_bpm), COALESCE(musical_key, tag_key), analyzed, analysis_error, cutoff";

pub fn row_to_track(r: &Row) -> rusqlite::Result<Track> {
    Ok(Track {
        id: r.get(0)?,
        path: r.get(1)?,
        size: r.get(2)?,
        mtime: r.get(3)?,
        added: r.get(4)?,
        title: r.get(5)?,
        artist: r.get(6)?,
        album: r.get(7)?,
        album_artist: r.get(8)?,
        genre: r.get(9)?,
        year: r.get(10)?,
        track_no: r.get(11)?,
        disc_no: r.get(12)?,
        comment: r.get(13)?,
        duration: r.get(14)?,
        bitrate: r.get(15)?,
        sample_rate: r.get(16)?,
        channels: r.get(17)?,
        has_cover: r.get::<_, i64>(18)? != 0,
        bpm: r.get(19)?,
        key: r.get(20)?,
        analyzed: r.get::<_, i64>(21)? != 0,
        analysis_error: r.get(22)?,
        cutoff: r.get(23)?,
        ..Default::default()
    })
    .map(|mut t: Track| {
        (t.real_kbps, t.upscaled, t.lossless) =
            quality(&t.path, t.bitrate, t.sample_rate, t.cutoff);
        t
    })
}

pub fn all_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut st = conn.prepare(&format!("SELECT {TRACK_COLS} FROM tracks ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE"))?;
    let rows = st.query_map([], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn track(conn: &Connection, id: i64) -> Result<Option<Track>> {
    Ok(conn
        .query_row(
            &format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1"),
            [id],
            row_to_track,
        )
        .optional()?)
}

pub fn tracks_by_ids(conn: &Connection, ids: &[i64]) -> Result<Vec<Track>> {
    let mut out = Vec::with_capacity(ids.len());
    let mut st = conn.prepare(&format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1"))?;
    for id in ids {
        if let Some(t) = st.query_row([id], row_to_track).optional()? {
            out.push(t);
        }
    }
    Ok(out)
}

pub fn peaks(conn: &Connection, id: i64) -> Result<Option<Vec<u8>>> {
    Ok(conn
        .query_row("SELECT peaks FROM tracks WHERE id = ?1", [id], |r| {
            r.get::<_, Option<Vec<u8>>>(0)
        })
        .optional()?
        .flatten())
}

pub fn fingerprints(conn: &Connection) -> Result<std::collections::HashMap<i64, Vec<u32>>> {
    let mut st =
        conn.prepare("SELECT id, fingerprint FROM tracks WHERE fingerprint IS NOT NULL")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?;
    let mut map = std::collections::HashMap::new();
    for row in rows {
        let (id, blob) = row?;
        map.insert(id, bytes_to_u32(&blob));
    }
    Ok(map)
}

pub fn u32_to_bytes(v: &[u32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

pub fn bytes_to_u32(b: &[u8]) -> Vec<u32> {
    b.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect()
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| {
            r.get(0)
        })
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Titel-Eintrag für eine Dateikopie anlegen (alle Tags/Analysen übernehmen, neuer Pfad).
/// Gibt die neue ID zurück.
pub fn clone_track(conn: &Connection, id: i64, new_path: &str) -> Result<i64> {
    let cols: Vec<String> = {
        let mut st = conn.prepare("PRAGMA table_info(tracks)")?;
        st.query_map([], |r| r.get::<_, String>(1))?
            .collect::<rusqlite::Result<_>>()?
    };
    let cols: Vec<&String> = cols.iter().filter(|c| c.as_str() != "id").collect();
    let names = cols
        .iter()
        .map(|c| format!("\"{c}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let values = cols
        .iter()
        .map(|c| {
            if c.as_str() == "path" {
                "?2".to_string()
            } else {
                format!("\"{c}\"")
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!("INSERT INTO tracks({names}) SELECT {values} FROM tracks WHERE id = ?1"),
        rusqlite::params![id, new_path],
    )?;
    let new_id = conn.last_insert_rowid();
    // Beatgrid/Cues (Pioneer) mitnehmen
    conn.execute(
        "INSERT OR IGNORE INTO pioneer_analysis(track_id, bpm, downbeat_ms, grid_edited, frames, updated)
         SELECT ?2, bpm, downbeat_ms, grid_edited, frames, updated FROM pioneer_analysis WHERE track_id = ?1",
        rusqlite::params![id, new_id],
    )?;
    conn.execute(
        "INSERT INTO cues(track_id, kind, start_ms, end_ms, color, comment) SELECT ?2, kind, start_ms, end_ms, color, comment FROM cues WHERE track_id = ?1",
        rusqlite::params![id, new_id],
    )?;
    Ok(new_id)
}

pub fn remove_tracks(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut st = conn.prepare("DELETE FROM tracks WHERE id = ?1")?;
    for id in ids {
        st.execute([id])?;
    }
    Ok(())
}

#[cfg(test)]
mod quality_tests {
    use super::quality;
    #[test]
    fn upscaled() {
        assert_eq!(
            quality("a.mp3", Some(320), Some(44100), Some(16_900)),
            (Some(128), true, false)
        );
        assert_eq!(
            quality("a.mp3", Some(128), Some(44100), Some(16_900)),
            (Some(128), false, false)
        );
        assert_eq!(
            quality("a.mp3", Some(320), Some(44100), Some(20_400)),
            (Some(320), false, false)
        );
        assert_eq!(
            quality("a.flac", Some(900), Some(44100), Some(17_000)),
            (Some(128), true, true)
        );
        assert_eq!(
            quality("a.flac", Some(900), Some(44100), Some(22_050)),
            (None, false, true)
        );
        assert_eq!(
            quality("a.mp3", Some(320), Some(44100), None),
            (None, false, false)
        );
    }
}

/// Spalten der Titel-Tabelle (für das Merken verschwundener Titel)
fn track_columns(conn: &Connection) -> Result<String> {
    let mut st = conn.prepare("PRAGMA table_info(tracks)")?;
    let v = st
        .query_map([], |r| r.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v.join(", "))
}

/// Titel aus der Bibliothek nehmen, weil die Datei weg ist – aber eine Woche lang merken (mit Playlist-Plätzen).
/// Taucht die Datei woanders wieder auf (gleiche Größe + Zeit oder gleicher Name + Größe), bekommt sie alles zurück.
pub fn forget_tracks(conn: &Connection, ids: &[i64]) -> Result<usize> {
    if ids.is_empty() {
        return Ok(0);
    }
    let cols = track_columns(conn)?;
    conn.execute(
        "DELETE FROM lost_tracks WHERE lost_at < ?1",
        [crate::now() - 7 * 86400],
    )?;
    let mut n = 0;
    for id in ids {
        conn.execute("DELETE FROM lost_tracks WHERE id = ?1", [id])?;
        conn.execute(
            &format!(
                "INSERT INTO lost_tracks({cols}, lost_at, lost_playlists) SELECT {cols}, ?2,
                   COALESCE((SELECT group_concat(playlist_id || ':' || pos) FROM playlist_items WHERE track_id = ?1), '')
                 FROM tracks WHERE id = ?1"
            ),
            params![id, crate::now()],
        )?;
        n += conn.execute("DELETE FROM tracks WHERE id = ?1", [id])?;
    }
    Ok(n)
}

/// Neue Datei = früher verschwundener Titel? Dann mit alter ID, Analyse und Playlist-Plätzen zurückholen.
pub fn revive_lost(conn: &Connection, path: &Path, size: i64, mtime: i64) -> Result<Option<i64>> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let found: Option<i64> = conn
        .query_row(
            "SELECT id FROM lost_tracks WHERE size = ?1 AND (mtime = ?2 OR path LIKE '%/' || ?3 OR path LIKE '%\\' || ?3)
             ORDER BY (mtime = ?2) DESC, lost_at DESC LIMIT 1",
            params![size, mtime, name],
            |r| r.get(0),
        )
        .optional()?;
    let Some(id) = found else { return Ok(None) };
    let taken: bool = conn.query_row("SELECT COUNT(*) FROM tracks WHERE id = ?1", [id], |r| {
        r.get::<_, i64>(0)
    })? > 0;
    if taken {
        conn.execute("DELETE FROM lost_tracks WHERE id = ?1", [id])?;
        return Ok(None);
    }
    let cols = track_columns(conn)?;
    conn.execute(
        &format!("INSERT INTO tracks({cols}) SELECT {cols} FROM lost_tracks WHERE id = ?1"),
        [id],
    )?;
    conn.execute(
        "UPDATE tracks SET path = ?2, mtime = ?3 WHERE id = ?1",
        params![id, path.to_string_lossy(), mtime],
    )?;
    let pls: String = conn.query_row(
        "SELECT lost_playlists FROM lost_tracks WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    for part in pls.split(',').filter(|s| !s.is_empty()) {
        let Some((pl, pos)) = part.split_once(':') else {
            continue;
        };
        let (Ok(pl), Ok(pos)) = (pl.parse::<i64>(), pos.parse::<i64>()) else {
            continue;
        };
        let exists: bool =
            conn.query_row("SELECT COUNT(*) FROM playlists WHERE id = ?1", [pl], |r| {
                r.get::<_, i64>(0)
            })? > 0;
        if !exists {
            continue;
        }
        conn.execute(
            "UPDATE playlist_items SET pos = pos + 1 WHERE playlist_id = ?1 AND pos >= ?2",
            params![pl, pos],
        )?;
        conn.execute(
            "INSERT INTO playlist_items(playlist_id, pos, track_id) VALUES(?1, ?2, ?3)",
            params![pl, pos, id],
        )?;
    }
    conn.execute("DELETE FROM lost_tracks WHERE id = ?1", [id])?;
    Ok(Some(id))
}

#[cfg(test)]
mod lost_tests {
    use super::*;
    #[test]
    fn forget_and_revive_keeps_id_and_playlist() {
        let f = std::env::temp_dir().join(format!("mllost{}.db", std::process::id()));
        let _ = std::fs::remove_file(&f);
        let c = open(&f).unwrap();
        c.execute(
            "INSERT INTO tracks(path, size, mtime, added) VALUES('/a/x.mp3', 100, 5, 0)",
            [],
        )
        .unwrap();
        let id = c.last_insert_rowid();
        let pl = crate::playlists::create(&c, "P", None).unwrap();
        c.execute(
            "INSERT INTO playlist_items(playlist_id, pos, track_id) VALUES(?1, 0, ?2)",
            params![pl, id],
        )
        .unwrap();
        forget_tracks(&c, &[id]).unwrap();
        assert_eq!(
            c.query_row("SELECT COUNT(*) FROM tracks", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            0
        );
        let got = revive_lost(&c, Path::new("/b/x.mp3"), 100, 9).unwrap();
        assert_eq!(got, Some(id));
        let p: String = c
            .query_row("SELECT path FROM tracks WHERE id = ?1", [id], |r| r.get(0))
            .unwrap();
        assert_eq!(p, "/b/x.mp3");
        assert_eq!(crate::playlists::track_ids(&c, pl).unwrap(), vec![id]);
    }
}

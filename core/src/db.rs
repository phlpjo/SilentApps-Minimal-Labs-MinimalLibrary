use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, Row, params};
use serde::{Deserialize, Serialize};
use std::path::Path;

const SCHEMA: &str = r#"
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
        conn.prepare(&format!("SELECT {col} FROM {table} LIMIT 0")).is_ok()
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
    if !has("move_batches", "kind") {
        conn.execute_batch("ALTER TABLE move_batches ADD COLUMN kind TEXT NOT NULL DEFAULT 'move'")?;
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
}

pub const TRACK_COLS: &str = "id, path, size, mtime, added, title, artist, album, album_artist, genre, year, track_no, disc_no, comment, duration, bitrate, sample_rate, channels, has_cover, COALESCE(bpm, tag_bpm), COALESCE(musical_key, tag_key), analyzed, analysis_error";

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
    })
}

pub fn all_tracks(conn: &Connection) -> Result<Vec<Track>> {
    let mut st = conn.prepare(&format!("SELECT {TRACK_COLS} FROM tracks ORDER BY artist COLLATE NOCASE, album COLLATE NOCASE, disc_no, track_no, title COLLATE NOCASE"))?;
    let rows = st.query_map([], row_to_track)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

pub fn track(conn: &Connection, id: i64) -> Result<Option<Track>> {
    Ok(conn
        .query_row(&format!("SELECT {TRACK_COLS} FROM tracks WHERE id = ?1"), [id], row_to_track)
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
        .query_row("SELECT peaks FROM tracks WHERE id = ?1", [id], |r| r.get::<_, Option<Vec<u8>>>(0))
        .optional()?
        .flatten())
}

pub fn fingerprints(conn: &Connection) -> Result<std::collections::HashMap<i64, Vec<u32>>> {
    let mut st = conn.prepare("SELECT id, fingerprint FROM tracks WHERE fingerprint IS NOT NULL")?;
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
    b.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

pub fn get_setting(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM settings WHERE key = ?1", [key], |r| r.get(0))
        .optional()?)
}

pub fn set_setting(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO settings(key, value) VALUES(?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn remove_tracks(conn: &Connection, ids: &[i64]) -> Result<()> {
    let mut st = conn.prepare("DELETE FROM tracks WHERE id = ?1")?;
    for id in ids {
        st.execute([id])?;
    }
    Ok(())
}

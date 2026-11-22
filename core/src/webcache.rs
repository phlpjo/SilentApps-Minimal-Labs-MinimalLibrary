//! Zwischenspeicher für Online-Abfragen (MusicBrainz, Cover, AcoustID) – Einträge verfallen nach 3 Tagen.

use crate::now;
use rusqlite::{Connection, OptionalExtension, params};
use std::sync::Mutex;

pub const TTL_SECS: i64 = 3 * 24 * 3600;

pub type Cache<'a> = Option<&'a Mutex<Connection>>;

fn ensure(c: &Connection) {
    let _ = c.execute_batch(
        "CREATE TABLE IF NOT EXISTS web_cache (key TEXT PRIMARY KEY, created INTEGER NOT NULL, body BLOB NOT NULL);",
    );
}

pub fn get(cache: Cache, key: &str) -> Option<Vec<u8>> {
    let c = cache?.lock().ok()?;
    ensure(&c);
    c.query_row(
        "SELECT body FROM web_cache WHERE key = ?1 AND created > ?2",
        params![key, now() - TTL_SECS],
        |r| r.get(0),
    )
    .optional()
    .ok()
    .flatten()
}

pub fn put(cache: Cache, key: &str, body: &[u8]) {
    if let Some(Ok(c)) = cache.map(|m| m.lock()) {
        ensure(&c);
        let _ = c.execute(
            "INSERT INTO web_cache(key, created, body) VALUES(?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET created = excluded.created, body = excluded.body",
            params![key, now(), body],
        );
        let _ = c.execute(
            "DELETE FROM web_cache WHERE created <= ?1",
            [now() - TTL_SECS],
        );
    }
}

/// Abgelaufene Einträge löschen (beim Start und nach jeder Abfrage)
pub fn purge(c: &Connection) -> usize {
    ensure(c);
    c.execute(
        "DELETE FROM web_cache WHERE created <= ?1",
        [now() - TTL_SECS],
    )
    .unwrap_or(0)
}

pub fn clear(c: &Connection) -> usize {
    ensure(c);
    c.execute("DELETE FROM web_cache", []).unwrap_or(0)
}

//! OneLibrary (Device Library Plus): verschlüsselte SQLite-Datenbank `exportLibrary.db` auf dem Stick.
//!
//! Tabellen und Spalten nach pyrekordbox/onelibrary/models.py (MIT, © Dylan Jones).
//! Die Datenbank ist mit SQLCipher verschlüsselt; der Schlüssel ist für alle Geräte gleich und stammt
//! ebenfalls aus pyrekordbox (dort verschleiert abgelegt). Siehe THIRD_PARTY_NOTICES.md.

use anyhow::{Context, Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

/// Ort der Datenbank auf dem Laufwerk.
/// OFFEN: In der erlaubten Referenz nicht dokumentiert – Wert so, wie rekordbox-Exporte ihn
/// üblicherweise anlegen. Beim Hardwaretest prüfen (docs/pioneer-hardwaretest.md).
pub const DB_REL: &str = "PIONEER/rekordbox/exportLibrary.db";

/// Schlüssel (XOR-verschleiert, damit er nicht im Klartext im Programm steht)
const KEY_X: [u8; 64] = [
    0x3f, 0x51, 0x09, 0x0d, 0x09, 0x0f, 0x1e, 0x78, 0x02, 0x5a, 0x46, 0x56, 0x4a, 0x4a, 0x1d, 0x2e, 0x1c, 0x53, 0x2f, 0x18, 0x18, 0x10,
    0x0a, 0x19, 0x49, 0x28, 0x05, 0x58, 0x18, 0x14, 0x15, 0x01, 0x2e, 0x5a, 0x16, 0x00, 0x03, 0x10, 0x01, 0x15, 0x7f, 0x5b, 0x0f, 0x21,
    0x5e, 0x56, 0x04, 0x04, 0x08, 0x4f, 0x79, 0x00, 0x5b, 0x06, 0x55, 0x07, 0x02, 0x3e, 0x18, 0x10, 0x0b, 0x10, 0x1e, 0x0a,
];
const KEY_MASK: &[u8] = b"MinimalLibrary-OneLibrary";

pub fn key() -> String {
    KEY_X.iter().enumerate().map(|(i, b)| (b ^ KEY_MASK[i % KEY_MASK.len()]) as char).collect()
}

/// Tabellen wie in pyrekordbox/onelibrary/models.py („key“ ist ein SQL-Schlüsselwort → in Anführungszeichen)
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS artist (artist_id INTEGER PRIMARY KEY AUTOINCREMENT, name VARCHAR(255) UNIQUE, nameForSearch VARCHAR(255));
CREATE TABLE IF NOT EXISTS image (image_id INTEGER PRIMARY KEY AUTOINCREMENT, path VARCHAR(255) UNIQUE);
CREATE TABLE IF NOT EXISTS album (album_id INTEGER PRIMARY KEY AUTOINCREMENT, name VARCHAR(255) UNIQUE, artist_id INTEGER REFERENCES artist(artist_id), image_id INTEGER REFERENCES image(image_id), isComplation INTEGER, nameForSearch VARCHAR(255));
CREATE TABLE IF NOT EXISTS genre (genre_id INTEGER PRIMARY KEY AUTOINCREMENT, name VARCHAR(255) UNIQUE);
CREATE TABLE IF NOT EXISTS label (label_id INTEGER PRIMARY KEY AUTOINCREMENT, name VARCHAR(255) UNIQUE);
CREATE TABLE IF NOT EXISTS "key" (key_id INTEGER PRIMARY KEY AUTOINCREMENT, name VARCHAR(255) UNIQUE);
CREATE TABLE IF NOT EXISTS color (color_id INTEGER PRIMARY KEY AUTOINCREMENT, name VARCHAR(255) UNIQUE);
CREATE TABLE IF NOT EXISTS menuItem (menuItem_id INTEGER PRIMARY KEY AUTOINCREMENT, kind INTEGER, name VARCHAR(255));
CREATE TABLE IF NOT EXISTS category (category_id INTEGER PRIMARY KEY AUTOINCREMENT, menuItem_id INTEGER REFERENCES menuItem(menuItem_id), sequenceNo INTEGER, isVisible INTEGER);
CREATE TABLE IF NOT EXISTS sort (sort_id INTEGER PRIMARY KEY AUTOINCREMENT, menuItem_id INTEGER NOT NULL REFERENCES menuItem(menuItem_id), sequenceNo INTEGER, isVisible INTEGER, isSelectedAsSubColumn INTEGER);
CREATE TABLE IF NOT EXISTS content (
  content_id INTEGER PRIMARY KEY AUTOINCREMENT, title VARCHAR(255), titleForSearch VARCHAR(255), subtitle VARCHAR(255), bpmx100 INTEGER, length INTEGER,
  trackNo INTEGER, discNo INTEGER, artist_id_artist INTEGER REFERENCES artist(artist_id), artist_id_remixer INTEGER REFERENCES artist(artist_id),
  artist_id_originalArtist INTEGER REFERENCES artist(artist_id), artist_id_composer INTEGER REFERENCES artist(artist_id), artist_id_lyricist INTEGER REFERENCES artist(artist_id),
  album_id INTEGER REFERENCES album(album_id), genre_id INTEGER REFERENCES genre(genre_id), label_id INTEGER REFERENCES label(label_id), key_id INTEGER REFERENCES "key"(key_id),
  color_id INTEGER REFERENCES color(color_id), image_id INTEGER REFERENCES image(image_id), djComment VARCHAR(255), rating INTEGER, releaseYear INTEGER, releaseDate TEXT,
  dateCreated TEXT, dateAdded TEXT, path VARCHAR(255) NOT NULL UNIQUE, fileName VARCHAR(255) NOT NULL, fileSize BIGINT NOT NULL, fileType INTEGER NOT NULL,
  bitrate INTEGER NOT NULL, bitDepth INTEGER NOT NULL, samplingRate INTEGER NOT NULL, isrc VARCHAR(12), djPlayCount INTEGER, isHotCueAutoLoadOn INTEGER,
  isKuvoDeliverStatusOn INTEGER, kuvoDeliveryComment VARCHAR(255), masterDbId INTEGER, masterContentId INTEGER, analysisDataFilePath VARCHAR(255), analysedBits INTEGER,
  contentLink INTEGER, hasModified INTEGER, cueUpdateCount INTEGER, analysisDataUpdateCount INTEGER, informationUpdateCount INTEGER);
CREATE TABLE IF NOT EXISTS cue (
  cue_id INTEGER PRIMARY KEY AUTOINCREMENT, content_id INTEGER REFERENCES content(content_id), kind INTEGER, colorTableIndex INTEGER, cueComment VARCHAR(255),
  isActiveLoop INTEGER, beatLoopNumerator INTEGER, beatLoopDenominator INTEGER, inUsec INTEGER, outUsec INTEGER, in150FramePerSec INTEGER, out150FramePerSec INTEGER,
  inMpegFrameNumber INTEGER, outMpegFrameNumber INTEGER, inMpegAbs INTEGER, outMpegAbs INTEGER, inDecodingStartFramePosition INTEGER, outDecodingStartFramePosition INTEGER,
  inFileOffsetInBlock INTEGER, outFileOffsetInBlock INTEGER, inNumberOfSampleInBlock INTEGER, outNumberOfSampleInBlock INTEGER);
CREATE TABLE IF NOT EXISTS playlist (playlist_id INTEGER PRIMARY KEY AUTOINCREMENT, sequenceNo INTEGER NOT NULL, name VARCHAR(255) NOT NULL, image_id INTEGER REFERENCES image(image_id), attribute INTEGER, playlist_id_parent INTEGER);
CREATE TABLE IF NOT EXISTS playlist_content (playlist_id INTEGER NOT NULL REFERENCES playlist(playlist_id), content_id INTEGER NOT NULL REFERENCES content(content_id), sequenceNo INTEGER NOT NULL, PRIMARY KEY (playlist_id, content_id));
CREATE TABLE IF NOT EXISTS history (history_id INTEGER PRIMARY KEY AUTOINCREMENT, sequenceNo INTEGER, name VARCHAR(255), attribute TEXT, history_id_parent INTEGER);
CREATE TABLE IF NOT EXISTS history_content (history_id INTEGER NOT NULL, content_id INTEGER NOT NULL, sequenceNo INTEGER, PRIMARY KEY (history_id, content_id));
CREATE TABLE IF NOT EXISTS hotCueBankList (hotCueBankList_id INTEGER PRIMARY KEY AUTOINCREMENT, sequenceNo INTEGER, name VARCHAR(255), image_id INTEGER, attribute INTEGER, hotCueBankList_id_parent INTEGER);
CREATE TABLE IF NOT EXISTS hotCueBankList_cue (hotCueBankList_id INTEGER NOT NULL, cue_id INTEGER NOT NULL, sequenceNo INTEGER, PRIMARY KEY (hotCueBankList_id, cue_id));
CREATE TABLE IF NOT EXISTS myTag (myTag_id INTEGER PRIMARY KEY AUTOINCREMENT, sequenceNo INTEGER NOT NULL, name VARCHAR(255) UNIQUE, attribute INTEGER, myTag_id_parent INTEGER);
CREATE TABLE IF NOT EXISTS myTag_content (myTag_id INTEGER NOT NULL, content_id INTEGER NOT NULL, PRIMARY KEY (myTag_id, content_id));
CREATE TABLE IF NOT EXISTS property (deviceName VARCHAR(255) NOT NULL PRIMARY KEY, dbVersion INTEGER, numberOfContents INTEGER, createdDate TEXT, backGroundColorType INTEGER, myTagMasterDBID INTEGER);
CREATE TABLE IF NOT EXISTS recommendedLike (content_id_1 INTEGER NOT NULL, content_id_2 INTEGER NOT NULL, rating INTEGER, createdDate TEXT, PRIMARY KEY (content_id_1, content_id_2));
"#;

/// Datenbank öffnen (vorhandene, z. B. von rekordbox) oder neu anlegen. `encrypted = false` nur für Tests.
pub fn open(path: &Path, encrypted: bool) -> Result<(Connection, bool)> {
    let existed = path.exists();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let c = Connection::open(path).with_context(|| format!("{} öffnen", path.display()))?;
    if encrypted {
        c.pragma_update(None, "key", key())?;
    }
    // Prüfen, ob sich die Datei mit diesem Schlüssel lesen lässt
    c.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0))
        .map_err(|_| anyhow!("Die vorhandene Bibliothek auf dem Laufwerk lässt sich nicht öffnen (anderes Format oder beschädigt) – es wurde nichts verändert."))?;
    if !existed {
        c.execute_batch(SCHEMA)?;
    } else {
        // Bestehende Bibliothek: nur prüfen, ob die nötigen Tabellen da sind – nichts umbauen
        for t in ["content", "playlist", "playlist_content", "cue", "artist", "album", "genre", "key", "image", "property"] {
            let ok: Option<String> = c
                .query_row("SELECT name FROM sqlite_master WHERE type = 'table' AND name = ?1", [t], |r| r.get(0))
                .optional()?;
            if ok.is_none() {
                return Err(anyhow!("Die Bibliothek auf dem Laufwerk hat ein unbekanntes Format (Tabelle „{t}“ fehlt) – es wurde nichts verändert."));
            }
        }
    }
    Ok((c, !existed))
}

/// Zeitstempel im Format der Datenbank: "YYYY-MM-DD HH:MM:SS.SSS +00:00"
pub fn timestamp(unix: i64) -> String {
    let days = unix.div_euclid(86_400);
    let secs = unix.rem_euclid(86_400);
    // Kalenderdatum aus Tagen seit 1970 (Algorithmus nach Howard Hinnant, gemeinfrei)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02}.000 +00:00", secs / 3600, secs / 60 % 60, secs % 60)
}

/// Eintrag mit eindeutigem Namen holen oder anlegen (artist, album, genre, key, label, image)
pub fn id_for(c: &Connection, table: &str, col: &str, value: &str) -> Result<i64> {
    let idc = format!("{}_id", table.trim_matches('"'));
    if let Some(id) = c
        .query_row(&format!("SELECT {idc} FROM {table} WHERE {col} = ?1"), [value], |r| r.get(0))
        .optional()?
    {
        return Ok(id);
    }
    c.execute(&format!("INSERT INTO {table}({col}) VALUES(?1)"), [value])?;
    Ok(c.last_insert_rowid())
}

/// Dateityp-Nummer der Datenbank (pyrekordbox FileType). None = von den Playern nicht abspielbar.
pub fn file_type(ext: &str) -> Option<i64> {
    match ext.to_ascii_lowercase().as_str() {
        "mp3" => Some(1),
        "m4a" | "aac" => Some(4),
        "mp4" => Some(3),
        "flac" => Some(5),
        "wav" => Some(11),
        "aif" | "aiff" => Some(12),
        _ => None,
    }
}

/// Text für die Suche der Player (Großbuchstaben, ohne Akzente wäre schöner – OFFEN: genaue Regel)
pub fn for_search(s: &str) -> String {
    s.to_uppercase()
}

pub fn set_property(c: &Connection, device_name: &str, now: i64) -> Result<()> {
    let n: i64 = c.query_row("SELECT count(*) FROM content", [], |r| r.get(0))?;
    let has: i64 = c.query_row("SELECT count(*) FROM property", [], |r| r.get(0))?;
    if has == 0 {
        // dbVersion 1000 wie der Standardwert in pyrekordbox. OFFEN: Wert, den die Player erwarten.
        c.execute(
            "INSERT INTO property(deviceName, dbVersion, numberOfContents, createdDate, backGroundColorType, myTagMasterDBID) VALUES(?1, 1000, ?2, ?3, 0, 0)",
            params![device_name, n, timestamp(now)],
        )?;
    } else {
        c.execute("UPDATE property SET numberOfContents = ?1", [n])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_and_timestamp() {
        let k = key();
        assert_eq!(k.len(), 64);
        assert!(k.starts_with("r8gd"));
        assert_eq!(timestamp(0), "1970-01-01 00:00:00.000 +00:00");
        assert_eq!(timestamp(1_795_325_869), "2026-11-22 05:37:49.000 +00:00");
    }

    #[test]
    fn create_encrypted_and_reopen() {
        let p = std::env::temp_dir().join(format!("ml-onelib-{}.db", std::process::id()));
        let _ = std::fs::remove_file(&p);
        {
            let (c, created) = open(&p, true).unwrap();
            assert!(created);
            let a = id_for(&c, "artist", "name", "Band").unwrap();
            assert_eq!(id_for(&c, "artist", "name", "Band").unwrap(), a);
            id_for(&c, "\"key\"", "name", "Am").unwrap();
        }
        // ohne Schlüssel nicht lesbar
        let plain = Connection::open(&p).unwrap();
        assert!(plain.query_row("SELECT count(*) FROM sqlite_master", [], |r| r.get::<_, i64>(0)).is_err());
        let (c, created) = open(&p, true).unwrap();
        assert!(!created);
        assert_eq!(c.query_row("SELECT count(*) FROM artist", [], |r| r.get::<_, i64>(0)).unwrap(), 1);
        let _ = std::fs::remove_file(&p);
    }
}

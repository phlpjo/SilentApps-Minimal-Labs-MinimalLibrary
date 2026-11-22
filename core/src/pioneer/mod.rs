//! Pioneer DJ / AlphaTheta-Player: Beatgrid, Cues, Analysedateien (ANLZ), OneLibrary-Export und
//! rekordbox-XML. Alles nur aktiv, wenn „Pioneer-Export“ in den Einstellungen eingeschaltet ist.
//!
//! Formatreferenz: pyrekordbox (MIT, © Dylan Jones) – siehe THIRD_PARTY_NOTICES.md.
//! Kein Code aus GPL-Projekten.

pub mod anlz;
pub mod export;
pub mod onelibrary;
pub mod xml;

use crate::{history, now};
use anyhow::{Result, anyhow};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

/// Konstantes Beatgrid eines Titels
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Grid {
    pub bpm: f64,
    /// Erster Downbeat (Schlag 1 eines Takts) in ms
    pub downbeat_ms: f64,
    pub edited: bool,
}

impl Grid {
    /// Alle Schläge im Titel: (Zeit in ms, Taktposition 1–4)
    pub fn beats(&self, duration_ms: f64) -> Vec<(f64, u8)> {
        if self.bpm <= 0.0 || duration_ms <= 0.0 {
            return Vec::new();
        }
        let p = 60000.0 / self.bpm;
        // frühesten Schlag ≥ 0 finden (auch vor dem ersten Downbeat)
        let mut k = -((self.downbeat_ms / p).floor() as i64);
        let mut out = Vec::new();
        loop {
            let t = self.downbeat_ms + k as f64 * p;
            if t >= duration_ms {
                break;
            }
            if t >= 0.0 {
                out.push((t, (k.rem_euclid(4) + 1) as u8));
            }
            k += 1;
        }
        out
    }
}

/// Memory Cue (kind 0) oder Hot Cue A–H (kind 1–8)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Cue {
    #[serde(default)]
    pub id: i64,
    pub kind: u8,
    pub start_ms: f64,
    /// Ende einer Schleife (optional)
    pub end_ms: Option<f64>,
    /// Farbindex der Player (0 = keine) – siehe Oberfläche
    pub color: Option<u8>,
    pub comment: Option<String>,
}

pub fn grid(c: &Connection, track: i64) -> Result<Option<Grid>> {
    Ok(c.query_row(
        "SELECT bpm, downbeat_ms, grid_edited FROM pioneer_analysis WHERE track_id = ?1",
        [track],
        |r| {
            Ok(Grid {
                bpm: r.get(0)?,
                downbeat_ms: r.get(1)?,
                edited: r.get::<_, i64>(2)? != 0,
            })
        },
    )
    .optional()?)
}

/// Pioneer-Wellenform (150 Werte/s × [Spitze, Bass, Mitten, Höhen])
pub fn frames(c: &Connection, track: i64) -> Result<Option<Vec<u8>>> {
    Ok(c.query_row(
        "SELECT frames FROM pioneer_analysis WHERE track_id = ?1",
        [track],
        |r| r.get::<_, Option<Vec<u8>>>(0),
    )
    .optional()?
    .flatten())
}

pub fn cues(c: &Connection, track: i64) -> Result<Vec<Cue>> {
    let mut st = c.prepare("SELECT id, kind, start_ms, end_ms, color, comment FROM cues WHERE track_id = ?1 ORDER BY kind, start_ms")?;
    let v = st
        .query_map([track], |r| {
            Ok(Cue {
                id: r.get(0)?,
                kind: r.get::<_, i64>(1)? as u8,
                start_ms: r.get(2)?,
                end_ms: r.get(3)?,
                color: r.get::<_, Option<i64>>(4)?.map(|v| v as u8),
                comment: r.get(5)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?;
    Ok(v)
}

/// Stand vor einer Änderung sichern (für Strg+Z)
fn backup(c: &Connection, batch: i64, track: i64) -> Result<()> {
    let g = grid(c, track)?;
    let cs = cues(c, track)?;
    c.execute(
        "INSERT INTO grid_backups(batch_id, track_id, grid, cues) VALUES(?1, ?2, ?3, ?4)",
        params![
            batch,
            track,
            g.map(|g| serde_json::to_string(&g)).transpose()?,
            serde_json::to_string(&cs)?
        ],
    )?;
    Ok(())
}

/// Beatgrid setzen (Downbeat, Tempo). Rückgängig über den Verlauf.
pub fn set_grid(c: &Connection, track: i64, bpm: f64, downbeat_ms: f64, what: &str) -> Result<i64> {
    if !(20.0..=400.0).contains(&bpm) {
        return Err(anyhow!("Tempo außerhalb von 20–400 BPM"));
    }
    let batch = history::begin(c, "grid", what)?;
    backup(c, batch, track)?;
    let bpm = (bpm * 100.0).round() / 100.0;
    c.execute(
        "INSERT INTO pioneer_analysis(track_id, bpm, downbeat_ms, grid_edited, updated) VALUES(?1, ?2, ?3, 1, ?4)
         ON CONFLICT(track_id) DO UPDATE SET bpm = excluded.bpm, downbeat_ms = excluded.downbeat_ms, grid_edited = 1, updated = excluded.updated",
        params![track, bpm, downbeat_ms, now()],
    )?;
    history::finish(c, batch, 1)?;
    Ok(batch)
}

/// Cue anlegen oder ersetzen (Hot Cue A–H gibt es je Titel nur einmal)
pub fn set_cue(c: &Connection, track: i64, cue: &Cue) -> Result<i64> {
    if cue.kind > 8 {
        return Err(anyhow!("Ungültiger Cue"));
    }
    let what = if cue.kind == 0 {
        "Memory Cue gesetzt".to_string()
    } else {
        format!("Hot Cue {} gesetzt", (b'A' + cue.kind - 1) as char)
    };
    let batch = history::begin(c, "grid", &what)?;
    backup(c, batch, track)?;
    if cue.kind > 0 {
        c.execute(
            "DELETE FROM cues WHERE track_id = ?1 AND kind = ?2",
            params![track, cue.kind],
        )?;
    }
    c.execute(
        "INSERT INTO cues(track_id, kind, start_ms, end_ms, color, comment) VALUES(?1, ?2, ?3, ?4, ?5, ?6)",
        params![track, cue.kind, cue.start_ms.max(0.0), cue.end_ms, cue.color, cue.comment],
    )?;
    history::finish(c, batch, 1)?;
    Ok(batch)
}

pub fn delete_cue(c: &Connection, track: i64, cue_id: i64) -> Result<i64> {
    let batch = history::begin(c, "grid", "Cue gelöscht")?;
    backup(c, batch, track)?;
    let n = c.execute(
        "DELETE FROM cues WHERE id = ?1 AND track_id = ?2",
        params![cue_id, track],
    )?;
    history::finish(c, batch, n)?;
    Ok(batch)
}

/// Rückgängig: Grid und Cues auf den gesicherten Stand zurücksetzen
pub fn undo(c: &Connection, batch: i64) -> Result<usize> {
    let rows: Vec<(i64, Option<String>, String)> = {
        let mut st =
            c.prepare("SELECT track_id, grid, cues FROM grid_backups WHERE batch_id = ?1")?;
        st.query_map([batch], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (track, g, cs) in &rows {
        match g {
            Some(g) => {
                let g: Grid = serde_json::from_str(g)?;
                c.execute(
                    "UPDATE pioneer_analysis SET bpm = ?2, downbeat_ms = ?3, grid_edited = ?4 WHERE track_id = ?1",
                    params![track, g.bpm, g.downbeat_ms, g.edited as i64],
                )?;
            }
            None => {
                c.execute(
                    "DELETE FROM pioneer_analysis WHERE track_id = ?1 AND frames IS NULL",
                    [track],
                )?;
            }
        }
        let cs: Vec<Cue> = serde_json::from_str(cs)?;
        c.execute("DELETE FROM cues WHERE track_id = ?1", [track])?;
        for q in cs {
            c.execute(
                "INSERT INTO cues(id, track_id, kind, start_ms, end_ms, color, comment) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![q.id, track, q.kind, q.start_ms, q.end_ms, q.color, q.comment],
            )?;
        }
    }
    Ok(rows.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_beats_and_undo() {
        let g = Grid {
            bpm: 120.0,
            downbeat_ms: 1250.0,
            edited: false,
        };
        let b = g.beats(4000.0);
        // 120 BPM = 500 ms: Schläge bei 250 (4), 750 (… ) – erster Downbeat bei 1250
        assert_eq!(b[0], (250.0, 3));
        assert_eq!(b.iter().find(|x| x.1 == 1).unwrap().0, 1250.0);
        assert_eq!(b.len(), 8);

        let c =
            crate::db::open(&std::env::temp_dir().join(format!("mlgrid{}.db", std::process::id())))
                .unwrap();
        c.execute(
            "INSERT INTO tracks(id, path, size) VALUES(1, '/x.mp3', 1)",
            [],
        )
        .unwrap();
        set_grid(&c, 1, 128.0, 100.0, "Test").unwrap();
        let b2 = set_cue(
            &c,
            1,
            &Cue {
                id: 0,
                kind: 1,
                start_ms: 500.0,
                end_ms: None,
                color: Some(3),
                comment: Some("Drop".into()),
            },
        )
        .unwrap();
        set_cue(
            &c,
            1,
            &Cue {
                id: 0,
                kind: 1,
                start_ms: 900.0,
                end_ms: None,
                color: None,
                comment: None,
            },
        )
        .unwrap();
        assert_eq!(cues(&c, 1).unwrap().len(), 1);
        assert_eq!(cues(&c, 1).unwrap()[0].start_ms, 900.0);
        undo(&c, b2 + 1).unwrap();
        assert_eq!(cues(&c, 1).unwrap()[0].start_ms, 500.0);
        let b3 = set_grid(&c, 1, 64.0, 100.0, "halb").unwrap();
        undo(&c, b3).unwrap();
        assert_eq!(grid(&c, 1).unwrap().unwrap().bpm, 128.0);
    }
}

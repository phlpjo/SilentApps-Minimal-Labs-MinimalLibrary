//! Titel am Klang erkennen: Audio-Fingerabdruck → AcoustID → MusicBrainz-Daten.
//! Braucht einen kostenlosen Anwendungs-Schlüssel von https://acoustid.org/new-application

use crate::musicbrainz::Candidate;
use anyhow::{Result, anyhow, bail};
use base64::Engine;
use rusty_chromaprint::{Configuration, FingerprintCompressor};
use serde_json::Value;
use std::time::Duration;

/// Fingerabdruck im Chromaprint-Textformat (komprimiert, Base64 URL-sicher ohne Auffüllung)
pub fn encode(fp: &[u32]) -> String {
    let cfg = Configuration::preset_test2();
    let bytes = FingerprintCompressor::from(&cfg).compress(fp);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_string).filter(|x| !x.is_empty())
}

fn artists(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| format!("{}{}", x["name"].as_str().unwrap_or(""), x["joinphrase"].as_str().unwrap_or("")))
                .collect::<String>()
        })
        .unwrap_or_default()
}

pub fn lookup(key: &str, fp: &[u32], duration: f64) -> Result<Vec<Candidate>> {
    if key.trim().is_empty() {
        bail!("Kein AcoustID-Schlüssel eingetragen (Einstellungen → Online)");
    }
    if fp.len() < 40 {
        bail!("Kein Fingerabdruck – Titel zuerst analysieren");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent("MinimalLibrary/1.0 ( apps@silentapps-minimal-labs.com )")
        .build()
        .into();
    let dur = (duration.round() as i64).max(1).to_string();
    let fpe = encode(fp);
    let v: Value = agent
        .post("https://api.acoustid.org/v2/lookup")
        .send_form([
            ("client", key.trim()),
            ("duration", dur.as_str()),
            ("fingerprint", fpe.as_str()),
            ("meta", "recordings releasegroups releases tracks compress"),
            ("format", "json"),
        ])
        .map_err(|e| anyhow!("AcoustID nicht erreichbar: {e}"))?
        .body_mut()
        .read_json()?;
    if v["status"] != "ok" {
        bail!("AcoustID: {}", v["error"]["message"].as_str().unwrap_or("unbekannter Fehler"));
    }
    let mut out = Vec::new();
    for res in v["results"].as_array().into_iter().flatten() {
        let score = (res["score"].as_f64().unwrap_or(0.0) * 100.0).round() as u32;
        for rec in res["recordings"].as_array().into_iter().flatten() {
            let Some(title) = s(&rec["title"]) else { continue };
            let artist = artists(&rec["artists"]);
            let length = rec["duration"].as_f64();
            let mut added = false;
            // Alben bevorzugen
            let mut groups: Vec<&Value> = rec["releasegroups"].as_array().into_iter().flatten().collect();
            groups.sort_by_key(|g| if g["type"] == "Album" { 0 } else { 1 });
            for g in groups.into_iter().take(3) {
                let album_artist = Some(artists(&g["artists"])).filter(|x| !x.is_empty());
                let mut rels: Vec<&Value> = g["releases"].as_array().into_iter().flatten().collect();
                rels.sort_by_key(|r| r["date"]["year"].as_i64().unwrap_or(9999));
                let rel = rels.first();
                let medium = rel.and_then(|r| r["mediums"].as_array()).and_then(|m| m.first());
                out.push(Candidate {
                    score,
                    title: title.clone(),
                    artist: artist.clone(),
                    album: s(&g["title"]),
                    album_artist,
                    year: rel.and_then(|r| r["date"]["year"].as_i64()).map(|y| y.to_string()),
                    track_no: medium
                        .and_then(|m| m["tracks"].as_array())
                        .and_then(|t| t.first())
                        .and_then(|t| t["position"].as_u64())
                        .map(|x| x as u32),
                    disc_no: medium.and_then(|m| m["position"].as_u64()).map(|x| x as u32),
                    genre: None,
                    release_id: rel.and_then(|r| s(&r["id"])),
                    length,
                });
                added = true;
            }
            if !added {
                out.push(Candidate {
                    score, title, artist, album: None, album_artist: None, year: None, track_no: None, disc_no: None,
                    genre: None, release_id: None, length,
                });
            }
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score));
    out.truncate(25);
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn encode_roundtrip_len() {
        let fp: Vec<u32> = (0..120u32).map(|i| i.wrapping_mul(2654435761)).collect();
        let e = super::encode(&fp);
        assert!(e.starts_with("AQAA"), "Chromaprint-Kopf fehlt: {}", &e[..8]);
    }
}

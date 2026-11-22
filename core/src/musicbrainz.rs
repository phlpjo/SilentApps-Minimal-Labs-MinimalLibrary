//! Optionale Online-Suche nach Tags und Covern (MusicBrainz + Cover Art Archive).
//! Wird nur auf ausdrücklichen Klick genutzt.

use crate::webcache::{self, Cache};
use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::time::Duration;

const UA: &str = "MinimalLibrary/1.0 ( apps@silentapps-minimal-labs.com )";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub score: u32,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub genre: Option<String>,
    pub release_id: Option<String>,
    pub length: Option<f64>,
}

#[derive(Deserialize)]
struct RecResp {
    recordings: Vec<Rec>,
}
#[derive(Deserialize)]
struct Rec {
    score: Option<u32>,
    title: String,
    length: Option<u64>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    #[serde(default)]
    releases: Vec<Release>,
    #[serde(default)]
    tags: Vec<MbTag>,
}
#[derive(Deserialize)]
struct Credit {
    name: String,
    #[serde(default)]
    joinphrase: String,
}
#[derive(Deserialize)]
struct Release {
    id: String,
    title: String,
    date: Option<String>,
    status: Option<String>,
    #[serde(rename = "artist-credit", default)]
    artist_credit: Vec<Credit>,
    #[serde(default)]
    media: Vec<Medium>,
    #[serde(rename = "release-group")]
    release_group: Option<ReleaseGroup>,
}
#[derive(Deserialize)]
struct ReleaseGroup {
    #[serde(rename = "primary-type")]
    primary_type: Option<String>,
}
#[derive(Deserialize)]
struct Medium {
    position: Option<u32>,
    #[serde(default)]
    track: Vec<MTrack>,
}
#[derive(Deserialize)]
struct MTrack {
    number: Option<String>,
}
#[derive(Deserialize)]
struct MbTag {
    name: String,
    count: Option<i64>,
}

fn credit(c: &[Credit]) -> String {
    c.iter().map(|x| format!("{}{}", x.name, x.joinphrase)).collect::<String>()
}

fn esc(s: &str) -> String {
    s.chars()
        .flat_map(|c| if "+-&|!(){}[]^\"~*?:\\/".contains(c) { vec!['\\', c] } else { vec![c] })
        .collect()
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(20)))
        .user_agent(UA)
        .http_status_as_error(false)
        .build()
        .into()
}

/// MusicBrainz erlaubt ~1 Anfrage pro Sekunde und Client: alle Anfragen laufen nacheinander mit Abstand.
fn throttle() -> std::sync::MutexGuard<'static, Option<std::time::Instant>> {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let guard = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = *guard {
        let wait = Duration::from_millis(1100).saturating_sub(t.elapsed());
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
    guard
}

/// GET mit Zwischenspeicher (3 Tage), Drosselung und Wiederholung bei 503/429 (2 s, 4 s, 8 s, 16 s).
pub fn fetch(cache: Cache, url: &str, what: &str) -> Result<Vec<u8>> {
    if let Some(b) = webcache::get(cache, url) {
        return Ok(b);
    }
    let mut delay = 2;
    for attempt in 0..5 {
        let resp = {
            let mut last = throttle(); // hält die Sperre während der Anfrage → nie parallel
            let r = agent().get(url).header("Accept", "application/json").call();
            *last = Some(std::time::Instant::now());
            r
        };
        let mut resp = resp.map_err(|e| anyhow!("{what} nicht erreichbar: {e}"))?;
        let status = resp.status().as_u16();
        match status {
            200..=299 => {
                let body = resp.body_mut().with_config().limit(25 * 1024 * 1024).read_to_vec()?;
                webcache::put(cache, url, &body);
                return Ok(body);
            }
            503 | 429 | 502 | 504 if attempt < 4 => {
                // Server überlastet oder zu schnell gefragt → warten und erneut versuchen
                let retry_after = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(delay)
                    .min(30);
                std::thread::sleep(Duration::from_secs(retry_after.max(delay)));
                delay *= 2;
            }
            404 => return Err(anyhow!("{what}: nichts gefunden")),
            _ => return Err(anyhow!("{what}: Antwort {status}")),
        }
    }
    Err(anyhow!("{what} ist gerade überlastet – bitte später erneut versuchen"))
}

fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
            _ => format!("%{b:02X}"),
        })
        .collect()
}

pub fn search(cache: Cache, artist: &str, title: &str, album: Option<&str>) -> Result<Vec<Candidate>> {
    let mut q = format!("recording:\"{}\"", esc(title));
    if !artist.trim().is_empty() {
        q.push_str(&format!(" AND artist:\"{}\"", esc(artist)));
    }
    if let Some(a) = album.filter(|a| !a.trim().is_empty()) {
        q.push_str(&format!(" AND release:\"{}\"", esc(a)));
    }
    let url = format!("https://musicbrainz.org/ws/2/recording?query={}&fmt=json&limit=15", url_encode(&q));
    let body = fetch(cache, &url, "MusicBrainz")?;
    let resp: RecResp = serde_json::from_slice(&body).map_err(|e| anyhow!("MusicBrainz: unerwartete Antwort ({e})"))?;
    let mut out = Vec::new();
    for r in resp.recordings {
        let genre = r
            .tags
            .iter()
            .max_by_key(|t| t.count.unwrap_or(0))
            .map(|t| {
                let mut c = t.name.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            });
        // Offizielle Alben bevorzugen
        let mut rels: Vec<&Release> = r.releases.iter().collect();
        rels.sort_by_key(|x| {
            let official = x.status.as_deref() == Some("Official");
            let album = x.release_group.as_ref().and_then(|g| g.primary_type.as_deref()) == Some("Album");
            (!official, !album, x.date.clone().unwrap_or_else(|| "9999".into()))
        });
        let base = |rel: Option<&Release>| Candidate {
            score: r.score.unwrap_or(0),
            title: r.title.clone(),
            artist: credit(&r.artist_credit),
            album: rel.map(|x| x.title.clone()),
            album_artist: rel.map(|x| credit(&x.artist_credit)).filter(|s| !s.is_empty()),
            year: rel.and_then(|x| x.date.as_ref()).map(|d| d.chars().take(4).collect()),
            track_no: rel
                .and_then(|x| x.media.first())
                .and_then(|m| m.track.first())
                .and_then(|t| t.number.as_ref())
                .and_then(|n| n.parse().ok()),
            disc_no: rel.and_then(|x| x.media.first()).and_then(|m| m.position),
            genre: genre.clone(),
            release_id: rel.map(|x| x.id.clone()),
            length: r.length.map(|l| l as f64 / 1000.0),
        };
        if rels.is_empty() {
            out.push(base(None));
        } else {
            for rel in rels.into_iter().take(3) {
                out.push(base(Some(rel)));
            }
        }
    }
    Ok(out)
}

/// Front-Cover (500 px) eines Releases laden.
pub fn cover(cache: Cache, release_id: &str) -> Result<Vec<u8>> {
    let url = format!("https://coverartarchive.org/release/{release_id}/front-500");
    fetch(cache, &url, "Cover Art Archive")
}

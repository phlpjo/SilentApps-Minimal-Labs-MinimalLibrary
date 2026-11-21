//! Optionale Online-Suche nach Tags und Covern (MusicBrainz + Cover Art Archive).
//! Wird nur auf ausdrücklichen Klick genutzt.

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
        .timeout_global(Some(Duration::from_secs(15)))
        .user_agent(UA)
        .build()
        .into()
}

pub fn search(artist: &str, title: &str, album: Option<&str>) -> Result<Vec<Candidate>> {
    let mut q = format!("recording:\"{}\"", esc(title));
    if !artist.trim().is_empty() {
        q.push_str(&format!(" AND artist:\"{}\"", esc(artist)));
    }
    if let Some(a) = album.filter(|a| !a.trim().is_empty()) {
        q.push_str(&format!(" OR release:\"{}\"", esc(a)));
    }
    let resp: RecResp = agent()
        .get("https://musicbrainz.org/ws/2/recording")
        .query("query", &q)
        .query("fmt", "json")
        .query("limit", "15")
        .call()
        .map_err(|e| anyhow!("MusicBrainz nicht erreichbar: {e}"))?
        .body_mut()
        .read_json()?;
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
pub fn cover(release_id: &str) -> Result<Vec<u8>> {
    let url = format!("https://coverartarchive.org/release/{release_id}/front-500");
    let mut resp = agent().get(&url).call().map_err(|e| anyhow!("Kein Cover gefunden: {e}"))?;
    Ok(resp.body_mut().with_config().limit(20 * 1024 * 1024).read_to_vec()?)
}

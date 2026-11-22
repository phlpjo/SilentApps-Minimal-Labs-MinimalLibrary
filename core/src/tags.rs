use anyhow::{Context, Result};
use lofty::config::WriteOptions;
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::prelude::*;
use lofty::tag::Tag;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TagData {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub comment: Option<String>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    pub duration: f64,
    pub bitrate: Option<u32>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub has_cover: bool,
}

fn clean(s: Option<std::borrow::Cow<'_, str>>) -> Option<String> {
    s.map(|v| v.trim().trim_matches('\0').to_string())
        .filter(|v| !v.is_empty())
}

pub fn read(path: &Path) -> Result<TagData> {
    let tf =
        lofty::read_from_path(path).with_context(|| format!("Tags lesen: {}", path.display()))?;
    let p = tf.properties();
    let mut d = TagData {
        duration: p.duration().as_secs_f64(),
        bitrate: p.audio_bitrate().or(p.overall_bitrate()),
        sample_rate: p.sample_rate(),
        channels: p.channels().map(u32::from),
        ..Default::default()
    };
    let tag = tf.primary_tag().or_else(|| tf.first_tag());
    if let Some(t) = tag {
        d.title = clean(t.title());
        d.artist = clean(t.artist());
        d.album = clean(t.album());
        d.genre = clean(t.genre());
        d.comment = clean(t.comment());
        d.album_artist = t
            .get_string(ItemKey::AlbumArtist)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        d.year = t
            .get_string(ItemKey::RecordingDate)
            .or_else(|| t.get_string(ItemKey::Year))
            .or_else(|| t.get_string(ItemKey::OriginalReleaseDate))
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        d.track_no = t.track();
        d.disc_no = t.disk();
        d.bpm = t
            .get_string(ItemKey::Bpm)
            .or_else(|| t.get_string(ItemKey::IntegerBpm))
            .and_then(|s| s.trim().replace(',', ".").parse::<f64>().ok())
            .filter(|b| *b > 0.0);
        d.key = t
            .get_string(ItemKey::InitialKey)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        d.has_cover = !t.pictures().is_empty();
    }
    // Andere Tags (z. B. ID3v1 neben ID3v2) können ein Cover tragen
    if !d.has_cover {
        d.has_cover = tf.tags().iter().any(|t| !t.pictures().is_empty());
    }
    Ok(d)
}

/// Änderungen: `None` = unverändert, `Some("")` = Feld löschen.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct TagUpdate {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track_no: Option<String>,
    pub disc_no: Option<String>,
    pub comment: Option<String>,
    pub bpm: Option<String>,
    pub key: Option<String>,
}

impl TagUpdate {
    pub fn is_empty(&self) -> bool {
        [
            &self.title,
            &self.artist,
            &self.album,
            &self.album_artist,
            &self.genre,
            &self.year,
            &self.track_no,
            &self.disc_no,
            &self.comment,
            &self.bpm,
            &self.key,
        ]
        .iter()
        .all(|f| f.is_none())
    }
}

fn set(tag: &mut Tag, key: ItemKey, v: &Option<String>) {
    match v {
        None => {}
        Some(s) if s.trim().is_empty() => tag.remove_key(key),
        Some(s) => {
            tag.insert_text(key, s.trim().to_string());
        }
    }
}

fn with_tag<F: FnOnce(&mut Tag)>(path: &Path, f: F) -> Result<()> {
    let mut tf =
        lofty::read_from_path(path).with_context(|| format!("Datei lesen: {}", path.display()))?;
    let tt = tf.primary_tag_type();
    if tf.tag(tt).is_none() {
        tf.insert_tag(Tag::new(tt));
    }
    let tag = tf.tag_mut(tt).expect("Tag eben eingefügt");
    f(tag);
    tag.save_to_path(path, WriteOptions::default())
        .with_context(|| format!("Tags schreiben: {}", path.display()))?;
    Ok(())
}

pub fn write(path: &Path, u: &TagUpdate) -> Result<()> {
    if u.is_empty() {
        return Ok(());
    }
    with_tag(path, |tag| {
        set(tag, ItemKey::TrackTitle, &u.title);
        set(tag, ItemKey::TrackArtist, &u.artist);
        set(tag, ItemKey::AlbumTitle, &u.album);
        set(tag, ItemKey::AlbumArtist, &u.album_artist);
        set(tag, ItemKey::Genre, &u.genre);
        set(tag, ItemKey::Comment, &u.comment);
        set(tag, ItemKey::InitialKey, &u.key);
        if u.year.is_some() {
            tag.remove_key(ItemKey::Year);
            set(tag, ItemKey::RecordingDate, &u.year);
        }
        let num = |v: &Option<String>| -> Option<String> {
            v.as_ref().map(|s| {
                // "3/12" → "3"
                let n = s.split('/').next().unwrap_or("").trim();
                n.parse::<u32>().map(|x| x.to_string()).unwrap_or_default()
            })
        };
        set(tag, ItemKey::TrackNumber, &num(&u.track_no));
        set(tag, ItemKey::DiscNumber, &num(&u.disc_no));
        if let Some(b) = &u.bpm {
            let rounded = b
                .trim()
                .replace(',', ".")
                .parse::<f64>()
                .map(|x| format!("{}", x.round() as i64))
                .unwrap_or_default();
            tag.remove_key(ItemKey::Bpm);
            set(tag, ItemKey::IntegerBpm, &Some(rounded));
        }
    })
}

pub fn cover(path: &Path) -> Result<Option<(String, Vec<u8>)>> {
    let tf = lofty::read_from_path(path)?;
    for tag in tf.primary_tag().into_iter().chain(tf.tags().iter()) {
        let pics = tag.pictures();
        let pic = pics
            .iter()
            .find(|p| p.pic_type() == PictureType::CoverFront)
            .or(pics.first());
        if let Some(p) = pic {
            let mime = p
                .mime_type()
                .map(|m| m.as_str().to_string())
                .unwrap_or_else(|| sniff_mime(p.data()).to_string());
            return Ok(Some((mime, p.data().to_vec())));
        }
    }
    Ok(None)
}

fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        "image/png"
    } else if data.starts_with(b"GIF") {
        "image/gif"
    } else if data.len() > 12 && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

pub fn set_cover(path: &Path, data: Vec<u8>) -> Result<()> {
    let mime = match sniff_mime(&data) {
        "image/png" => MimeType::Png,
        "image/gif" => MimeType::Gif,
        _ => MimeType::Jpeg,
    };
    with_tag(path, move |tag| {
        tag.remove_picture_type(PictureType::CoverFront);
        tag.push_picture(
            Picture::unchecked(data)
                .pic_type(PictureType::CoverFront)
                .mime_type(mime)
                .build(),
        );
    })
}

pub fn remove_cover(path: &Path) -> Result<()> {
    with_tag(path, |tag| {
        while !tag.pictures().is_empty() {
            tag.remove_picture(0);
        }
    })
}

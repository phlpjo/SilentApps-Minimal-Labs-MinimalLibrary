//! Kern von Minimal Library: Bibliothek, Tags, Analyse, Duplikate, Sortieren, Playlisten, Export.
//! Ohne GUI-Abhängigkeit, damit alles testbar bleibt.

pub mod acoustid;
pub mod analysis;
pub mod cast;
pub mod db;
pub mod dirs;
pub mod duplicates;
pub mod export;
pub mod history;
pub mod lyrics;
pub mod musicbrainz;
pub mod organize;
pub mod paths;
pub mod pioneer;
pub mod playlists;
pub mod remote;
pub mod ripper;
pub mod scanner;
pub mod sync;
pub mod tags;
pub mod watch;
pub mod webcache;

/// Fortschritt: (erledigt, gesamt, Text)
pub type Progress<'a> = &'a (dyn Fn(usize, usize, &str) + Sync);

pub const AUDIO_EXTS: &[&str] = &[
    "mp3", "flac", "m4a", "mp4", "aac", "ogg", "oga", "opus", "wav", "aif", "aiff",
];

pub fn is_audio(path: &std::path::Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| AUDIO_EXTS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

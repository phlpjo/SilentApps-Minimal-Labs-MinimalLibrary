use crate::db::Track;
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Modes {
    pub metadata: bool,
    pub filename: bool,
    pub audio: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupGroup {
    pub track_ids: Vec<i64>,
    /// "metadata", "filename", "audio"
    pub reasons: Vec<String>,
    /// Audio-Ähnlichkeit 0..1, falls ermittelt
    pub audio_similarity: Option<f32>,
    /// Vorschlag, welcher Titel bleiben soll
    pub keep: i64,
}

struct Uf {
    p: Vec<usize>,
}
impl Uf {
    fn new(n: usize) -> Self {
        Uf {
            p: (0..n).collect(),
        }
    }
    fn find(&mut self, x: usize) -> usize {
        let mut r = x;
        while self.p[r] != r {
            r = self.p[r];
        }
        let mut x = x;
        while self.p[x] != r {
            let n = self.p[x];
            self.p[x] = r;
            x = n;
        }
        r
    }
    fn union(&mut self, a: usize, b: usize) {
        let (a, b) = (self.find(a), self.find(b));
        if a != b {
            self.p[b] = a;
        }
    }
}

/// Kleinbuchstaben, Zusätze wie "(Remastered 2011)" oder "feat. X" entfernen, nur Buchstaben/Ziffern.
pub fn normalize_text(s: &str) -> String {
    let mut s = s.to_lowercase();
    // Klammerzusätze mit Remaster/Explicit/Jahreszahl entfernen
    for (open, close) in [('(', ')'), ('[', ']')] {
        loop {
            let Some(a) = s.find(open) else { break };
            let Some(b_rel) = s[a..].find(close) else {
                break;
            };
            let b = a + b_rel;
            let inner = &s[a + 1..b];
            let drop = [
                "remaster",
                "explicit",
                "album version",
                "single version",
                "mono",
                "stereo",
                "digital",
            ]
            .iter()
            .any(|k| inner.contains(k))
                || (inner.trim().len() == 4 && inner.trim().chars().all(|c| c.is_ascii_digit()));
            if drop {
                s.replace_range(a..=b, " ");
            } else {
                // Klammern behalten, Inhalt bleibt (z. B. "(Radio Edit)" ≠ "(Extended Mix)")
                s.replace_range(a..=a, " ");
                s.replace_range(b..=b, " ");
            }
        }
    }
    for sep in [" feat. ", " feat ", " ft. ", " featuring "] {
        if let Some(i) = s.find(sep) {
            s.truncate(i);
        }
    }
    s.replace(" - remastered", " ")
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Dateiname ohne Endung, ohne Tracknummer vorne und ohne " (1)", " - Kopie" usw.
pub fn normalize_filename(path: &str) -> String {
    let stem = std::path::Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut s = stem.trim().to_string();
    // Tracknummer vorne: "01 ", "01. ", "01 - ", "1-02 "
    let trimmed = s.trim_start_matches(|c: char| {
        c.is_ascii_digit() || c == '-' || c == '.' || c == '_' || c == ' '
    });
    if trimmed.len() >= 3 && trimmed.len() < s.len() {
        s = trimmed.to_string();
    }
    for suffix in [" - kopie", " - copy", " kopie", " copy", "_copy"] {
        if let Some(i) = s.rfind(suffix) {
            s.truncate(i);
        }
    }
    // " (1)", "(2)", "_1"
    loop {
        let t = s.trim_end();
        if t.ends_with(')') {
            if let Some(i) = t.rfind('(') {
                if t[i + 1..t.len() - 1].chars().all(|c| c.is_ascii_digit()) && t.len() - i > 2 {
                    s = t[..i].to_string();
                    continue;
                }
            }
        }
        if let Some(i) = t.rfind('_') {
            if !t[i + 1..].is_empty()
                && t[i + 1..].len() <= 2
                && t[i + 1..].chars().all(|c| c.is_ascii_digit())
            {
                s = t[..i].to_string();
                continue;
            }
        }
        break;
    }
    normalize_text(&s)
}

/// Bit-Ähnlichkeit zweier Chromaprint-Fingerabdrücke mit Versatzsuche (±12 Elemente ≈ ±1,5 s).
pub fn fp_similarity(a: &[u32], b: &[u32]) -> f32 {
    let mut best = 0.0f32;
    for off in -12i32..=12 {
        let (aa, bb) = if off >= 0 {
            (&a[(off as usize).min(a.len())..], b)
        } else {
            (a, &b[((-off) as usize).min(b.len())..])
        };
        let n = aa.len().min(bb.len()).min(600);
        if n < 40 {
            continue;
        }
        let errors: u32 = aa[..n]
            .iter()
            .zip(&bb[..n])
            .map(|(x, y)| (x ^ y).count_ones())
            .sum();
        let sim = 1.0 - errors as f32 / (32.0 * n as f32);
        if sim > best {
            best = sim;
        }
    }
    best
}

pub const AUDIO_THRESHOLD: f32 = 0.86;

fn quality(t: &Track) -> (i64, i64, i64, i64) {
    let ext = std::path::Path::new(&t.path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let lossless = matches!(ext.as_str(), "flac" | "wav" | "aif" | "aiff") as i64;
    let tags = [&t.title, &t.artist, &t.album, &t.genre, &t.year]
        .iter()
        .filter(|x| x.is_some())
        .count() as i64
        + t.has_cover as i64;
    (lossless, t.bitrate.unwrap_or(0) as i64, tags, t.size)
}

/// Paare, die als „kein Duplikat“ markiert wurden (kleinere ID zuerst)
pub type Ignored = std::collections::HashSet<(i64, i64)>;

pub fn pair(a: i64, b: i64) -> (i64, i64) {
    (a.min(b), a.max(b))
}

pub fn find(
    tracks: &[Track],
    fps: &HashMap<i64, Vec<u32>>,
    modes: Modes,
    ignored: &Ignored,
    progress: crate::Progress,
) -> Vec<DupGroup> {
    find_scoped(tracks, fps, modes, ignored, &|_, _| true, progress)
}

/// Wie `find`, aber nur Paare (Index a, Index b), für die `scope` wahr ist, werden verglichen
/// (z. B. nicht über Musikordner hinweg).
pub fn find_scoped(
    tracks: &[Track],
    fps: &HashMap<i64, Vec<u32>>,
    modes: Modes,
    ignored: &Ignored,
    scope: &(dyn Fn(usize, usize) -> bool + Sync),
    progress: crate::Progress,
) -> Vec<DupGroup> {
    let n = tracks.len();
    let mut uf = Uf::new(n);
    let mut reasons: HashMap<(usize, usize), BTreeSet<&'static str>> = HashMap::new();
    let mut audio_sim: HashMap<(usize, usize), f32> = HashMap::new();
    let is_ignored =
        |a: usize, b: usize| !scope(a, b) || ignored.contains(&pair(tracks[a].id, tracks[b].id));
    let mut add = |uf: &mut Uf, a: usize, b: usize, why: &'static str| {
        if is_ignored(a, b) {
            return;
        }
        uf.union(a, b);
        reasons.entry((a.min(b), a.max(b))).or_default().insert(why);
    };
    let dur_close = |a: &Track, b: &Track, tol: f64| {
        a.duration <= 0.0 || b.duration <= 0.0 || (a.duration - b.duration).abs() <= tol
    };

    if modes.metadata {
        progress(0, 3, "Metadaten vergleichen …");
        let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, t) in tracks.iter().enumerate() {
            let (Some(ar), Some(ti)) = (&t.artist, &t.title) else {
                continue;
            };
            let (ar, ti) = (normalize_text(ar), normalize_text(ti));
            if ar.is_empty() || ti.is_empty() {
                continue;
            }
            by_key.entry(format!("{ar}|{ti}")).or_default().push(i);
        }
        for v in by_key.values() {
            for (x, &a) in v.iter().enumerate() {
                for &b in &v[x + 1..] {
                    if dur_close(&tracks[a], &tracks[b], 10.0) {
                        add(&mut uf, a, b, "metadata");
                    }
                }
            }
        }
    }

    if modes.filename {
        progress(1, 3, "Dateinamen vergleichen …");
        let mut names: Vec<(String, usize)> = tracks
            .iter()
            .enumerate()
            .map(|(i, t)| (normalize_filename(&t.path), i))
            .filter(|(s, _)| s.chars().count() >= 4)
            .collect();
        names.sort();
        for i in 0..names.len() {
            for j in i + 1..(i + 25).min(names.len()) {
                let (sa, a) = (&names[i].0, names[i].1);
                let (sb, b) = (&names[j].0, names[j].1);
                let same = sa == sb || strsim::normalized_levenshtein(sa, sb) >= 0.92;
                if same && dur_close(&tracks[a], &tracks[b], 10.0) {
                    add(&mut uf, a, b, "filename");
                }
            }
        }
    }

    if modes.audio {
        progress(2, 3, "Audio-Fingerabdrücke vergleichen …");
        let mut idx: Vec<usize> = (0..n)
            .filter(|&i| fps.get(&tracks[i].id).is_some_and(|f| f.len() >= 40))
            .collect();
        idx.sort_by(|&a, &b| {
            tracks[a]
                .duration
                .partial_cmp(&tracks[b].duration)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let pairs: Vec<(usize, usize, f32)> = (0..idx.len())
            .into_par_iter()
            .flat_map_iter(|x| {
                let a = idx[x];
                let fa = &fps[&tracks[a].id];
                let mut out = Vec::new();
                for &b in &idx[x + 1..] {
                    if tracks[b].duration - tracks[a].duration > 8.0 {
                        break;
                    }
                    if !scope(a, b) {
                        continue;
                    }
                    let sim = fp_similarity(fa, &fps[&tracks[b].id]);
                    if sim >= AUDIO_THRESHOLD {
                        out.push((a, b, sim));
                    }
                }
                out
            })
            .collect();
        for (a, b, s) in pairs {
            if is_ignored(a, b) {
                continue;
            }
            add(&mut uf, a, b, "audio");
            audio_sim.insert((a.min(b), a.max(b)), s);
        }
    }
    progress(3, 3, "Gruppen bilden …");

    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let r = uf.find(i);
        groups.entry(r).or_default().push(i);
    }
    let mut out: Vec<DupGroup> = groups
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|g| {
            let mut rs: BTreeSet<&str> = BTreeSet::new();
            let mut sim: Option<f32> = None;
            for (x, &a) in g.iter().enumerate() {
                for &b in &g[x + 1..] {
                    let k = (a.min(b), a.max(b));
                    if let Some(r) = reasons.get(&k) {
                        rs.extend(r.iter());
                    }
                    if let Some(&s) = audio_sim.get(&k) {
                        sim = Some(sim.map_or(s, |m: f32| m.min(s)));
                    }
                }
            }
            let keep = g
                .iter()
                .max_by_key(|&&i| quality(&tracks[i]))
                .map(|&i| tracks[i].id)
                .unwrap_or(0);
            DupGroup {
                track_ids: g.iter().map(|&i| tracks[i].id).collect(),
                reasons: rs.into_iter().map(String::from).collect(),
                audio_similarity: sim,
                keep,
            }
        })
        .collect();
    out.sort_by_key(|g| std::cmp::Reverse(g.reasons.len()));
    out
}

/// Treffer beim Kopieren/Verschieben: Quell-Titel und gleiche Titel am Ziel
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DupHit {
    pub track_id: i64,
    pub matches: Vec<i64>,
    pub reasons: Vec<String>,
    pub audio_similarity: Option<f32>,
}

/// Haben die `src`-Titel schon Gegenstücke in `cand`? Verglichen wird nur Quelle ↔ Ziel
/// (Metadaten, Dateiname, Audio). IDs in `cand` dürfen nicht mit `src` kollidieren.
pub fn check_against(
    src: &[Track],
    cand: &[Track],
    fps: &HashMap<i64, Vec<u32>>,
    ignored: &Ignored,
) -> Vec<DupHit> {
    let ns = src.len();
    let all: Vec<Track> = src.iter().chain(cand.iter()).cloned().collect();
    let modes = Modes {
        metadata: true,
        filename: true,
        audio: true,
    };
    let groups = find_scoped(
        &all,
        fps,
        modes,
        ignored,
        &|a, b| (a < ns) != (b < ns),
        &|_, _, _| {},
    );
    let src_ids: std::collections::HashSet<i64> = src.iter().map(|t| t.id).collect();
    let mut out = Vec::new();
    for g in groups {
        let matches: Vec<i64> = g
            .track_ids
            .iter()
            .copied()
            .filter(|id| !src_ids.contains(id))
            .collect();
        if matches.is_empty() {
            continue;
        }
        for id in g.track_ids.iter().filter(|id| src_ids.contains(id)) {
            out.push(DupHit {
                track_id: *id,
                matches: matches.clone(),
                reasons: g.reasons.clone(),
                audio_similarity: g.audio_similarity,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalize() {
        assert_eq!(normalize_text("Hey Jude (Remastered 2015)"), "hey jude");
        assert_eq!(normalize_text("Song (Radio Edit)"), "song radio edit");
        assert_eq!(normalize_text("Track feat. Someone"), "track");
        assert_eq!(normalize_filename("C:/m/01 - Hey Jude (1).mp3"), "hey jude");
        assert_eq!(normalize_filename("/m/Hey Jude - Kopie.flac"), "hey jude");
        assert_eq!(normalize_filename("/m/1999.mp3"), "1999");
    }

    #[test]
    fn check_against_only_compares_source_with_target() {
        let mk = |id: i64, path: &str, artist: &str, title: &str| Track {
            id,
            path: path.into(),
            artist: Some(artist.into()),
            title: Some(title.into()),
            duration: 200.0,
            ..Default::default()
        };
        let src = vec![
            mk(1, "/a/Band - Lied.mp3", "Band", "Lied"),
            mk(2, "/a/Band - Lied (2).mp3", "Band", "Lied"),
            mk(3, "/a/x.mp3", "Andere", "Sache"),
        ];
        let cand = vec![
            mk(10, "/b/Band - Lied.flac", "Band", "Lied"),
            mk(11, "/b/y.mp3", "Wer", "Was"),
        ];
        let hits = check_against(&src, &cand, &HashMap::new(), &Ignored::default());
        let mut ids: Vec<i64> = hits.iter().map(|h| h.track_id).collect();
        ids.sort();
        assert_eq!(ids, vec![1, 2]);
        assert!(hits.iter().all(|h| h.matches == vec![10]));
    }
}

//! Analysedateien der Player (ANLZ0000.DAT / .EXT) schreiben und für Tests wieder lesen.
//!
//! Aufbau nach der Formatdokumentation von pyrekordbox (MIT, © Dylan Jones):
//! Datei-Kopf „PMAI“, danach Abschnitte (Tags) mit 4-Zeichen-Kennung, Kopflänge und Gesamtlänge,
//! alle Zahlen Big Endian. Feste Konstanten stammen aus pyrekordbox/anlz/structs.py.
//! Unklare Felder sind mit „OFFEN:“ markiert und in der README/Zusammenfassung aufgeführt.

use super::{Cue, Grid};
use anyhow::{Result, anyhow, bail};

/// Spalten der Vorschau-Wellenformen
pub const PREVIEW_COLS: usize = 400;
/// OFFEN: Spaltenzahl von PWV2 (Mini-Vorschau, ältere Player) ist in der Referenz nicht angegeben.
pub const TINY_COLS: usize = 100;
pub const COLOR_PREVIEW_COLS: usize = 1200;

struct W(Vec<u8>);

impl W {
    fn new() -> Self {
        W(Vec::new())
    }
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u16(&mut self, v: u16) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_be_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
    fn zeros(&mut self, n: usize) -> &mut Self {
        self.0.resize(self.0.len() + n, 0);
        self
    }
}

/// Ein Abschnitt: Kennung, Kopflänge, Gesamtlänge, dann `head` (Rest des Kopfs) und `body`
fn tag(kind: &[u8; 4], head: &[u8], body: &[u8]) -> Vec<u8> {
    let len_header = 12 + head.len();
    let mut w = W::new();
    w.bytes(kind).u32(len_header as u32).u32((len_header + body.len()) as u32).bytes(head).bytes(body);
    w.0
}

fn utf16be(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|c| c.to_be_bytes()).collect()
}

/// PPTH: Pfad der Audiodatei auf dem Gerät (UTF-16 BE mit abschließender 0)
fn ppth(path: &str) -> Vec<u8> {
    let mut p = utf16be(path);
    p.extend_from_slice(&[0, 0]);
    let mut head = W::new();
    head.u32(p.len() as u32);
    tag(b"PPTH", &head.0, &p)
}

/// PVBR: Sprungtabelle für VBR-MP3s. OFFEN: Inhalt nicht dokumentiert – laut Referenz meist 0.
fn pvbr() -> Vec<u8> {
    let mut head = W::new();
    head.u32(0);
    let mut body = W::new();
    body.zeros(400 * 4 + 4);
    tag(b"PVBR", &head.0, &body.0)
}

/// PQTZ: Beatgrid – je Schlag Taktposition (1–4), Tempo×100, Zeit in ms
fn pqtz(beats: &[(f64, u8)], bpm: f64) -> Vec<u8> {
    let mut head = W::new();
    head.u32(0).u32(0x0008_0000).u32(beats.len() as u32);
    let mut body = W::new();
    let tempo = (bpm * 100.0).round() as u16;
    for (t, b) in beats {
        body.u16(*b as u16).u16(tempo).u32(t.round().max(0.0) as u32);
    }
    tag(b"PQTZ", &head.0, &body.0)
}

/// Vorschau-Byte: Höhe 0–31 (5 Bit) + „Weißanteil“ 0–7 (3 Bit)
fn mono_byte(height: f32, white: f32) -> u8 {
    let h = (height.clamp(0.0, 1.0) * 31.0).round() as u8;
    let w = (white.clamp(0.0, 1.0) * 7.0).round() as u8;
    h | (w << 5)
}

/// Rahmen (150/s, je [Spitze, Bass, Mitten, Höhen]) auf `cols` Spalten verdichten
fn columns(frames: &[u8], cols: usize) -> Vec<[f32; 4]> {
    let n = frames.len() / 4;
    let mut out = vec![[0.0f32; 4]; cols];
    if n == 0 {
        return out;
    }
    for (c, o) in out.iter_mut().enumerate() {
        let s = c * n / cols;
        let e = ((c + 1) * n / cols).max(s + 1).min(n);
        for i in s..e {
            for k in 0..4 {
                let v = frames[i * 4 + k] as f32 / 255.0;
                if v > o[k] {
                    o[k] = v;
                }
            }
        }
    }
    out
}

fn whiteness(f: &[f32; 4]) -> f32 {
    let sum = f[1] + f[2] + f[3];
    if sum <= 0.0 { 0.0 } else { (f[3] + 0.5 * f[2]) / sum }
}

/// PWAV (400 Spalten) bzw. PWV2 (Mini-Vorschau)
fn preview(kind: &[u8; 4], frames: &[u8], cols: usize) -> Vec<u8> {
    let data: Vec<u8> = columns(frames, cols).iter().map(|f| mono_byte(f[0], whiteness(f))).collect();
    let mut head = W::new();
    head.u32(data.len() as u32).u32(0x0001_0000);
    tag(kind, &head.0, &data)
}

/// PWV3: Detail-Wellenform, 150 Werte/s, ein Byte wie PWAV
fn pwv3(frames: &[u8]) -> Vec<u8> {
    let data: Vec<u8> = frames
        .chunks_exact(4)
        .map(|f| {
            let v = [f[0] as f32 / 255.0, f[1] as f32 / 255.0, f[2] as f32 / 255.0, f[3] as f32 / 255.0];
            mono_byte(v[0], whiteness(&v))
        })
        .collect();
    let mut head = W::new();
    head.u32(1).u32(data.len() as u32).u32(0x0096_0000);
    tag(b"PWV3", &head.0, &data)
}

/// PWV4: farbige Vorschau, 1200 Spalten × 6 Byte.
/// Bedeutung laut pyrekordbox-Leser: [0] unbekannt, [1] Helligkeit, [2] umgekehrte Intensität (blaue
/// Ansicht), [3] Rot, [4] Grün, [5] Blau/Höhe der vorderen Welle – jeweils 0–127.
/// OFFEN: Byte 0 und das Kopffeld „unknown“ sind nicht dokumentiert (hier 0).
fn pwv4(frames: &[u8]) -> Vec<u8> {
    let mut body = W::new();
    for f in columns(frames, COLOR_PREVIEW_COLS) {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 127.0).round() as u8;
        body.u8(0).u8(127).u8(q(1.0 - f[0])).u8(q(f[1] * f[0].max(0.2))).u8(q(f[2] * f[0].max(0.2))).u8(q(f[0]));
    }
    let mut head = W::new();
    head.u32(6).u32(COLOR_PREVIEW_COLS as u32).u32(0);
    tag(b"PWV4", &head.0, &body.0)
}

/// PWV5: farbige Detail-Wellenform, 150 Werte/s, 16 Bit: Rot(15–13) Grün(12–10) Blau(9–7) Höhe(6–2)
pub fn color_word(peak: f32, lo: f32, mid: f32, hi: f32) -> u16 {
    let c = |v: f32| (v.clamp(0.0, 1.0) * 7.0).round() as u16;
    let h = (peak.clamp(0.0, 1.0) * 31.0).round() as u16;
    (c(lo) << 13) | (c(mid) << 10) | (c(hi) << 7) | (h << 2)
}

fn pwv5(frames: &[u8]) -> Vec<u8> {
    let mut body = W::new();
    let mut n = 0u32;
    for f in frames.chunks_exact(4) {
        body.u16(color_word(f[0] as f32 / 255.0, f[1] as f32 / 255.0, f[2] as f32 / 255.0, f[3] as f32 / 255.0));
        n += 1;
    }
    let mut head = W::new();
    // OFFEN: Kopffeld laut Doku „960305“ – Bedeutung unbekannt
    head.u32(2).u32(n).u32(0x0096_0305);
    tag(b"PWV5", &head.0, &body.0)
}

/// PCOB: Cue-Liste (Typ 0 = Memory Cues, 1 = Hot Cues), Einträge „PCPT“ zu 56 Byte
fn pcob(hot: bool, cues: &[&Cue]) -> Vec<u8> {
    let mut head = W::new();
    // OFFEN: „unk“ und „memory_count“ nicht dokumentiert (hier 0)
    head.u32(hot as u32).u16(0).u16(cues.len() as u16).u32(0);
    let mut body = W::new();
    let n = cues.len();
    for (i, c) in cues.iter().enumerate() {
        let is_loop = c.end_ms.is_some();
        // Verkettung als Liste: vorheriger/nächster Eintrag, 0xFFFF am Rand
        let prev = if i == 0 { 0xFFFF } else { (i - 1) as u16 };
        let next = if i + 1 == n { 0xFFFF } else { (i + 1) as u16 };
        body.bytes(b"PCPT")
            .u32(0x1c)
            .u32(0x38)
            .u32(if hot { c.kind as u32 } else { 0 })
            .u32(0) // Status: 0 = normal (4 = aktive Schleife)
            .u32(0x0001_0000)
            .u16(prev)
            .u16(next)
            .u8(if is_loop { 2 } else { 1 })
            .u8(0)
            .u16(1000)
            .u32(c.start_ms.round().max(0.0) as u32)
            .u32(c.end_ms.map(|e| e.round().max(0.0) as u32).unwrap_or(0xFFFF_FFFF))
            .zeros(16);
    }
    tag(b"PCOB", &head.0, &body.0)
}

/// PCO2: erweiterte Cue-Liste mit Farbe und Kommentar (Einträge „PCP2“)
/// OFFEN: Kopflänge eines PCP2-Eintrags (hier 16) und das zweite Kopffeld von PCO2 sind nicht dokumentiert.
fn pco2(hot: bool, cues: &[&Cue]) -> Vec<u8> {
    let mut head = W::new();
    head.u32(hot as u32).u16(cues.len() as u16).u16(0);
    let mut body = W::new();
    for c in cues {
        let comment = c.comment.as_deref().filter(|s| !s.is_empty()).map(|s| {
            let mut v = utf16be(s);
            v.extend_from_slice(&[0, 0]);
            v
        });
        let len_comment = comment.as_ref().map(|v| v.len()).unwrap_or(0);
        let len_entry = 48 + len_comment;
        let (r, g, b) = color_rgb(c.color.unwrap_or(0));
        body.bytes(b"PCP2")
            .u32(16)
            .u32(len_entry as u32)
            .u32(if hot { c.kind as u32 } else { 0 })
            .u8(if c.end_ms.is_some() { 2 } else { 1 })
            .zeros(3)
            .u32(c.start_ms.round().max(0.0) as u32)
            .u32(c.end_ms.map(|e| e.round().max(0.0) as u32).unwrap_or(0xFFFF_FFFF))
            .u8(c.color.unwrap_or(0))
            .zeros(7)
            .u16(0)
            .u16(0)
            .u32(len_comment as u32);
        if let Some(v) = &comment {
            body.bytes(v);
        }
        body.u8(c.color.unwrap_or(0)).u8(r).u8(g).u8(b);
    }
    tag(b"PCO2", &head.0, &body.0)
}

/// Farbtabelle für Hot Cues (Index 0 = keine). OFFEN: Zuordnung der Player-Farbindizes zu RGB
/// ist in der Referenz nicht dokumentiert – hier eine eigene Tabelle mit 8 Grundfarben.
pub const CUE_COLORS: [(u8, u8, u8); 9] = [
    (0, 0, 0),
    (0xE6, 0x28, 0x28), // Rot
    (0xFF, 0x8C, 0x00), // Orange
    (0xFF, 0xDC, 0x00), // Gelb
    (0x28, 0xE2, 0x14), // Grün
    (0x10, 0xB1, 0x76), // Türkis
    (0x30, 0x5A, 0xFF), // Blau
    (0x88, 0x32, 0xFF), // Violett
    (0xFF, 0x34, 0xB5), // Pink
];

pub fn color_rgb(i: u8) -> (u8, u8, u8) {
    CUE_COLORS.get(i as usize).copied().unwrap_or((0, 0, 0))
}

fn file(tags: &[Vec<u8>]) -> Vec<u8> {
    let body: usize = tags.iter().map(|t| t.len()).sum();
    let mut w = W::new();
    // OFFEN: die vier Felder nach len_file sind nicht dokumentiert (hier 0)
    w.bytes(b"PMAI").u32(28).u32((28 + body) as u32).zeros(16);
    for t in tags {
        w.bytes(t);
    }
    w.0
}

/// Alles, was für die Analysedateien eines Titels nötig ist
pub struct TrackAnalysis<'a> {
    /// Pfad der Audiodatei auf dem Gerät, z. B. "/Contents/Künstler/Titel.mp3"
    pub device_path: &'a str,
    pub duration_ms: f64,
    pub grid: Option<Grid>,
    pub frames: &'a [u8],
    pub cues: &'a [Cue],
}

fn split<'a>(cues: &'a [Cue]) -> (Vec<&'a Cue>, Vec<&'a Cue>) {
    let mut mem: Vec<&Cue> = cues.iter().filter(|c| c.kind == 0).collect();
    let mut hot: Vec<&Cue> = cues.iter().filter(|c| c.kind > 0).collect();
    mem.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
    hot.sort_by_key(|c| c.kind);
    (mem, hot)
}

/// ANLZ0000.DAT
pub fn build_dat(a: &TrackAnalysis) -> Vec<u8> {
    let (mem, hot) = split(a.cues);
    let mut tags = vec![ppth(a.device_path), pvbr()];
    if let Some(g) = a.grid {
        tags.push(pqtz(&g.beats(a.duration_ms), g.bpm));
    }
    tags.push(preview(b"PWAV", a.frames, PREVIEW_COLS));
    tags.push(preview(b"PWV2", a.frames, TINY_COLS));
    tags.push(pcob(false, &mem));
    tags.push(pcob(true, &hot));
    file(&tags)
}

/// ANLZ0000.EXT
pub fn build_ext(a: &TrackAnalysis) -> Vec<u8> {
    let (mem, hot) = split(a.cues);
    let tags = vec![
        ppth(a.device_path),
        pcob(false, &mem),
        pcob(true, &hot),
        pco2(false, &mem),
        pco2(true, &hot),
        pwv3(a.frames),
        pwv4(a.frames),
        pwv5(a.frames),
    ];
    file(&tags)
}

// ---------- Lesen (für Tests und zur Kontrolle) ----------

#[derive(Debug, Default)]
pub struct Parsed {
    pub tags: Vec<String>,
    pub path: String,
    /// (Taktposition, Tempo×100, Zeit ms)
    pub beats: Vec<(u16, u16, u32)>,
    /// (Hot-Cue-Nummer, Zeit ms, Schleifenende ms)
    pub cues: Vec<(u32, u32, Option<u32>)>,
    pub preview: Vec<u8>,
    pub detail: Vec<u8>,
    pub color_detail: Vec<u16>,
    pub comments: Vec<String>,
}

fn be32(b: &[u8], i: usize) -> Result<u32> {
    b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or_else(|| anyhow!("zu kurz"))
}
fn be16(b: &[u8], i: usize) -> Result<u16> {
    b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or_else(|| anyhow!("zu kurz"))
}

pub fn parse(data: &[u8]) -> Result<Parsed> {
    if data.get(0..4) != Some(b"PMAI") {
        bail!("kein PMAI-Kopf");
    }
    let len_header = be32(data, 4)? as usize;
    if be32(data, 8)? as usize != data.len() {
        bail!("len_file passt nicht");
    }
    let mut p = Parsed::default();
    let mut i = len_header;
    while i < data.len() {
        let kind = String::from_utf8_lossy(&data[i..i + 4]).to_string();
        let lh = be32(data, i + 4)? as usize;
        let lt = be32(data, i + 8)? as usize;
        if lt < lh || i + lt > data.len() {
            bail!("Abschnitt {kind} hat ungültige Länge");
        }
        let t = &data[i..i + lt];
        match kind.as_str() {
            "PPTH" => {
                let n = be32(t, 12)? as usize;
                let s: Vec<u16> = t[16..16 + n - 2].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                p.path = String::from_utf16_lossy(&s);
            }
            "PQTZ" => {
                let n = be32(t, 20)? as usize;
                for k in 0..n {
                    let o = 24 + k * 8;
                    p.beats.push((be16(t, o)?, be16(t, o + 2)?, be32(t, o + 4)?));
                }
            }
            "PCOB" => {
                let n = be16(t, 18)? as usize;
                for k in 0..n {
                    let o = 24 + k * 56;
                    if &t[o..o + 4] != b"PCPT" || be32(t, o + 8)? != 56 {
                        bail!("PCPT-Eintrag defekt");
                    }
                    let lt2 = be32(t, o + 36)?;
                    p.cues.push((be32(t, o + 12)?, be32(t, o + 32)?, (lt2 != 0xFFFF_FFFF).then_some(lt2)));
                }
            }
            "PCO2" => {
                let n = be16(t, 16)? as usize;
                let mut o = 20;
                for _ in 0..n {
                    let len_entry = be32(t, o + 8)? as usize;
                    let len_c = be32(t, o + 40)? as usize;
                    if len_c >= 2 {
                        let s: Vec<u16> = t[o + 44..o + 44 + len_c - 2].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                        p.comments.push(String::from_utf16_lossy(&s));
                    }
                    o += len_entry;
                }
            }
            "PWAV" => p.preview = t[lh..].to_vec(),
            "PWV3" => p.detail = t[lh..].to_vec(),
            "PWV5" => p.color_detail = t[lh..].chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect(),
            _ => {}
        }
        p.tags.push(kind);
        i += lt;
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_dat_ext() {
        // 10 s Titel, 150 Rahmen/s
        let mut frames = Vec::new();
        for i in 0..1500u32 {
            frames.extend_from_slice(&[(i % 256) as u8, 200, 100, 50]);
        }
        let cues = vec![
            Cue { id: 1, kind: 0, start_ms: 1000.0, end_ms: None, color: None, comment: None },
            Cue { id: 2, kind: 1, start_ms: 2500.4, end_ms: None, color: Some(3), comment: Some("Drop ä".into()) },
            Cue { id: 3, kind: 3, start_ms: 4000.0, end_ms: Some(6000.0), color: Some(6), comment: None },
        ];
        let a = TrackAnalysis {
            device_path: "/Contents/Künstler/Titel.mp3",
            duration_ms: 10_000.0,
            grid: Some(Grid { bpm: 128.0, downbeat_ms: 120.0, edited: false }),
            frames: &frames,
            cues: &cues,
        };
        let dat = parse(&build_dat(&a)).unwrap();
        assert_eq!(dat.tags, ["PPTH", "PVBR", "PQTZ", "PWAV", "PWV2", "PCOB", "PCOB"]);
        assert_eq!(dat.path, "/Contents/Künstler/Titel.mp3");
        assert_eq!(dat.beats[0], (1, 12800, 120));
        assert_eq!(dat.beats[1].0, 2);
        assert_eq!(dat.beats.len(), 22);
        assert_eq!(dat.preview.len(), PREVIEW_COLS);
        assert_eq!(dat.cues, vec![(0, 1000, None), (1, 2500, None), (3, 4000, Some(6000))]);

        let ext = parse(&build_ext(&a)).unwrap();
        assert_eq!(ext.tags, ["PPTH", "PCOB", "PCOB", "PCO2", "PCO2", "PWV3", "PWV4", "PWV5"]);
        assert_eq!(ext.detail.len(), 1500);
        assert_eq!(ext.color_detail.len(), 1500);
        assert_eq!(ext.comments, vec!["Drop ä".to_string()]);
        // Farbe/Höhe kodieren wie dokumentiert
        let w = color_word(1.0, 1.0, 0.0, 0.0);
        assert_eq!((w & 0xE000) >> 13, 7);
        assert_eq!((w & 0x7C) >> 2, 31);
    }
}

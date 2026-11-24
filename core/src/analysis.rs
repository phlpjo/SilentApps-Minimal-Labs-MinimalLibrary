//! Ein Dekodier-Durchlauf pro Datei liefert: Wellenform, Audio-Fingerabdruck, BPM und Tonart.

use crate::{Progress, db};
use anyhow::{Context, Result, anyhow};
use rayon::prelude::*;
use rusqlite::{Connection, params};
use rustfft::{FftPlanner, num_complex::Complex};
use rusty_chromaprint::{Configuration, Fingerprinter};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymErr;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// Sperre für Dateien auf Cloud-Laufwerken (nacheinander statt parallel)
static CLOUD_ONE: Mutex<()> = Mutex::new(());

/// Anzahl Wellenform-Punkte pro Titel (je Punkt: Spitze + RMS als u8)
pub const PEAK_BINS: usize = 1000;
const FP_SECONDS: usize = 120;
const TARGET_RATE: u32 = 11025;

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub duration: f64,
    /// Nur wenn der Pioneer-Export eingeschaltet ist: Beatgrid + Wellenform im Pioneer-Raster
    pub pioneer: Option<PioneerData>,
    /// [spitze0, rms0, spitze1, rms1, …]
    pub peaks: Vec<u8>,
    pub fingerprint: Vec<u32>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
    /// Obere Grenzfrequenz in Hz (Tiefpass des Encoders) – bei vollem Frequenzgang die halbe Abtastrate
    pub cutoff: Option<u32>,
}

/// Zusatzdaten für den Pioneer-Export (Einstellung „Pioneer-Export“)
#[derive(Debug, Clone, Default)]
pub struct PioneerData {
    /// Tempo mit 2 Nachkommastellen
    pub bpm: f64,
    /// Erster Downbeat (Taktschlag 1) in ms ab Titelanfang
    pub downbeat_ms: f64,
    /// 150 Werte pro Sekunde, je 4 Byte: Spitze, Bass, Mitten, Höhen (0–255)
    pub frames: Vec<u8>,
}

/// Wellenform-Raster der Player: 150 Werte pro Sekunde (halbe CD-Frames)
pub const PIONEER_FPS: f64 = 150.0;

/// Frequenzbänder für die farbige Wellenform (einfache Einpol-Filter)
struct Bands {
    step: f64,
    pos: f64,
    lp1: f32,
    lp2: f32,
    a1: f32,
    a2: f32,
    peak: f32,
    lo: f64,
    mid: f64,
    hi: f64,
    n: usize,
    out: Vec<[f32; 4]>,
}

impl Bands {
    fn new(rate: u32) -> Self {
        let a = |fc: f32| 1.0 - (-2.0 * std::f32::consts::PI * fc / rate as f32).exp();
        Bands {
            step: rate as f64 / PIONEER_FPS,
            pos: 0.0,
            lp1: 0.0,
            lp2: 0.0,
            a1: a(200.0),
            a2: a(2500.0),
            peak: 0.0,
            lo: 0.0,
            mid: 0.0,
            hi: 0.0,
            n: 0,
            out: Vec::new(),
        }
    }

    fn push(&mut self, m: f32) {
        self.lp1 += self.a1 * (m - self.lp1);
        self.lp2 += self.a2 * (m - self.lp2);
        let lo = self.lp1;
        let mid = self.lp2 - self.lp1;
        let hi = m - self.lp2;
        self.peak = self.peak.max(m.abs());
        self.lo += (lo * lo) as f64;
        self.mid += (mid * mid) as f64;
        self.hi += (hi * hi) as f64;
        self.n += 1;
        self.pos += 1.0;
        if self.pos >= self.step {
            self.pos -= self.step;
            self.flush();
        }
    }

    fn flush(&mut self) {
        if self.n == 0 {
            return;
        }
        let n = self.n as f64;
        self.out.push([
            self.peak,
            (self.lo / n).sqrt() as f32,
            (self.mid / n).sqrt() as f32,
            (self.hi / n).sqrt() as f32,
        ]);
        self.peak = 0.0;
        self.lo = 0.0;
        self.mid = 0.0;
        self.hi = 0.0;
        self.n = 0;
    }

    /// Auf 0–255 normieren (Spitze am Titelmaximum, Bänder gemeinsam)
    fn finish(mut self) -> Vec<u8> {
        self.flush();
        let pk = self.out.iter().map(|f| f[0]).fold(1e-6f32, f32::max);
        let bm = self
            .out
            .iter()
            .map(|f| f[1].max(f[2]).max(f[3]))
            .fold(1e-6f32, f32::max);
        let mut v = Vec::with_capacity(self.out.len() * 4);
        for f in &self.out {
            v.push(((f[0] / pk).min(1.0) * 255.0).round() as u8);
            for b in &f[1..] {
                v.push(((b / bm).min(1.0).sqrt() * 255.0).round() as u8);
            }
        }
        v
    }
}

struct Accum {
    rate: u32,
    bands: Option<Bands>,
    want_pioneer: bool,
    decim: usize,
    block: usize,
    // 10-ms-Blöcke
    b_max: f32,
    b_sq: f64,
    b_n: usize,
    blocks_max: Vec<f32>,
    blocks_rms: Vec<f32>,
    // dezimiertes Mono-Signal
    d_acc: f32,
    d_n: usize,
    low: Vec<f32>,
    // Fingerabdruck
    fp: Option<Fingerprinter>,
    fp_buf: Vec<i16>,
    fp_frames: usize,
    frames: usize,
    spec: Spectrum,
}

/// Mittleres Leistungsspektrum aus Stichproben (alle ~0,5 s ein Fenster) → Grenzfrequenz
const SPEC_N: usize = 4096;
struct Spectrum {
    buf: Vec<f32>,
    skip: usize,
    hop: usize,
    power: Vec<f64>,
    frames: usize,
    fft: Option<std::sync::Arc<dyn rustfft::Fft<f32>>>,
}

impl Spectrum {
    fn new() -> Self {
        Spectrum {
            buf: Vec::with_capacity(SPEC_N),
            skip: 0,
            hop: 0,
            power: vec![0.0; SPEC_N / 2 + 1],
            frames: 0,
            fft: None,
        }
    }

    fn init(&mut self, rate: u32) {
        self.hop = rate as usize / 2;
        self.fft = Some(FftPlanner::new().plan_fft_forward(SPEC_N));
    }

    fn push(&mut self, m: f32) {
        if self.skip > 0 {
            self.skip -= 1;
            return;
        }
        self.buf.push(m);
        if self.buf.len() < SPEC_N {
            return;
        }
        let rms = (self.buf.iter().map(|x| x * x).sum::<f32>() / SPEC_N as f32).sqrt();
        if rms > 1e-3 {
            if let Some(fft) = &self.fft {
                let mut c: Vec<Complex<f32>> = self
                    .buf
                    .iter()
                    .enumerate()
                    .map(|(i, &x)| {
                        let w = 0.5
                            - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / SPEC_N as f32).cos();
                        Complex::new(x * w, 0.0)
                    })
                    .collect();
                fft.process(&mut c);
                for (p, v) in self.power.iter_mut().zip(&c) {
                    *p += v.norm_sqr() as f64;
                }
                self.frames += 1;
            }
        }
        self.buf.clear();
        self.skip = self.hop;
    }

    /// Höchste Frequenz mit nennenswertem Inhalt, wenn darüber ein steiler Abfall folgt (Encoder-Tiefpass)
    fn cutoff(&self, rate: u32) -> Option<u32> {
        if self.frames < 4 || rate < 32000 {
            return None;
        }
        let hz = rate as f64 / SPEC_N as f64;
        let nyq = rate as f64 / 2.0;
        // 100-Hz-Bänder in dB
        let band = 100.0;
        let nb = (nyq / band) as usize;
        let db: Vec<f64> = (0..nb)
            .map(|b| {
                let (lo, hi) = (
                    (b as f64 * band / hz) as usize,
                    (((b + 1) as f64 * band) / hz) as usize,
                );
                let hi = hi.min(self.power.len()).max(lo + 1);
                let m =
                    self.power[lo..hi].iter().sum::<f64>() / (hi - lo) as f64 / self.frames as f64;
                10.0 * (m + 1e-20).log10()
            })
            .collect();
        let at = |f: f64| (f / band) as usize;
        let refl =
            db[at(1000.0)..at(5000.0)].iter().sum::<f64>() / (at(5000.0) - at(1000.0)) as f64;
        // relativ zum Mittenpegel
        let rel: Vec<f64> = db.iter().map(|d| d - refl).collect();
        let mean = |a: usize, b: usize| rel[a..=b].iter().sum::<f64>() / (b - a + 1) as f64;
        let top = (nb - 1).min(at(21900.0));
        // Kante = steiler Abfall: 0,7 kHz unterhalb klar hörbarer Inhalt, 0,2–0,8 kHz oberhalb mindestens 18 dB leiser.
        // Die tiefste solche Kante zählt – Rauschen/Dither oder ein späterer 320er-Tiefpass darüber ändern nichts.
        let drop = |c: usize| mean(c - 6, c) - mean(c + 2, (c + 7).min(top));
        let first = (at(10000.0)..top.saturating_sub(4))
            .find(|&c| drop(c) >= 18.0 && mean(c - 6, c) > -65.0);
        match first {
            Some(c) => {
                // innerhalb der Kante die steilste Stelle nehmen
                let end = (c + 10).min(top.saturating_sub(4));
                let best = (c..=end)
                    .filter(|&k| mean(k - 6, k) > -65.0)
                    .max_by(|&x, &y| drop(x).total_cmp(&drop(y)))
                    .unwrap_or(c);
                Some(((best + 1) as f64 * band) as u32)
            }
            None => Some(nyq as u32), // voller Frequenzgang
        }
    }
}

/// Geschätzte echte MP3-/AAC-Qualität aus der Grenzfrequenz (typische Encoder-Tiefpässe)
pub fn kbps_from_cutoff(cutoff: u32) -> u32 {
    match cutoff {
        0..=11_499 => 64,
        11_500..=14_499 => 96,
        14_500..=17_299 => 128,
        17_300..=18_199 => 160,
        18_200..=19_199 => 192,
        19_200..=19_799 => 256,
        _ => 320,
    }
}

impl Accum {
    fn new() -> Self {
        Accum {
            rate: 0,
            bands: None,
            want_pioneer: false,
            decim: 1,
            block: 441,
            b_max: 0.0,
            b_sq: 0.0,
            b_n: 0,
            blocks_max: Vec::new(),
            blocks_rms: Vec::new(),
            d_acc: 0.0,
            d_n: 0,
            low: Vec::new(),
            fp: None,
            fp_buf: Vec::with_capacity(8192),
            fp_frames: 0,
            frames: 0,
            spec: Spectrum::new(),
        }
    }

    fn init(&mut self, rate: u32) {
        self.rate = rate;
        self.decim = ((rate as f32 / TARGET_RATE as f32).round() as usize).max(1);
        self.block = (rate as usize / 100).max(1);
        if self.want_pioneer {
            self.bands = Some(Bands::new(rate));
        }
        self.spec.init(rate);
        let cfg = Configuration::preset_test2();
        let mut fp = Fingerprinter::new(&cfg);
        if fp.start(rate, 1).is_ok() {
            self.fp = Some(fp);
        }
    }

    fn push(&mut self, inter: &[f32], ch: usize, rate: u32) {
        if self.rate == 0 {
            self.init(rate);
        }
        let fp_limit = self.rate as usize * FP_SECONDS;
        for frame in inter.chunks_exact(ch) {
            let m = frame.iter().sum::<f32>() / ch as f32;
            if let Some(b) = self.bands.as_mut() {
                b.push(m);
            }
            self.spec.push(m);
            let a = m.abs();
            if a > self.b_max {
                self.b_max = a;
            }
            self.b_sq += (m * m) as f64;
            self.b_n += 1;
            if self.b_n == self.block {
                self.blocks_max.push(self.b_max);
                self.blocks_rms
                    .push((self.b_sq / self.b_n as f64).sqrt() as f32);
                self.b_max = 0.0;
                self.b_sq = 0.0;
                self.b_n = 0;
            }
            self.d_acc += m;
            self.d_n += 1;
            if self.d_n == self.decim {
                self.low.push(self.d_acc / self.decim as f32);
                self.d_acc = 0.0;
                self.d_n = 0;
            }
            if self.fp.is_some() && self.fp_frames < fp_limit {
                self.fp_buf.push((m.clamp(-1.0, 1.0) * 32767.0) as i16);
                self.fp_frames += 1;
                if self.fp_buf.len() >= 8192 {
                    self.fp.as_mut().unwrap().consume(&self.fp_buf);
                    self.fp_buf.clear();
                }
            }
            self.frames += 1;
        }
    }
}

fn decode(path: &Path, acc: &mut Accum, cancel: &AtomicBool) -> Result<()> {
    let file = std::fs::File::open(path).with_context(|| format!("Öffnen: {}", path.display()))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| anyhow!("Format nicht unterstützt: {e}"))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("Keine Audiospur"))?;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow!("Keine Codec-Parameter"))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| anyhow!("Codec nicht unterstützt: {e}"))?;
    let track_id = track.id;
    let mut buf: Vec<f32> = Vec::new();
    let mut errors = 0usize;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("abgebrochen"));
        }
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(SymErr::ResetRequired) => break,
            Err(SymErr::IoError(_)) => break,
            Err(e) => return Err(anyhow!("Lesefehler: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(ab) => {
                let ch = ab.spec().channels().count().max(1);
                let rate = ab.spec().rate();
                buf.resize(ab.samples_interleaved(), 0.0);
                ab.copy_to_slice_interleaved(&mut buf);
                acc.push(&buf, ch, rate);
            }
            Err(SymErr::DecodeError(_)) | Err(SymErr::IoError(_)) => {
                errors += 1;
                if errors > 500 {
                    return Err(anyhow!("Datei beschädigt (zu viele Dekodierfehler)"));
                }
            }
            Err(e) => return Err(anyhow!("Dekodierfehler: {e}")),
        }
    }
    if acc.frames == 0 {
        return Err(anyhow!("Keine Audiodaten"));
    }
    Ok(())
}

pub fn analyze(path: &Path, cancel: &AtomicBool) -> Result<AnalysisResult> {
    analyze_with(path, cancel, false)
}

/// Wie `analyze`, mit `pioneer = true` zusätzlich Beatgrid und Pioneer-Wellenform
pub fn analyze_with(path: &Path, cancel: &AtomicBool, pioneer: bool) -> Result<AnalysisResult> {
    let mut acc = Accum::new();
    acc.want_pioneer = pioneer;
    decode(path, &mut acc, cancel)?;
    if acc.rate == 0 || acc.frames == 0 {
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow!("Abgebrochen"));
        }
        return Err(anyhow!("Keine Audiodaten gelesen"));
    }
    if acc.b_n > 0 {
        acc.blocks_max.push(acc.b_max);
        acc.blocks_rms
            .push((acc.b_sq / acc.b_n as f64).sqrt() as f32);
    }
    let fingerprint = match acc.fp.as_mut() {
        Some(fp) => {
            if !acc.fp_buf.is_empty() {
                fp.consume(&acc.fp_buf);
            }
            fp.finish();
            fp.fingerprint().to_vec()
        }
        None => Vec::new(),
    };
    let low_rate = acc.rate as f32 / acc.decim as f32;
    let bpm = detect_bpm(&acc.low, low_rate);
    let pioneer = match acc.bands.take() {
        Some(b) => {
            let frames = b.finish();
            let grid = detect_grid(&acc.low, low_rate, bpm);
            Some(PioneerData {
                bpm: grid
                    .map(|g| g.0)
                    .or(bpm.map(|b| (b * 100.0).round() / 100.0))
                    .unwrap_or(0.0),
                downbeat_ms: grid.map(|g| g.1).unwrap_or(0.0),
                frames,
            })
        }
        None => None,
    };
    Ok(AnalysisResult {
        duration: acc.frames as f64 / acc.rate as f64,
        pioneer,
        peaks: make_peaks(&acc.blocks_max, &acc.blocks_rms),
        fingerprint,
        bpm,
        key: detect_key(&acc.low, low_rate),
        cutoff: acc.spec.cutoff(acc.rate),
    })
}

fn make_peaks(maxs: &[f32], rms: &[f32]) -> Vec<u8> {
    let n = maxs.len();
    let mut out = vec![0u8; PEAK_BINS * 2];
    if n == 0 {
        return out;
    }
    let global = maxs.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    for i in 0..PEAK_BINS {
        let s = i * n / PEAK_BINS;
        let e = ((i + 1) * n / PEAK_BINS).max(s + 1).min(n);
        if s >= n {
            break;
        }
        let mx = maxs[s..e].iter().cloned().fold(0.0f32, f32::max);
        let r = (rms[s..e].iter().map(|x| x * x).sum::<f32>() / (e - s) as f32).sqrt();
        out[i * 2] = ((mx / global).min(1.0) * 255.0).round() as u8;
        out[i * 2 + 1] = ((r / global * 1.4).min(1.0) * 255.0).round() as u8;
    }
    out
}

// ---------- BPM ----------

pub fn detect_bpm(x: &[f32], rate: f32) -> Option<f64> {
    const HOP: usize = 128;
    const WIN: usize = 256;
    if x.len() < rate as usize * 10 {
        return None;
    }
    // Für Geschwindigkeit: höchstens 4 Minuten aus der Mitte
    let max_len = (rate * 240.0) as usize;
    let x = if x.len() > max_len {
        let s = (x.len() - max_len) / 2;
        &x[s..s + max_len]
    } else {
        x
    };
    let frames = (x.len() - WIN) / HOP;
    let mut energy = Vec::with_capacity(frames);
    for f in 0..frames {
        let s = &x[f * HOP..f * HOP + WIN];
        let e: f32 = s.iter().map(|v| v * v).sum::<f32>() / WIN as f32;
        energy.push((1.0 + 1000.0 * e).ln());
    }
    let mut onset: Vec<f32> = vec![0.0; frames];
    for i in 1..frames {
        onset[i] = (energy[i] - energy[i - 1]).max(0.0);
    }
    // gleitenden Mittelwert abziehen
    let orate = rate / HOP as f32;
    let w = (orate * 0.25) as usize;
    let mut prefix = vec![0.0f64; frames + 1];
    for i in 0..frames {
        prefix[i + 1] = prefix[i] + onset[i] as f64;
    }
    let o: Vec<f32> = (0..frames)
        .map(|i| {
            let a = i.saturating_sub(w);
            let b = (i + w + 1).min(frames);
            (onset[i] - ((prefix[b] - prefix[a]) / (b - a) as f64) as f32).max(0.0)
        })
        .collect();
    let mean_o: f32 = o.iter().sum::<f32>() / frames as f32;
    if mean_o < 1e-5 {
        return None;
    }
    let lag_of = |bpm: f32| orate * 60.0 / bpm;
    let max_lag = (lag_of(60.0) * 8.0).ceil() as usize + 2;
    if max_lag >= frames / 2 {
        return None;
    }
    let ac: Vec<f32> = (0..=max_lag)
        .into_par_iter()
        .map(|l| {
            let n = frames - l;
            let mut s = 0.0f32;
            for i in 0..n {
                s += o[i] * o[i + l];
            }
            s / n as f32
        })
        .collect();
    let interp = |lag: f32| -> f32 {
        let i = lag.floor() as usize;
        if i + 1 >= ac.len() {
            return 0.0;
        }
        let f = lag - i as f32;
        ac[i] * (1.0 - f) + ac[i + 1] * f
    };
    // Kammfilter über mehrere Schläge, mit Gewichtung um 120 BPM
    let score = |bpm: f32| -> f32 {
        let l = lag_of(bpm);
        let mut s = 0.0;
        for k in 1..=8 {
            s += interp(l * k as f32) / (k as f32).sqrt();
        }
        let oct = (bpm / 120.0).log2();
        s * (-0.5 * (oct / 0.9).powi(2)).exp()
    };
    let mut best = (0.0f32, 0.0f32);
    let mut b = 60.0f32;
    while b <= 200.0 {
        let s = score(b);
        if s > best.1 {
            best = (b, s);
        }
        b += 0.5;
    }
    let mut fine = best;
    let mut b = best.0 - 1.0;
    while b <= best.0 + 1.0 {
        let s = score(b);
        if s > fine.1 {
            fine = (b, s);
        }
        b += 0.02;
    }
    if fine.1 <= 0.0 {
        return None;
    }
    Some((fine.0 as f64 * 10.0).round() / 10.0)
}

// ---------- Beatgrid ----------

/// Konstantes Beatgrid: (BPM mit 2 Nachkommastellen, erster Downbeat in ms).
/// Vorgehen: Onset-Hüllkurve → Tempo um den BPM-Wert herum auf 0,01 genau so wählen, dass die
/// Schläge über den ganzen Titel am besten deckungsgleich liegen → Phase → Taktanfang über die Bassenergie.
pub fn detect_grid(x: &[f32], rate: f32, bpm_hint: Option<f64>) -> Option<(f64, f64)> {
    const HOP: usize = 32;
    const WIN: usize = 128;
    let bpm0 = bpm_hint.or_else(|| detect_bpm(x, rate))?;
    if x.len() < WIN * 8 {
        return None;
    }
    let frames = (x.len() - WIN) / HOP;
    let fr = rate as f64 / HOP as f64;
    // Onset: Anstieg der log-Energie, gesamt und Bass (Bass für den Taktanfang)
    let a_lp = 1.0 - (-2.0 * std::f32::consts::PI * 150.0 / rate).exp();
    let mut lp = 0.0f32;
    let bass: Vec<f32> = x
        .iter()
        .map(|&v| {
            lp += a_lp * (v - lp);
            lp
        })
        .collect();
    let env = |sig: &[f32]| -> Vec<f32> {
        let mut e = Vec::with_capacity(frames);
        for f in 0..frames {
            let s = &sig[f * HOP..f * HOP + WIN];
            e.push((1.0 + 1000.0 * s.iter().map(|v| v * v).sum::<f32>() / WIN as f32).ln());
        }
        let mut o = vec![0.0f32; frames];
        for i in 1..frames {
            o[i] = (e[i] - e[i - 1]).max(0.0);
        }
        o
    };
    let o = env(x);
    let ob = env(&bass);
    // Zeitpunkt eines Onset-Rahmens: Mitte zwischen den beiden verglichenen Fenstern
    let t_of = |frame: f64| (frame * HOP as f64 + WIN as f64 / 2.0) / rate as f64;

    const BINS: usize = 96;
    let phase_hist = |bpm: f64| -> [f64; BINS] {
        let period = fr * 60.0 / bpm;
        let mut h = [0.0f64; BINS];
        for (i, &v) in o.iter().enumerate() {
            if v > 0.0 {
                let ph = (i as f64 / period).fract();
                h[((ph * BINS as f64) as usize).min(BINS - 1)] += v as f64;
            }
        }
        h
    };
    let peak_of = |h: &[f64; BINS]| -> (usize, f64) {
        let mut best = (0, f64::MIN);
        for i in 0..BINS {
            let s = h[(i + BINS - 1) % BINS] * 0.5 + h[i] + h[(i + 1) % BINS] * 0.5;
            if s > best.1 {
                best = (i, s);
            }
        }
        best
    };
    // Tempo verfeinern: grob ±1 BPM in 0,05, dann ±0,06 in 0,01
    let mut best = (bpm0, f64::MIN);
    let search = |from: f64, to: f64, step: f64, best: &mut (f64, f64)| {
        let mut b = from;
        while b <= to + 1e-9 {
            let (_, s) = peak_of(&phase_hist(b));
            if s > best.1 {
                *best = (b, s);
            }
            b += step;
        }
    };
    search(bpm0 - 1.0, bpm0 + 1.0, 0.05, &mut best);
    let c = best.0;
    search(c - 0.06, c + 0.06, 0.01, &mut best);
    let bpm = (best.0 * 100.0).round() / 100.0;
    let period = fr * 60.0 / bpm;
    let h = phase_hist(bpm);
    let (bin, _) = peak_of(&h);
    // Bin-Mitte, mit Nachbarn verfeinert (Parabel)
    let (l, m, r) = (h[(bin + BINS - 1) % BINS], h[bin], h[(bin + 1) % BINS]);
    let den = l - 2.0 * m + r;
    let off = if den.abs() > 1e-12 {
        (0.5 * (l - r) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let phase = ((bin as f64 + 0.5 + off) / BINS as f64).rem_euclid(1.0);
    let first_frame = phase * period;
    // Taktanfang: welcher von 4 Schlägen hat die meiste Bassenergie?
    let mut bar = [0.0f64; 4];
    let mut k = 0usize;
    loop {
        let f = first_frame + k as f64 * period;
        let i = f.round() as usize;
        if i + 2 >= frames {
            break;
        }
        bar[k % 4] += (ob[i.saturating_sub(1)] + ob[i] + ob[i + 1]) as f64;
        k += 1;
    }
    let down = (0..4)
        .max_by(|a, b| bar[*a].partial_cmp(&bar[*b]).unwrap())
        .unwrap_or(0);
    let beat_s = 60.0 / bpm;
    let mut t = t_of(first_frame + down as f64 * period);
    // frühester Downbeat ≥ 0 (ganze Takte zurück)
    while t - 4.0 * beat_s >= 0.0 {
        t -= 4.0 * beat_s;
    }
    Some((bpm, (t * 1000.0 * 10.0).round() / 10.0))
}

// ---------- Tonart ----------

const NOTES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const MAJOR: [f32; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR: [f32; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

fn pearson(a: &[f32; 12], b: &[f32; 12]) -> f32 {
    let ma = a.iter().sum::<f32>() / 12.0;
    let mb = b.iter().sum::<f32>() / 12.0;
    let (mut num, mut da, mut db) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let x = a[i] - ma;
        let y = b[i] - mb;
        num += x * y;
        da += x * x;
        db += y * y;
    }
    num / (da * db).sqrt().max(1e-9)
}

pub fn detect_key(x: &[f32], rate: f32) -> Option<String> {
    const N: usize = 8192;
    const HOP: usize = 4096;
    if x.len() < N * 4 {
        return None;
    }
    let mut planner = FftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let window: Vec<f32> = (0..N)
        .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos())
        .collect();
    // Bin → Tonklasse vorberechnen
    let bin_pc: Vec<Option<usize>> = (0..N / 2)
        .map(|k| {
            let f = k as f32 * rate / N as f32;
            if !(60.0..=2100.0).contains(&f) {
                return None;
            }
            let midi = 69.0 + 12.0 * (f / 440.0).log2();
            let r = midi.round();
            if (midi - r).abs() > 0.4 {
                return None;
            }
            Some(((r as i32).rem_euclid(12)) as usize)
        })
        .collect();
    let mut chroma = [0.0f32; 12];
    let mut buf = vec![Complex::new(0.0f32, 0.0); N];
    let mut pos = 0;
    while pos + N <= x.len() {
        for i in 0..N {
            buf[i] = Complex::new(x[pos + i] * window[i], 0.0);
        }
        fft.process(&mut buf);
        let mut c = [0.0f32; 12];
        for k in 1..N / 2 {
            if let Some(pc) = bin_pc[k] {
                c[pc] += buf[k].norm();
            }
        }
        let total: f32 = c.iter().sum();
        if total > 1e-3 {
            for i in 0..12 {
                chroma[i] += (c[i] / total).sqrt();
            }
        }
        pos += HOP;
    }
    if chroma.iter().sum::<f32>() <= 0.0 {
        return None;
    }
    let mut best = (f32::MIN, String::new());
    for root in 0..12 {
        let mut maj = [0.0f32; 12];
        let mut min = [0.0f32; 12];
        for i in 0..12 {
            maj[(i + root) % 12] = MAJOR[i];
            min[(i + root) % 12] = MINOR[i];
        }
        let sm = pearson(&chroma, &maj);
        if sm > best.0 {
            best = (sm, NOTES[root].to_string());
        }
        let sn = pearson(&chroma, &min);
        if sn > best.0 {
            best = (sn, format!("{}m", NOTES[root]));
        }
    }
    Some(best.1)
}

// ---------- Bibliothek analysieren ----------

pub fn store(conn: &Connection, id: i64, r: &Result<AnalysisResult>) -> Result<()> {
    // Erst den Titel als analysiert markieren – geht danach etwas schief, wird nicht alles wiederholt
    match r {
        Ok(a) => {
            conn.execute(
                "UPDATE tracks SET analyzed = 1, analysis_error = NULL, peaks = ?2, fingerprint = ?3, bpm = ?4, musical_key = ?5,
                 duration = CASE WHEN duration > 0 THEN duration ELSE ?6 END, cutoff = ?7 WHERE id = ?1",
                params![id, a.peaks, db::u32_to_bytes(&a.fingerprint), a.bpm, a.key, a.duration, a.cutoff.map(|c| c as i64).unwrap_or(-1)],
            )?;
        }
        Err(e) => {
            conn.execute(
                "UPDATE tracks SET analyzed = 1, analysis_error = ?2 WHERE id = ?1",
                params![id, e.to_string()],
            )?;
        }
    }
    if let Ok(AnalysisResult {
        pioneer: Some(p), ..
    }) = r
    {
        // Ein von Hand korrigiertes Grid bleibt erhalten, nur die Wellenform wird erneuert
        conn.execute(
            "INSERT INTO pioneer_analysis(track_id, bpm, downbeat_ms, grid_edited, frames, updated) VALUES(?1, ?2, ?3, 0, ?4, ?5)
             ON CONFLICT(track_id) DO UPDATE SET frames = excluded.frames, updated = excluded.updated,
               bpm = CASE WHEN grid_edited = 1 THEN bpm ELSE excluded.bpm END,
               downbeat_ms = CASE WHEN grid_edited = 1 THEN downbeat_ms ELSE excluded.downbeat_ms END",
            params![id, p.bpm, p.downbeat_ms, p.frames, crate::now()],
        )?;
    }
    Ok(())
}

/// Dieselbe Datei ist schon analysiert (gleicher Name, gleiche Größe und Änderungszeit – z. B. derselbe Titel
/// unter anderem Pfad oder als Kopie)? Dann deren Analyse übernehmen statt die Datei nochmal übers Netz zu lesen.
fn reuse(conn: &Connection, id: i64, donor: i64, pioneer: bool) -> Result<bool> {
    if pioneer {
        let has: bool = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM pioneer_analysis WHERE track_id = ?1)",
            [donor],
            |r| r.get(0),
        )?;
        if !has {
            return Ok(false);
        }
    }
    let n = conn.execute(
        "UPDATE tracks SET (analyzed, analysis_error, peaks, fingerprint, bpm, musical_key, cutoff) =
           (SELECT 1, NULL, peaks, fingerprint, bpm, musical_key, cutoff FROM tracks WHERE id = ?2),
         duration = CASE WHEN duration > 0 THEN duration ELSE (SELECT duration FROM tracks WHERE id = ?2) END
         WHERE id = ?1",
        params![id, donor],
    )?;
    conn.execute(
        "INSERT OR IGNORE INTO pioneer_analysis(track_id, bpm, downbeat_ms, grid_edited, frames, updated)
         SELECT ?1, bpm, downbeat_ms, grid_edited, frames, updated FROM pioneer_analysis WHERE track_id = ?2",
        params![id, donor],
    )?;
    Ok(n > 0)
}

fn file_name(p: &str) -> String {
    let s = p.rsplit(['/', '\\']).next().unwrap_or(p);
    if cfg!(windows) { s.to_lowercase() } else { s.to_string() }
}

/// Analysiert alle noch nicht analysierten Titel (oder die übergebenen IDs) parallel.
/// `on_done` wird nach jedem Titel mit der ID aufgerufen (für Live-Aktualisierung der Liste).
pub fn analyze_pending(
    conn: &Mutex<Connection>,
    ids: Option<Vec<i64>>,
    threads: usize,
    cancel: &AtomicBool,
    progress: Progress,
    on_done: &(dyn Fn(i64) + Sync),
) -> Result<usize> {
    analyze_pending_with(conn, ids, threads, false, cancel, progress, on_done)
}

/// `pioneer = true`: zusätzlich Titel ohne Pioneer-Analyse (Beatgrid/Wellenform) nachholen
pub fn analyze_pending_with(
    conn: &Mutex<Connection>,
    ids: Option<Vec<i64>>,
    threads: usize,
    pioneer: bool,
    cancel: &AtomicBool,
    progress: Progress,
    on_done: &(dyn Fn(i64) + Sync),
) -> Result<usize> {
    type Job = (i64, String, bool, i64, i64);
    let row = |r: &rusqlite::Row| -> rusqlite::Result<Job> { Ok((r.get(0)?, r.get(1)?, r.get::<_, i64>(2)? != 0, r.get(3)?, r.get(4)?)) };
    let (mut jobs, reused): (Vec<Job>, Vec<i64>) = {
        let c = conn.lock().unwrap();
        let jobs: Vec<Job> = match ids {
            Some(ids) => {
                let mut st = c.prepare("SELECT id, path, analyzed, size, mtime FROM tracks WHERE id = ?1")?;
                ids.iter().filter_map(|id| st.query_row([id], row).ok()).collect()
            }
            None => {
                let mut st = c.prepare(
                    "SELECT id, path, analyzed, size, mtime FROM tracks WHERE analyzed = 0 OR (analysis_error IS NULL AND cutoff IS NULL)
                       OR (?1 AND analysis_error IS NULL AND id NOT IN (SELECT track_id FROM pioneer_analysis)) ORDER BY id",
                )?;
                st.query_map([pioneer], row)?.collect::<rusqlite::Result<_>>()?
            }
        };
        // Noch nie analysierte Titel: gibt es dieselbe Datei schon fertig analysiert? (nur Datenbank, kein Dateizugriff)
        let mut reused = Vec::new();
        if jobs.iter().any(|j| !j.2) {
            let mut donors: std::collections::HashMap<(i64, i64, String), i64> = std::collections::HashMap::new();
            let mut st = c.prepare(
                "SELECT id, path, size, mtime FROM tracks WHERE analyzed = 1 AND analysis_error IS NULL AND peaks IS NOT NULL AND cutoff IS NOT NULL AND size > 0",
            )?;
            let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?)))?;
            for (id, path, size, mtime) in rows.flatten() {
                donors.entry((size, mtime, file_name(&path))).or_insert(id);
            }
            if !donors.is_empty() {
                for j in jobs.iter().filter(|j| !j.2 && j.3 > 0) {
                    if let Some(&d) = donors.get(&(j.3, j.4, file_name(&j.1))) {
                        if d != j.0 && reuse(&c, j.0, d, pioneer).unwrap_or(false) {
                            reused.push(j.0);
                        }
                    }
                }
            }
        }
        (jobs, reused)
    };
    if !reused.is_empty() {
        let set: std::collections::HashSet<i64> = reused.iter().copied().collect();
        jobs.retain(|j| !set.contains(&j.0));
        for id in &reused {
            on_done(*id);
        }
    }
    let jobs: Vec<(i64, String)> = jobs.into_iter().map(|j| (j.0, j.1)).collect();
    let total = jobs.len();
    if total == 0 {
        return Ok(reused.len());
    }
    let done = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads.max(1))
        .build()?;
    pool.install(|| {
        jobs.par_iter().for_each(|(id, path)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            // Cloud-Laufwerk: immer nur eine Datei gleichzeitig laden und analysieren
            let _one = crate::paths::is_cloud(Path::new(path)).then(|| CLOUD_ONE.lock());
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let r = analyze_with(Path::new(path), cancel, pioneer);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(c) = conn.lock() {
                let _ = store(&c, *id, &r);
            }
            on_done(*id);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            let name = Path::new(path)
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            progress(n, total, &name);
        });
    });
    Ok(done.load(Ordering::Relaxed) + reused.len())
}

#[cfg(test)]
mod reuse_tests {
    use super::*;

    #[test]
    fn same_file_elsewhere_keeps_analysis() {
        let base = std::env::temp_dir().join(format!("ml-reuse-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let c = crate::db::open(&base.join("t.db")).unwrap();
        c.execute_batch(
            "INSERT INTO tracks(id, path, size, mtime, added, analyzed, peaks, cutoff, bpm) VALUES(1, '/nas/a/Song.mp3', 1000, 77, 0, 1, x'01', -1, 128.0);
             INSERT INTO tracks(id, path, size, mtime, added) VALUES(2, '/nas/b/Song.mp3', 1000, 77, 0);
             INSERT INTO tracks(id, path, size, mtime, added) VALUES(3, '/nas/b/Other.mp3', 1000, 77, 0);",
        )
        .unwrap();
        let m = Mutex::new(c);
        let done = Mutex::new(Vec::new());
        let n = analyze_pending_with(&m, Some(vec![2]), 1, false, &AtomicBool::new(false), &|_, _, _| {}, &|id| done.lock().unwrap().push(id)).unwrap();
        assert_eq!(n, 1);
        assert_eq!(*done.lock().unwrap(), vec![2]);
        let c = m.lock().unwrap();
        let (a, bpm): (i64, f64) = c.query_row("SELECT analyzed, bpm FROM tracks WHERE id = 2", [], |r| Ok((r.get(0)?, r.get(1)?))).unwrap();
        assert_eq!((a, bpm), (1, 128.0));
        // anderer Dateiname → keine Übernahme
        assert!(!c.query_row("SELECT analyzed FROM tracks WHERE id = 3", [], |r| r.get::<_, bool>(0)).unwrap());
    }
}

#[cfg(test)]
mod grid_tests {
    use super::*;

    /// Synthetische 4/4-Spur: Bass-Kick auf 1, leisere Klicks auf 2–4
    fn track(bpm: f64, offset_ms: f64, secs: f64, rate: f32) -> Vec<f32> {
        let n = (secs * rate as f64) as usize;
        let mut x = vec![0.0f32; n];
        let beat = 60.0 / bpm;
        let mut k = 0usize;
        loop {
            let t = offset_ms / 1000.0 + k as f64 * beat;
            let i0 = (t * rate as f64) as usize;
            if i0 >= n {
                break;
            }
            let down = k % 4 == 0;
            for j in 0..(rate as usize / 20) {
                if i0 + j >= n {
                    break;
                }
                let tt = j as f32 / rate;
                let env = (-tt * 40.0).exp();
                let v = if down {
                    (2.0 * std::f32::consts::PI * 55.0 * tt).sin() * 0.9
                } else {
                    (2.0 * std::f32::consts::PI * 1500.0 * tt).sin() * 0.35
                };
                x[i0 + j] += v * env;
            }
            k += 1;
        }
        x
    }

    #[test]
    fn grid_finds_tempo_and_downbeat() {
        let rate = 11025.0;
        for (bpm, off) in [(128.0, 250.0), (124.37, 1730.0), (174.0, 90.0)] {
            let x = track(bpm, off, 90.0, rate);
            let (b, d) = detect_grid(&x, rate, Some((bpm * 10.0f64).round() / 10.0)).expect("grid");
            assert!((b - bpm).abs() <= 0.02, "bpm {b} statt {bpm}");
            let bar = 4.0 * 60000.0 / bpm;
            let want = off % bar;
            let diff = ((d - want + bar / 2.0).rem_euclid(bar) - bar / 2.0).abs();
            assert!(diff < 12.0, "downbeat {d} ms statt {want} ms (bpm {bpm})");
        }
    }
}

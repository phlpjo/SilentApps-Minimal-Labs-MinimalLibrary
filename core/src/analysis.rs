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

/// Anzahl Wellenform-Punkte pro Titel (je Punkt: Spitze + RMS als u8)
pub const PEAK_BINS: usize = 1000;
const FP_SECONDS: usize = 120;
const TARGET_RATE: u32 = 11025;

#[derive(Debug, Clone)]
pub struct AnalysisResult {
    pub duration: f64,
    /// [spitze0, rms0, spitze1, rms1, …]
    pub peaks: Vec<u8>,
    pub fingerprint: Vec<u32>,
    pub bpm: Option<f64>,
    pub key: Option<String>,
}

struct Accum {
    rate: u32,
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
}

impl Accum {
    fn new() -> Self {
        Accum {
            rate: 0, decim: 1, block: 441, b_max: 0.0, b_sq: 0.0, b_n: 0,
            blocks_max: Vec::new(), blocks_rms: Vec::new(), d_acc: 0.0, d_n: 0, low: Vec::new(),
            fp: None, fp_buf: Vec::with_capacity(8192), fp_frames: 0, frames: 0,
        }
    }

    fn init(&mut self, rate: u32) {
        self.rate = rate;
        self.decim = ((rate as f32 / TARGET_RATE as f32).round() as usize).max(1);
        self.block = (rate as usize / 100).max(1);
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
            let a = m.abs();
            if a > self.b_max {
                self.b_max = a;
            }
            self.b_sq += (m * m) as f64;
            self.b_n += 1;
            if self.b_n == self.block {
                self.blocks_max.push(self.b_max);
                self.blocks_rms.push((self.b_sq / self.b_n as f64).sqrt() as f32);
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
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| anyhow!("Format nicht unterstützt: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| anyhow!("Keine Audiospur"))?;
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
    let mut acc = Accum::new();
    decode(path, &mut acc, cancel)?;
    if acc.b_n > 0 {
        acc.blocks_max.push(acc.b_max);
        acc.blocks_rms.push((acc.b_sq / acc.b_n as f64).sqrt() as f32);
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
    Ok(AnalysisResult {
        duration: acc.frames as f64 / acc.rate as f64,
        peaks: make_peaks(&acc.blocks_max, &acc.blocks_rms),
        fingerprint,
        bpm: detect_bpm(&acc.low, low_rate),
        key: detect_key(&acc.low, low_rate),
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

// ---------- Tonart ----------

const NOTES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const MAJOR: [f32; 12] = [6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88];
const MINOR: [f32; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

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
    match r {
        Ok(a) => {
            conn.execute(
                "UPDATE tracks SET analyzed = 1, analysis_error = NULL, peaks = ?2, fingerprint = ?3, bpm = ?4, musical_key = ?5,
                 duration = CASE WHEN duration > 0 THEN duration ELSE ?6 END WHERE id = ?1",
                params![id, a.peaks, db::u32_to_bytes(&a.fingerprint), a.bpm, a.key, a.duration],
            )?;
        }
        Err(e) => {
            conn.execute(
                "UPDATE tracks SET analyzed = 1, analysis_error = ?2 WHERE id = ?1",
                params![id, e.to_string()],
            )?;
        }
    }
    Ok(())
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
    let jobs: Vec<(i64, String)> = {
        let c = conn.lock().unwrap();
        match ids {
            Some(ids) => {
                let mut st = c.prepare("SELECT id, path FROM tracks WHERE id = ?1")?;
                ids.iter().filter_map(|id| st.query_row([id], |r| Ok((r.get(0)?, r.get(1)?))).ok()).collect()
            }
            None => {
                let mut st = c.prepare("SELECT id, path FROM tracks WHERE analyzed = 0 ORDER BY id")?;
                st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?
            }
        }
    };
    let total = jobs.len();
    if total == 0 {
        return Ok(0);
    }
    let done = AtomicUsize::new(0);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads.max(1)).build()?;
    pool.install(|| {
        jobs.par_iter().for_each(|(id, path)| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let r = analyze(Path::new(path), cancel);
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            if let Ok(c) = conn.lock() {
                let _ = store(&c, *id, &r);
            }
            on_done(*id);
            let n = done.fetch_add(1, Ordering::Relaxed) + 1;
            let name = Path::new(path).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            progress(n, total, &name);
        });
    });
    Ok(done.load(Ordering::Relaxed))
}

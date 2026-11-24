// Spektrum („optischer Equalizer“): zeigt, was gerade zu hören ist – Frequenzen 20 Hz–20 kHz,
// Spitzen halten (Peak-Hold), Einfrieren (Hold), Mittelwert, Pegel links/rechts mit Spitzenanzeige.
import { eq } from "./eq";
import { L } from "./i18n";
import { player } from "./player";
import { store } from "./store";
import { h, toast } from "./ui";

type Mode = "bars" | "curve";
interface Prefs { mode: Mode; speed: "fast" | "slow"; hold: number; avg: boolean }

const PREF_KEY = "ml.spectrum";
const loadPrefs = (): Prefs => {
  try {
    return { mode: "bars", speed: "fast", hold: 2, avg: false, ...JSON.parse(localStorage.getItem(PREF_KEY) ?? "{}") };
  } catch {
    return { mode: "bars", speed: "fast", hold: 2, avg: false };
  }
};

const FMIN = 20;
// bis zur halben Abtastrate (44,1 kHz → 22 kHz): so sieht man, ob oben etwas abgeschnitten ist
let FMAX = 22050;
const DB_MIN = -90;
const DB_MAX = 0;
// Terzbänder (1/3 Oktave), 20 Hz – 20 kHz
const THIRDS = Array.from({ length: 33 }, (_, i) => 1000 * Math.pow(2, (i - 17) / 3)).filter((f) => f >= 19 && f <= 24000);
const GRID = [31.5, 63, 125, 250, 500, 1000, 2000, 4000, 8000, 12000, 16000, 20000];
const fmtF = (f: number) => (f >= 1000 ? (f / 1000).toFixed(f >= 10000 ? 0 : 1).replace(".0", "") + " k" : Math.round(f) + "");

let panel: HTMLElement | null = null;

/** Spektrum für einen Titel öffnen (spielt ihn, falls er nicht schon läuft) */
export async function openSpectrum(trackId?: number) {
  if (trackId != null && player.current?.id !== trackId) await player.play(trackId, 0, player.queue.includes(trackId) ? player.queue : [trackId]);
  if (panel) return;
  const prefs = loadPrefs();
  const save = () => localStorage.setItem(PREF_KEY, JSON.stringify(prefs));

  const cv = h("canvas", { class: "spec-cv" }) as HTMLCanvasElement;
  const meter = h("canvas", { class: "spec-meter" }) as HTMLCanvasElement;
  const title = h("span", { class: "spec-title" });
  const readout = h("span", { class: "spec-read" });
  const note = h("div", { class: "spec-note" });
  let frozen = false;
  const opt = (label: string, on: () => boolean, click: () => void, tip = "") => {
    const b = h("button", { class: "btn small", title: tip, onclick: () => { click(); save(); paintBtns(); } }, label);
    btns.push([b, on]);
    return b;
  };
  const btns: [HTMLElement, () => boolean][] = [];
  const paintBtns = () => btns.forEach(([b, on]) => b.classList.toggle("on", on()));
  const holdLabel = () => (prefs.hold === 0 ? L("Spitzen: aus") : prefs.hold === 99 ? L("Spitzen: halten") : L("Spitzen: {0} s", prefs.hold));
  const holdBtn = h("button", { class: "btn small", title: L("Wie lange die Spitzen stehen bleiben"), onclick: () => {
    const seq = [0, 2, 5, 99];
    prefs.hold = seq[(seq.indexOf(prefs.hold) + 1) % seq.length];
    holdBtn.textContent = holdLabel();
    peaks.fill(-200);
    peakT.fill(0);
    save();
  } }, holdLabel());
  const freezeBtn = h("button", { class: "btn small", title: L("Bild anhalten (Leertaste im Fenster)"), onclick: () => toggleFreeze() }, L("Hold"));
  const toggleFreeze = () => {
    frozen = !frozen;
    freezeBtn.classList.toggle("on", frozen);
    freezeBtn.textContent = frozen ? L("● Hold") : L("Hold");
  };
  const bar = h("div", { class: "spec-tools" },
    opt(L("Balken"), () => prefs.mode === "bars", () => { prefs.mode = "bars"; resetHold(); }, L("Terzbänder (1/3 Oktave)")),
    opt(L("Kurve"), () => prefs.mode === "curve", () => { prefs.mode = "curve"; resetHold(); }, L("Feine Kurve")),
    h("span", { class: "spec-sep" }),
    opt(L("Schnell"), () => prefs.speed === "fast", () => (prefs.speed = "fast")),
    opt(L("Ruhig"), () => prefs.speed === "slow", () => (prefs.speed = "slow")),
    h("span", { class: "spec-sep" }),
    holdBtn,
    opt(L("Mittelwert"), () => prefs.avg, () => { prefs.avg = !prefs.avg; avg.fill(-200); }, L("Langzeit-Mittel als Linie")),
    h("span", { class: "spec-sep" }),
    freezeBtn,
    h("button", { class: "btn small", title: L("Spitzen und Mittelwert zurücksetzen"), onclick: () => { peaks.fill(-200); avg.fill(-200); lrPeak = [-200, -200]; clip = [false, false]; } }, L("Zurücksetzen")),
  );
  const head = h("div", { class: "spec-head" }, h("span", { class: "spec-name" }, L("Spektrum")), title, h("span", { class: "spacer" }),
    h("button", { class: "ibtn", title: L("Schließen (Esc)"), onclick: () => close() }, "✕"));
  panel = h("div", { class: "spec-panel" }, head, h("div", { class: "spec-body" }, cv, meter), h("div", { class: "spec-foot" }, readout, note), bar);
  document.body.append(panel);
  paintBtns();

  // Fenster verschieben (am Kopf ziehen), Position merken
  try {
    const pos = JSON.parse(localStorage.getItem("ml.spectrumPos") ?? "null");
    if (pos) Object.assign(panel.style, { left: Math.min(pos.x, innerWidth - 200) + "px", top: Math.min(pos.y, innerHeight - 120) + "px", right: "auto", bottom: "auto" });
  } catch { /* Standard */ }
  head.addEventListener("pointerdown", (e) => {
    if ((e.target as HTMLElement).closest("button")) return;
    const r = panel!.getBoundingClientRect();
    const dx = e.clientX - r.left;
    const dy = e.clientY - r.top;
    const move = (m: PointerEvent) => {
      Object.assign(panel!.style, { left: Math.max(0, Math.min(innerWidth - 120, m.clientX - dx)) + "px", top: Math.max(0, Math.min(innerHeight - 60, m.clientY - dy)) + "px", right: "auto", bottom: "auto" });
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const q = panel?.getBoundingClientRect();
      if (q) localStorage.setItem("ml.spectrumPos", JSON.stringify({ x: q.left, y: q.top }));
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  });

  // Maus über dem Spektrum: Frequenz und Pegel ablesen
  let hover: { x: number; y: number } | null = null;
  cv.addEventListener("pointermove", (e) => { const r = cv.getBoundingClientRect(); hover = { x: e.clientX - r.left, y: e.clientY - r.top }; });
  cv.addEventListener("pointerleave", () => (hover = null));
  cv.addEventListener("click", () => toggleFreeze());
  const key = (e: KeyboardEvent) => {
    if (!panel) return;
    if (e.key === "Escape") { e.stopPropagation(); close(); }
  };
  window.addEventListener("keydown", key, true);

  let data: Float32Array<ArrayBuffer> = new Float32Array(8192);
  const tL = new Float32Array(2048);
  const tR = new Float32Array(2048);
  let peaks = new Float32Array(400).fill(-200);
  let peakT = new Float64Array(400);
  let avg = new Float32Array(400).fill(-200);
  let lrPeak = [-200, -200];
  let lrPeakT = [0, 0];
  let clip = [false, false];
  let lrRms = [-200, -200];
  let raf = 0;
  let last = performance.now();
  function resetHold() { peaks.fill(-200); avg.fill(-200); }

  const ink = () => getComputedStyle(document.documentElement).getPropertyValue("--ink").trim() || "#fff";
  const dim = () => getComputedStyle(document.documentElement).getPropertyValue("--weak").trim() || "#999";

  const size = (c: HTMLCanvasElement) => {
    const r = c.getBoundingClientRect();
    const dpr = Math.min(2, devicePixelRatio || 1);
    const w = Math.max(10, Math.round(r.width * dpr));
    const hh = Math.max(10, Math.round(r.height * dpr));
    if (c.width !== w || c.height !== hh) { c.width = w; c.height = hh; }
    return { w: r.width, h: r.height, dpr };
  };

  const frame = () => {
    if (!panel) return;
    const now = performance.now();
    const dt = Math.min(0.1, (now - last) / 1000);
    last = now;
    const t = player.current;
    title.textContent = t ? store.label(t) + (t.artist ? " · " + t.artist : "") : L("Nichts läuft");
    const tap = player.device ? null : eq.tap();
    note.textContent = player.device
      ? L("Läuft auf „{0}“ – das Spektrum zeigt nur, was hier am Rechner spielt.", player.device.name)
      : !tap ? L("Klangweg aus (Einstellungen → Ausgabe) – kein Spektrum möglich.") : !player.playing && !frozen ? L("Pausiert") : "";
    if (tap && !frozen) {
      tap.spec.smoothingTimeConstant = prefs.speed === "fast" ? 0.55 : 0.85;
      if (data.length !== tap.spec.frequencyBinCount) data = new Float32Array(tap.spec.frequencyBinCount);
      tap.spec.getFloatFrequencyData(data);
      tap.left.getFloatTimeDomainData(tL);
      tap.right.getFloatTimeDomainData(tR);
    }
    draw(tap?.rate ?? 48000, dt, now, !!tap && !frozen);
    raf = requestAnimationFrame(frame);
  };

  const draw = (rate: number, dt: number, now: number, live: boolean) => {
    FMAX = rate / 2;
    const { w, h: hh, dpr } = size(cv);
    const g = cv.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, hh);
    const padL = 34, padB = 18, padT = 6, padR = 6;
    const pw = w - padL - padR, ph = hh - padT - padB;
    const xOf = (f: number) => padL + (Math.log10(f / FMIN) / Math.log10(FMAX / FMIN)) * pw;
    const fOf = (x: number) => FMIN * Math.pow(FMAX / FMIN, (x - padL) / pw);
    const yOf = (db: number) => padT + (1 - (Math.max(DB_MIN, Math.min(DB_MAX, db)) - DB_MIN) / (DB_MAX - DB_MIN)) * ph;
    const INK = ink(), DIM = dim();
    // Raster
    g.font = "10px " + getComputedStyle(document.body).fontFamily;
    g.fillStyle = DIM;
    g.strokeStyle = DIM;
    g.globalAlpha = 0.25;
    g.lineWidth = 1;
    for (const f of GRID) {
      const x = Math.round(xOf(f)) + 0.5;
      g.beginPath(); g.moveTo(x, padT); g.lineTo(x, padT + ph); g.stroke();
    }
    for (let db = DB_MIN; db <= DB_MAX; db += 10) {
      const y = Math.round(yOf(db)) + 0.5;
      g.beginPath(); g.moveTo(padL, y); g.lineTo(padL + pw, y); g.stroke();
    }
    g.globalAlpha = 0.8;
    g.textAlign = "center";
    // Beschriftung: nur, was nicht mit der vorigen überlappt (20 k hat Vorrang vor 12 k)
    let lastRight = -1e9;
    const labels = GRID.filter((f) => f !== 12000);
    labels.forEach((f, i) => {
      const t = fmtF(f).replace(" ", "");
      const tw = g.measureText(t).width;
      const x = xOf(f);
      const nextX = i + 1 < labels.length ? xOf(labels[i + 1]) - g.measureText(fmtF(labels[i + 1]).replace(" ", "")).width / 2 : 1e9;
      if (x - tw / 2 > lastRight + 3 && (f === 20000 || x + tw / 2 < nextX - 3 || f === 16000)) {
        g.fillText(t, x, hh - 4);
        lastRight = x + tw / 2;
      }
    });
    g.textAlign = "right";
    for (let db = DB_MIN; db <= DB_MAX; db += 20) g.fillText(String(db), padL - 5, yOf(db) + 3);
    g.globalAlpha = 1;
    // Gemessene Grenzfrequenz (Analyse „echte Qualität“) als gestrichelte Linie
    const cut = player.current?.cutoff;
    if (cut && cut > FMIN && cut < FMAX) {
      const x = Math.round(xOf(cut)) + 0.5;
      g.strokeStyle = INK;
      g.globalAlpha = 0.7;
      g.setLineDash([4, 4]);
      g.beginPath(); g.moveTo(x, padT); g.lineTo(x, padT + ph); g.stroke();
      g.setLineDash([]);
      g.textAlign = "right";
      g.fillStyle = INK;
      g.fillText(L("Grenze {0}Hz", fmtF(cut)), x - 4, padT + 12);
      g.globalAlpha = 1;
    }

    const binHz = rate / 2 / data.length;
    const level = (f0: number, f1: number) => {
      // Leistungsmittel der Bins im Band
      let i0 = Math.max(1, Math.floor(f0 / binHz)), i1 = Math.max(i0, Math.min(data.length - 1, Math.ceil(f1 / binHz)));
      let sum = 0, n = 0;
      for (let i = i0; i <= i1; i++) { sum += Math.pow(10, data[i] / 10); n++; }
      return n ? 10 * Math.log10(sum / n + 1e-20) : -200;
    };
    const holdS = prefs.hold === 99 ? Infinity : prefs.hold;
    const updPeak = (k: number, v: number) => {
      if (!live) return;
      if (v >= peaks[k]) { peaks[k] = v; peakT[k] = now; }
      else if (now - peakT[k] > holdS * 1000) peaks[k] = Math.max(v, peaks[k] - 18 * dt); // fällt mit 18 dB/s
      if (prefs.avg) avg[k] = avg[k] < -150 ? v : avg[k] + (v - avg[k]) * Math.min(1, dt / 4);
    };

    let maxV = -200, maxF = 0;
    if (prefs.mode === "bars") {
      const n = THIRDS.length;
      for (let k = 0; k < n; k++) {
        const f = THIRDS[k], f0 = f / Math.pow(2, 1 / 6), f1 = Math.min(FMAX, f * Math.pow(2, 1 / 6));
        if (f0 >= FMAX) break;
        const v = level(f0, f1);
        updPeak(k, v);
        if (v > maxV) { maxV = v; maxF = f; }
        const x0 = xOf(f0) + 1.5, x1 = xOf(f1) - 1.5;
        const y = yOf(v);
        g.fillStyle = INK;
        g.globalAlpha = 0.85;
        g.fillRect(x0, y, Math.max(1, x1 - x0), padT + ph - y);
        if (prefs.hold > 0 && peaks[k] > DB_MIN) {
          g.globalAlpha = 1;
          g.fillRect(x0, yOf(peaks[k]) - 1, Math.max(1, x1 - x0), 2);
        }
        if (prefs.avg && avg[k] > DB_MIN) {
          g.globalAlpha = 0.6;
          g.fillRect(x0, yOf(avg[k]), Math.max(1, x1 - x0), 1);
        }
      }
    } else {
      const steps = Math.min(400, Math.round(pw / 2));
      g.beginPath();
      const pk: [number, number][] = [];
      const av: [number, number][] = [];
      for (let k = 0; k < steps; k++) {
        const x = padL + (k / (steps - 1)) * pw;
        const f = fOf(x);
        const v = level(f / Math.pow(2, 1 / 24), f * Math.pow(2, 1 / 24));
        updPeak(k, v);
        if (v > maxV) { maxV = v; maxF = f; }
        const y = yOf(v);
        k ? g.lineTo(x, y) : g.moveTo(x, y);
        pk.push([x, yOf(peaks[k])]);
        av.push([x, yOf(avg[k])]);
      }
      g.lineTo(padL + pw, padT + ph);
      g.lineTo(padL, padT + ph);
      g.closePath();
      g.fillStyle = INK;
      g.globalAlpha = 0.18;
      g.fill();
      g.globalAlpha = 0.95;
      g.strokeStyle = INK;
      g.lineWidth = 1.4;
      g.beginPath();
      for (let k = 0; k < steps; k++) {
        const x = padL + (k / (steps - 1)) * pw;
        const v = level(fOf(x) / Math.pow(2, 1 / 24), fOf(x) * Math.pow(2, 1 / 24));
        k ? g.lineTo(x, yOf(v)) : g.moveTo(x, yOf(v));
      }
      g.stroke();
      if (prefs.hold > 0) {
        g.globalAlpha = 0.55;
        g.lineWidth = 1;
        g.setLineDash([3, 3]);
        g.beginPath();
        pk.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)));
        g.stroke();
        g.setLineDash([]);
      }
      if (prefs.avg) {
        g.globalAlpha = 0.7;
        g.lineWidth = 2;
        g.beginPath();
        av.forEach(([x, y], i) => (i ? g.lineTo(x, y) : g.moveTo(x, y)));
        g.stroke();
      }
    }
    g.globalAlpha = 1;
    // Ablesen unter der Maus
    if (hover && hover.x > padL && hover.x < padL + pw) {
      const f = fOf(hover.x);
      const v = level(f / Math.pow(2, 1 / 12), f * Math.pow(2, 1 / 12));
      g.strokeStyle = INK;
      g.globalAlpha = 0.5;
      g.beginPath(); g.moveTo(hover.x + 0.5, padT); g.lineTo(hover.x + 0.5, padT + ph); g.stroke();
      g.globalAlpha = 1;
      readout.textContent = `${fmtF(f)}Hz · ${v > -150 ? v.toFixed(1) : "–"} dB`;
    } else {
      readout.textContent = maxV > DB_MIN ? L("Stärkste Stelle: {0}Hz ({1} dB)", fmtF(maxF), maxV.toFixed(0)) : "";
    }
    drawMeter(dt, now, live);
  };

  // Pegel L/R: Effektivwert als Balken, Spitze als Strich (hält wie oben), CLIP ab 0 dBFS
  const drawMeter = (dt: number, now: number, live: boolean) => {
    const { w, h: hh, dpr } = size(meter);
    const g = meter.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, hh);
    const INK = ink(), DIM = dim();
    const padT = 18, padB = 18;
    const ph = hh - padT - padB;
    const yOf = (db: number) => padT + (1 - (Math.max(-60, Math.min(0, db)) + 60) / 60) * ph;
    // Mono-Datei: rechts ist still – dann beide Seiten gleich zeigen
    let rEnergy = 0;
    for (const s of tR) rEnergy += s * s;
    const chans = [tL, rEnergy < 1e-12 ? tL : tR];
    const bw = (w - 18) / 2;
    for (let c = 0; c < 2; c++) {
      if (live) {
        let sum = 0, pk = 0;
        for (const s of chans[c]) { sum += s * s; pk = Math.max(pk, Math.abs(s)); }
        const rms = 20 * Math.log10(Math.sqrt(sum / chans[c].length) + 1e-9);
        const pdb = 20 * Math.log10(pk + 1e-9);
        lrRms[c] = rms > lrRms[c] ? rms : lrRms[c] - 24 * dt;
        const holdS = prefs.hold === 99 ? Infinity : Math.max(1, prefs.hold);
        if (pdb >= lrPeak[c]) { lrPeak[c] = pdb; lrPeakT[c] = now; }
        else if (now - lrPeakT[c] > holdS * 1000) lrPeak[c] -= 18 * dt;
        if (pk >= 0.999) clip[c] = true;
      }
      const x = 6 + c * (bw + 6);
      g.fillStyle = DIM;
      g.globalAlpha = 0.25;
      g.fillRect(x, padT, bw, ph);
      g.globalAlpha = 0.9;
      g.fillStyle = INK;
      const y = yOf(lrRms[c]);
      g.fillRect(x, y, bw, padT + ph - y);
      g.globalAlpha = 1;
      g.fillRect(x, yOf(lrPeak[c]) - 1, bw, 2);
      g.font = "10px " + getComputedStyle(document.body).fontFamily;
      g.textAlign = "center";
      g.fillStyle = INK;
      g.fillText(c ? "R" : "L", x + bw / 2, hh - 5);
      g.globalAlpha = clip[c] ? 1 : 0.25;
      g.fillText("CLIP", x + bw / 2, 11);
      g.globalAlpha = 1;
    }
  };

  function close() {
    cancelAnimationFrame(raf);
    window.removeEventListener("keydown", key, true);
    panel?.remove();
    panel = null;
  }
  // Zurücksetzen bei Titelwechsel
  let forId = player.current?.id ?? -1;
  const un = store.on("player", () => {
    if (!panel) return typeof un === "function" && un();
    const id = player.current?.id ?? -1;
    if (id !== forId) { forId = id; peaks.fill(-200); avg.fill(-200); lrPeak = [-200, -200]; clip = [false, false]; }
  });
  if (!eq.enabled) toast(L("Für das Spektrum muss der Klangweg an sein (Einstellungen → Ausgabe)."));
  raf = requestAnimationFrame(frame);
}

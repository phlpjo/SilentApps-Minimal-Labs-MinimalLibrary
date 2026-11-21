import { h, fmtTime } from "./ui";
import { isLight } from "./theme";

let colors = { ink: "#fff", dim: "#d2d2d8", weak: "#a8a8b1", faint: "rgba(255,255,255,0.18)" };
function readColors() {
  const cs = getComputedStyle(document.documentElement);
  const light = isLight();
  colors = {
    ink: cs.getPropertyValue("--ink").trim() || "#fff",
    dim: cs.getPropertyValue("--dim").trim() || "#d2d2d8",
    weak: cs.getPropertyValue("--weak").trim() || "#a8a8b1",
    faint: light ? "rgba(0,0,0,0.16)" : "rgba(255,255,255,0.2)",
  };
}
readColors();
// Farben vor allen anderen Listenern neu lesen
document.addEventListener("themechange", readColors, true);

/**
 * Zeichnet die Wellenform. peaks = [spitze, rms, spitze, rms, …] (je 0–255).
 * progress: 0..1 = abgespielter Anteil (heller), <0 = nicht aktiv.
 */
export function drawWave(canvas: HTMLCanvasElement, peaks: Uint8Array | undefined, progress = -1) {
  const dpr = window.devicePixelRatio || 1;
  const w = Math.max(1, Math.round(canvas.clientWidth * dpr));
  const hgt = Math.max(1, Math.round(canvas.clientHeight * dpr));
  if (canvas.width !== w || canvas.height !== hgt) {
    canvas.width = w;
    canvas.height = hgt;
  }
  const ctx = canvas.getContext("2d")!;
  ctx.clearRect(0, 0, w, hgt);
  const mid = hgt / 2;
  if (!peaks || peaks.length < 4) {
    ctx.fillStyle = colors.faint;
    ctx.fillRect(0, Math.floor(mid), w, Math.max(1, dpr));
    return;
  }
  const bins = peaks.length / 2;
  const step = Math.max(1, Math.round(dpr * 2)); // Balkenbreite inkl. Lücke
  const bar = Math.max(1, step - Math.max(1, Math.round(dpr * 0.6)));
  const playedX = progress >= 0 ? progress * w : -1;
  for (let x = 0; x < w; x += step) {
    const b0 = Math.floor((x / w) * bins);
    const b1 = Math.max(b0 + 1, Math.floor(((x + step) / w) * bins));
    let mx = 0;
    let rms = 0;
    for (let b = b0; b < b1 && b < bins; b++) {
      mx = Math.max(mx, peaks[b * 2]);
      rms = Math.max(rms, peaks[b * 2 + 1]);
    }
    const hm = (mx / 255) * (mid - 1);
    const hr = (rms / 255) * (mid - 1);
    const active = progress < 0 || x < playedX;
    ctx.fillStyle = progress < 0 ? colors.weak : active ? colors.dim : colors.faint;
    ctx.fillRect(x, mid - hm, bar, hm * 2 || 1);
    ctx.fillStyle = progress < 0 ? colors.dim : active ? colors.ink : colors.weak;
    ctx.fillRect(x, mid - hr, bar, hr * 2 || 1);
  }
  if (playedX >= 0) {
    ctx.fillStyle = colors.ink;
    ctx.fillRect(Math.round(playedX), 0, Math.max(1, dpr), hgt);
  }
}

/** Wellenform mit Hover-Zeit; Klick → onSeek(anteil). */
export function waveElement(opts: {
  duration: () => number;
  onSeek: (frac: number) => void;
  cls?: string;
}) {
  const canvas = h("canvas", { class: opts.cls ?? "wave" });
  const line = h("div", { class: "hover-line" });
  const tip = h("div", { class: "hover-time" });
  const wrap = h("div", { class: "wave-wrap" }, canvas, line, tip);
  canvas.addEventListener("mousemove", (e) => {
    const r = canvas.getBoundingClientRect();
    const x = e.clientX - r.left;
    line.style.display = tip.style.display = "block";
    line.style.left = x + "px";
    tip.style.left = Math.max(18, Math.min(r.width - 18, x)) + "px";
    tip.textContent = fmtTime((x / r.width) * opts.duration());
  });
  canvas.addEventListener("mouseleave", () => (line.style.display = tip.style.display = "none"));
  canvas.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    e.stopPropagation();
    const r = canvas.getBoundingClientRect();
    opts.onSeek(Math.max(0, Math.min(1, (e.clientX - r.left) / r.width)));
  });
  canvas.addEventListener("dblclick", (e) => e.stopPropagation());
  return { wrap, canvas };
}

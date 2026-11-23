// Sternenhimmel hinter den Titellisten – wie in Minimal Player Connected.
// „bewegt“: Sterne treiben langsam auf einen zu, beim Scrollen kurzer Warp. „ruhig“: nur leises Funkeln. „aus“.

export type StarMode = "move" | "calm" | "off";

const KEY = "ml.stars";
const listeners = new Set<() => void>();

export function starMode(): StarMode {
  try {
    const v = localStorage.getItem(KEY);
    if (v === "move" || v === "calm" || v === "off") return v;
  } catch {
    /* Standard */
  }
  return "move";
}

export function setStarMode(m: StarMode) {
  try {
    localStorage.setItem(KEY, m);
  } catch {
    /* egal */
  }
  listeners.forEach((f) => f());
}

interface Star { x: number; y: number; z: number; tw: number }

const reduced = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;

/** Sternenhimmel in einem Element (absolut, hinter dem Inhalt). `kick(px)` beim Scrollen aufrufen. */
export function starfield(host: HTMLElement): { kick: (dy: number) => void; destroy: () => void } {
  const cv = document.createElement("canvas");
  cv.className = "stars";
  host.prepend(cv);
  const ctx = cv.getContext("2d")!;
  let w = 0;
  let hgt = 0;
  let dpr = 1;
  let stars: Star[] = [];
  let warp = 0;
  let raf = 0;
  let last = performance.now();
  let alive = true;

  const seed = () => {
    const n = Math.min(380, Math.round((w * hgt) / 2600));
    stars = Array.from({ length: n }, () => ({ x: Math.random() * 2 - 1, y: Math.random() * 2 - 1, z: Math.random() * 0.9 + 0.1, tw: Math.random() * Math.PI * 2 }));
  };
  const resize = () => {
    const r = host.getBoundingClientRect();
    dpr = Math.min(2, window.devicePixelRatio || 1);
    w = Math.max(1, r.width);
    hgt = Math.max(1, r.height);
    cv.width = Math.round(w * dpr);
    cv.height = Math.round(hgt * dpr);
    seed();
    draw(0);
  };
  const ro = new ResizeObserver(resize);
  ro.observe(host);

  const ink = () => (document.documentElement.classList.contains("light") || document.body.classList.contains("light") ? [0, 0, 0, 0.55] : [255, 255, 255, 1]);

  function draw(dt: number) {
    const mode = starMode();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, w, hgt);
    if (mode === "off") return;
    const moving = mode === "move" && !reduced();
    const [r, g, b, amax] = ink();
    const cx = w / 2;
    const cy = hgt / 2;
    const scale = Math.max(w, hgt) * 0.5;
    // sehr langsam treiben, beim Scrollen kurz schneller (Warp)
    const speed = moving ? 0.012 + warp * 0.9 : 0;
    const t = performance.now() / 1000;
    for (const s of stars) {
      const pz = s.z;
      if (moving) {
        s.z -= speed * dt;
        if (s.z <= 0.05) {
          s.x = Math.random() * 2 - 1;
          s.y = Math.random() * 2 - 1;
          s.z = 1;
          continue;
        }
      }
      const px = cx + (s.x / s.z) * scale;
      const py = cy + (s.y / s.z) * scale;
      if (px < -10 || px > w + 10 || py < -10 || py > hgt + 10) {
        if (moving) s.z = 0;
        continue;
      }
      const near = 1 - s.z;
      const twinkle = 0.75 + 0.25 * Math.sin(t * 1.3 + s.tw);
      const a = Math.min(1, (0.15 + near * 0.85) * twinkle) * amax * 0.8;
      const size = 0.4 + near * 1.4;
      ctx.fillStyle = `rgba(${r},${g},${b},${a})`;
      if (moving && warp > 0.02) {
        // Warp: kurze Striche nach außen
        const ox = cx + (s.x / pz) * scale;
        const oy = cy + (s.y / pz) * scale;
        ctx.strokeStyle = ctx.fillStyle;
        ctx.lineWidth = size;
        ctx.beginPath();
        ctx.moveTo(ox, oy);
        ctx.lineTo(px, py);
        ctx.stroke();
      } else {
        ctx.beginPath();
        ctx.arc(px, py, size, 0, Math.PI * 2);
        ctx.fill();
      }
    }
  }

  const loop = (now: number) => {
    if (!alive) return;
    const dt = Math.min(0.05, (now - last) / 1000);
    last = now;
    warp *= Math.pow(0.04, dt); // klingt in ~1 s ab
    if (!document.hidden && cv.isConnected) draw(dt);
    const mode = starMode();
    // ruhig: nur selten neu zeichnen (Funkeln), aus: gar nicht
    if (mode === "move" && !reduced()) raf = requestAnimationFrame(loop);
    else if (mode === "calm" || reduced()) raf = window.setTimeout(() => requestAnimationFrame(loop), 120) as unknown as number;
  };
  const restart = () => {
    cancelAnimationFrame(raf);
    clearTimeout(raf);
    last = performance.now();
    draw(0);
    raf = requestAnimationFrame(loop);
  };
  listeners.add(restart);
  restart();

  return {
    kick: (dy: number) => {
      if (starMode() !== "move") return;
      warp = Math.min(0.35, warp + Math.abs(dy) / 6000);
    },
    destroy: () => {
      alive = false;
      cancelAnimationFrame(raf);
      clearTimeout(raf);
      ro.disconnect();
      listeners.delete(restart);
      cv.remove();
    },
  };
}

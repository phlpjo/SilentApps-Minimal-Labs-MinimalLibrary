import { L, trMsg } from "./i18n";
// Kleine DOM-Helfer im Minimal-Stil

type Attrs = Record<string, unknown>;
type Child = Node | string | number | null | undefined | false | Child[];

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Attrs = {}, ...children: Child[]): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2).toLowerCase(), v as EventListener);
    } else if (k === "class") {
      el.className = String(v);
    } else if (k === "style" && typeof v === "object") {
      Object.assign(el.style, v);
    } else if (k in el && typeof v !== "string") {
      (el as unknown as Record<string, unknown>)[k] = v;
    } else if (k === "value" || k === "checked") {
      (el as unknown as Record<string, unknown>)[k] = v;
    } else {
      el.setAttribute(k, v === true ? "" : String(v));
    }
  }
  append(el, children);
  return el;
}

function append(el: Node, children: Child[]) {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(typeof c === "object" ? c : document.createTextNode(String(c)));
  }
}

export function clear(el: Element) {
  while (el.firstChild) el.removeChild(el.firstChild);
}

export function fmtTime(sec: number): string {
  if (!isFinite(sec) || sec < 0) sec = 0;
  const s = Math.floor(sec % 60);
  const m = Math.floor(sec / 60) % 60;
  const hr = Math.floor(sec / 3600);
  return hr > 0 ? `${hr}:${String(m).padStart(2, "0")}:${String(s).padStart(2, "0")}` : `${m}:${String(s).padStart(2, "0")}`;
}

export function fmtSize(bytes: number): string {
  if (bytes >= 1e9) return (bytes / 1e9).toFixed(2).replace(".", ",") + " GB";
  if (bytes >= 1e6) return (bytes / 1e6).toFixed(1).replace(".", ",") + " MB";
  return Math.round(bytes / 1e3) + " KB";
}

export function fmtDuration(sec: number): string {
  const hr = Math.floor(sec / 3600);
  const m = Math.round((sec % 3600) / 60);
  return hr > 0 ? L("{0} Std. {1} Min.", hr, m) : L("{0} Min.", m);
}

export function basename(p: string): string {
  return p.split(/[\\/]/).pop() ?? p;
}

export function dirname(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i > 0 ? p.slice(0, i) : p;
}

export function ext(p: string): string {
  const b = basename(p);
  const i = b.lastIndexOf(".");
  return i > 0 ? b.slice(i + 1).toUpperCase() : "";
}

// ---------- Toast ----------
let toastBox: HTMLElement | null = null;
export function toast(msg: string, ms = 3500) {
  if (!toastBox) {
    toastBox = h("div", { class: "toasts" });
    document.body.appendChild(toastBox);
  }
  // Meldungen vom Programmkern sind deutsch – hier übersetzen (Zeile für Zeile)
  const t = h("div", { class: "toast" }, msg.split("\n").map(trMsg).join("\n"));
  toastBox.appendChild(t);
  setTimeout(() => t.remove(), ms);
}

export function reportErrors(errors: string[], okMsg?: string) {
  if (errors.length) toast(L("{0} Fehler:\n", errors.length) + errors.slice(0, 4).join("\n") + (errors.length > 4 ? "\n…" : ""), 8000);
  else if (okMsg) toast(okMsg);
}

// ---------- Sheet ----------
export function sheet(title: string, body: Child, opts: { wide?: boolean; onClose?: () => void } = {}) {
  const box = h("div", { class: "sheet" + (opts.wide ? " wide" : "") }, h("h2", {}, title), body);
  const bg = h("div", { class: "sheet-bg" }, box);
  const close = () => {
    bg.remove();
    document.removeEventListener("keydown", onKey, true);
    opts.onClose?.();
  };
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
  };
  bg.addEventListener("mousedown", (e) => {
    if (e.target === bg) close();
  });
  document.addEventListener("keydown", onKey, true);
  document.body.appendChild(bg);
  return { close, box };
}

// ---------- Kontextmenü ----------
export interface MenuItem { label: string; hint?: string; action?: () => void; sub?: MenuItem[]; sep?: boolean; disabled?: boolean }

export function contextMenu(x: number, y: number, items: MenuItem[]) {
  document.querySelectorAll(".menu").forEach((m) => m.remove());
  /** level 0 = Hauptmenü, 1 = Untermenü. altX = Position links vom Elternmenü, falls rechts kein Platz */
  const build = (list: MenuItem[], px: number, py: number, level: number, altX?: number) => {
    const m = h("div", { class: "menu" + (level ? " sub" : "") });
    for (const it of list) {
      if (it.sep) {
        m.appendChild(h("hr"));
        continue;
      }
      const o = h("div", { class: "option", style: it.disabled ? { opacity: "0.45", pointerEvents: "none" } : {} }, it.label, it.hint || it.sub ? h("small", {}, it.hint ?? "›") : null);
      if (it.sub) {
        o.addEventListener("mouseenter", () => {
          document.querySelectorAll(".menu.sub").forEach((s) => s.remove());
          const r = o.getBoundingClientRect();
          build(it.sub!, r.right + 2, r.top - 6, level + 1, r.left - 2);
        });
      } else {
        // Nur im Hauptmenü schließt das Überfahren anderer Einträge das Untermenü
        if (level === 0) o.addEventListener("mouseenter", () => document.querySelectorAll(".menu.sub").forEach((s) => s.remove()));
        o.addEventListener("click", () => {
          document.querySelectorAll(".menu").forEach((mm) => mm.remove());
          it.action?.();
        });
      }
      m.appendChild(o);
    }
    if (level && !list.length) m.appendChild(h("div", { class: "option", style: { opacity: "0.6" } }, L("– keine –")));
    document.body.appendChild(m);
    const r = m.getBoundingClientRect();
    let left = px;
    if (left + r.width > window.innerWidth - 8) left = altX !== undefined ? altX - r.width : window.innerWidth - r.width - 8;
    m.style.left = Math.max(8, left) + "px";
    m.style.top = Math.max(8, Math.min(py, window.innerHeight - r.height - 8)) + "px";
    return m;
  };
  build(items, x, y, 0);
  // pointerdown statt mousedown: Listenzeilen unterdrücken mousedown (wegen Ziehen)
  const close = () => {
    document.querySelectorAll(".menu").forEach((m) => m.remove());
    document.removeEventListener("pointerdown", off, true);
    document.removeEventListener("keydown", esc, true);
    window.removeEventListener("blur", close);
  };
  const off = (e: Event) => {
    if (!(e.target as HTMLElement).closest(".menu")) close();
  };
  const esc = (e: KeyboardEvent) => {
    if (e.key === "Escape") {
      e.stopPropagation();
      close();
    }
  };
  setTimeout(() => {
    document.addEventListener("pointerdown", off, true);
    document.addEventListener("keydown", esc, true);
    window.addEventListener("blur", close);
  });
}

// ---------- Gedrückt halten (statt Rückfrage) ----------
export function holdButton(label: string, action: () => void, hint = L("gedrückt halten"), ms = 1200): HTMLElement {
  const fg = document.createElementNS("http://www.w3.org/2000/svg", "circle");
  fg.setAttribute("class", "fg");
  const bgc = document.createElementNS("http://www.w3.org/2000/svg", "circle");
  bgc.setAttribute("class", "bgc");
  for (const c of [bgc, fg]) {
    c.setAttribute("cx", "10");
    c.setAttribute("cy", "10");
    c.setAttribute("r", "8");
  }
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 20 20");
  svg.append(bgc, fg);
  const btn = h("button", { class: "btn hold" }, svg, h("span", {}, label), h("span", { class: "hint" }, hint));
  let start = 0;
  let raf = 0;
  let fired = false;
  const C = 50.27;
  const tick = () => {
    const p = Math.min(1, (performance.now() - start) / ms);
    fg.style.strokeDashoffset = String(C * (1 - p));
    if (p >= 1) {
      fired = true;
      fg.style.strokeDashoffset = "0";
      action();
      setTimeout(() => (fg.style.strokeDashoffset = String(C)), 400);
      return;
    }
    raf = requestAnimationFrame(tick);
  };
  const down = (e: PointerEvent) => {
    if (e.button !== 0) return;
    fired = false;
    start = performance.now();
    fg.style.transition = "none";
    raf = requestAnimationFrame(tick);
  };
  const up = () => {
    cancelAnimationFrame(raf);
    if (!fired) {
      fg.style.transition = "stroke-dashoffset 0.25s";
      fg.style.strokeDashoffset = String(C);
    }
  };
  btn.addEventListener("pointerdown", down);
  btn.addEventListener("pointerup", up);
  btn.addEventListener("pointerleave", up);
  return btn;
}

// ---------- Tonart / Camelot ----------
const CAMELOT: Record<string, string> = {
  "C": "8B", "G": "9B", "D": "10B", "A": "11B", "E": "12B", "B": "1B", "F#": "2B", "C#": "3B", "G#": "4B", "D#": "5B", "A#": "6B", "F": "7B",
  "Am": "8A", "Em": "9A", "Bm": "10A", "F#m": "11A", "C#m": "12A", "G#m": "1A", "D#m": "2A", "A#m": "3A", "Fm": "4A", "Cm": "5A", "Gm": "6A", "Dm": "7A",
};
const FLAT: Record<string, string> = { Db: "C#", Eb: "D#", Gb: "F#", Ab: "G#", Bb: "A#", Cb: "B", Fb: "E" };

/** "A minor", "Amin", "Am", "8A", "Bb" → "Am"/"A#" */
export function normKey(k: string | null): string | null {
  if (!k) return null;
  let s = k.trim().replace("♯", "#").replace("♭", "b");
  const cam = Object.entries(CAMELOT).find(([, v]) => v.toLowerCase() === s.toLowerCase());
  if (cam) return cam[0];
  const m = s.match(/^([A-Ga-g])([#b]?)\s*(m|min|minor|moll)?(?:\s*(?:maj|major|dur))?$/i);
  if (!m) return s;
  let root = m[1].toUpperCase() + (m[2] || "");
  root = FLAT[root] ?? root;
  return root + (m[3] ? "m" : "");
}

export function camelot(k: string | null): string {
  const n = normKey(k);
  return n ? CAMELOT[n] ?? "" : "";
}

export function keyLabel(k: string | null): string {
  const n = normKey(k);
  if (!n) return "";
  const c = CAMELOT[n];
  return c ? `${c} · ${n}` : n;
}

/** Sortierwert für Camelot-Rad */
export function keyOrder(k: string | null): number {
  const c = camelot(k);
  if (!c) return 999;
  return parseInt(c, 10) * 2 + (c.endsWith("B") ? 1 : 0);
}

// ---------- Klickbare Interpreten ----------
/** Trennt "A, B und C u. D feat. E" in einzelne Namen (Trenner bleiben sichtbar). "&" gehört zum Namen ("Simon & Garfunkel"). */
const ARTIST_SEP = /(\s*,\s*|\s+und\s+|\s+u\.\s*|\s+(?:feat\.?|ft\.|featuring)\s+)/i;

let artistHandler: ((name: string) => void) | null = null;
export function onArtistClick(fn: (name: string) => void) {
  artistHandler = fn;
}

export function artistLinks(text: string | null, fallback = ""): HTMLElement {
  const span = h("span", { class: "artists" });
  if (!text) {
    span.textContent = fallback;
    return span;
  }
  text.split(ARTIST_SEP).forEach((part, i) => {
    if (!part) return;
    if (i % 2 === 1) {
      span.append(part); // Trenner
      return;
    }
    const name = part.trim();
    if (!name) return;
    const a = h("span", { class: "artist-link", title: L("Nach „{0}“ suchen", name) }, part);
    a.addEventListener("pointerdown", (e) => e.stopPropagation());
    a.addEventListener("dblclick", (e) => e.stopPropagation());
    a.addEventListener("click", (e) => {
      e.stopPropagation();
      artistHandler?.(name);
    });
    span.append(a);
  });
  return span;
}

// ---------- Klickbare Alben ----------
let albumHandler: ((name: string) => void) | null = null;
export function onAlbumClick(fn: (name: string) => void) {
  albumHandler = fn;
}

export function albumLink(text: string | null): HTMLElement | string {
  if (!text) return "";
  const a = h("span", { class: "artist-link", title: L("Nach Album „{0}“ suchen", text) }, text);
  a.addEventListener("pointerdown", (e) => e.stopPropagation());
  a.addEventListener("dblclick", (e) => e.stopPropagation());
  a.addEventListener("click", (e) => {
    e.stopPropagation();
    albumHandler?.(text);
  });
  return a;
}

/** Für die Suche: "A&B", "A & B", "A  &B" sind gleich */
export const searchNorm = (s: string) => s.toLowerCase().replace(/\s*&\s*/g, " & ");

/** Echte Qualität als kurzer Text, z. B. „≈ 128 · hochgerechnet“ */
export function qualityText(t: { realKbps: number | null; upscaled: boolean; lossless: boolean; cutoff: number | null; analyzed: boolean }, short = false): string {
  if (t.cutoff == null) return short ? "" : t.analyzed ? L("wird bei der nächsten Analyse gemessen") : L("nach der Analyse");
  if (t.upscaled && t.lossless) return short ? L("≈ {0} (MP3)", t.realKbps) : L("wahrscheinlich aus MP3 hochgerechnet · echte Qualität ≈ {0} kbps", t.realKbps);
  if (t.upscaled) return short ? `≈ ${t.realKbps} ↑` : L("wahrscheinlich hochgerechnet · echte Qualität ≈ {0} kbps", t.realKbps);
  if (t.lossless) return short ? L("verlustfrei") : L("verlustfrei (voller Frequenzgang)");
  if (t.realKbps) return short ? `≈ ${t.realKbps}` : L("≈ {0} kbps", t.realKbps);
  return short ? "" : L("nicht messbar");
}

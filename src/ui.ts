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
  return hr > 0 ? `${hr} Std. ${m} Min.` : `${m} Min.`;
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
  const t = h("div", { class: "toast" }, msg);
  toastBox.appendChild(t);
  setTimeout(() => t.remove(), ms);
}

export function reportErrors(errors: string[], okMsg?: string) {
  if (errors.length) toast(`${errors.length} Fehler:\n` + errors.slice(0, 4).join("\n") + (errors.length > 4 ? "\n…" : ""), 8000);
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
  const build = (list: MenuItem[], px: number, py: number) => {
    const m = h("div", { class: "menu" });
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
          const s = build(it.sub!, r.right + 4, r.top - 6);
          s.classList.add("sub");
        });
      } else {
        o.addEventListener("mouseenter", () => document.querySelectorAll(".menu.sub").forEach((s) => s.remove()));
        o.addEventListener("click", () => {
          document.querySelectorAll(".menu").forEach((mm) => mm.remove());
          it.action?.();
        });
      }
      m.appendChild(o);
    }
    document.body.appendChild(m);
    const r = m.getBoundingClientRect();
    m.style.left = Math.min(px, window.innerWidth - r.width - 8) + "px";
    m.style.top = Math.min(py, window.innerHeight - r.height - 8) + "px";
    return m;
  };
  build(items, x, y);
  const off = (e: Event) => {
    if (!(e.target as HTMLElement).closest(".menu")) {
      document.querySelectorAll(".menu").forEach((m) => m.remove());
      document.removeEventListener("mousedown", off, true);
    }
  };
  setTimeout(() => document.addEventListener("mousedown", off, true));
}

// ---------- Gedrückt halten (statt Rückfrage) ----------
export function holdButton(label: string, action: () => void, hint = "gedrückt halten", ms = 1200): HTMLElement {
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

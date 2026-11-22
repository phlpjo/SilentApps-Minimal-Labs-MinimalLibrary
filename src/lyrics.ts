// Songtexte: anzeigen (mitlaufend), holen (LRCLIB), bearbeiten und Zeiten setzen (Tap-Sync, Feinjustierung)
import { api } from "./api";
import { player } from "./player";
import { store } from "./store";
import { fmtTime, h, holdButton, sheet, toast } from "./ui";

export interface Line { t: number | null; text: string }

const TS = /\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g;

/** LRC → Zeilen (mehrere Zeitmarken pro Zeile möglich, Kopfzeilen wie [ar:] fallen weg) */
export function parseLrc(text: string): Line[] {
  const out: Line[] = [];
  let offset = 0;
  for (const raw of text.split(/\r?\n/)) {
    const off = raw.match(/^\s*\[offset:\s*([+-]?\d+)\]/i);
    if (off) { offset = Number(off[1]) / 1000; continue; }
    if (/^\s*\[[a-z]+:.*\]\s*$/i.test(raw)) continue;
    const times: number[] = [];
    let m: RegExpExecArray | null;
    TS.lastIndex = 0;
    let rest = raw;
    while ((m = TS.exec(raw))) {
      const frac = m[3] ? Number(m[3]) / Math.pow(10, m[3].length) : 0;
      times.push(Number(m[1]) * 60 + Number(m[2]) + frac);
      rest = raw.slice(TS.lastIndex);
    }
    const txt = rest.trim();
    if (times.length) for (const t of times) out.push({ t: Math.max(0, t - offset), text: txt });
    else if (txt) out.push({ t: null, text: txt });
  }
  if (out.some((l) => l.t != null)) out.sort((a, b) => (a.t ?? 0) - (b.t ?? 0));
  return out;
}

export function stamp(t: number): string {
  const m = Math.floor(t / 60);
  const s = t - m * 60;
  return `[${String(m).padStart(2, "0")}:${s.toFixed(2).padStart(5, "0")}]`;
}

/** Index der gerade gesungenen Zeile */
export function currentLine(lines: Line[], t: number): number {
  let idx = -1;
  for (let i = 0; i < lines.length; i++) {
    const lt = lines[i].t;
    if (lt == null) continue;
    if (lt <= t + 0.05) idx = i;
    else break;
  }
  return idx;
}

// ---------- Bearbeiten ----------

interface Row { t: number | null; text: string }

/** Editor-Zeilen: Reihenfolge wie in der Datei, Kopfzeilen ([ar:] …) bleiben erhalten */
function toRows(text: string): { head: string[]; rows: Row[] } {
  const head: string[] = [];
  const rows: Row[] = [];
  let offset = 0;
  for (const raw of text.split(/\r?\n/)) {
    const off = raw.match(/^\s*\[offset:\s*([+-]?\d+)\]/i);
    if (off) { offset = Number(off[1]) / 1000; continue; }
    if (/^\s*\[[a-z]+:.*\]\s*$/i.test(raw)) { head.push(raw.trim()); continue; }
    const times: number[] = [];
    let m: RegExpExecArray | null;
    TS.lastIndex = 0;
    let rest = raw;
    while ((m = TS.exec(raw))) {
      const frac = m[3] ? Number(m[3]) / Math.pow(10, m[3].length) : 0;
      times.push(Math.max(0, Number(m[1]) * 60 + Number(m[2]) + frac - offset));
      rest = raw.slice(TS.lastIndex);
    }
    const txt = rest.trim();
    if (times.length) for (const t of times) rows.push({ t, text: txt });
    else rows.push({ t: null, text: txt });
  }
  while (rows.length && !rows[rows.length - 1].text && rows[rows.length - 1].t == null) rows.pop();
  return { head, rows: rows.length ? rows : [{ t: null, text: "" }] };
}

function fromRows(head: string[], rows: Row[]): string {
  return [...head, ...rows.map((r) => (r.t != null ? stamp(r.t) : "") + r.text)].join("\n");
}

function parseTime(s: string): number | null {
  const m = s.trim().replace(/^\[|\]$/g, "").match(/^(\d{1,3}):(\d{1,2})(?:[.,:](\d{1,3}))?$/);
  if (!m) return null;
  return Number(m[1]) * 60 + Number(m[2]) + (m[3] ? Number(m[3]) / Math.pow(10, m[3].length) : 0);
}

export async function editLyrics(id: number) {
  const t = store.tracks.get(id);
  if (!t) return;
  const cur = await api.lyricsGet(id).catch(() => null);
  let { head, rows } = toRows(cur?.text ?? "");
  let sel = 0;
  let nowIdx = -1;
  const playing = () => player.current?.id === id;
  const dur = () => (playing() && player.duration) || t.duration || 1;

  // ---- Zeilen ----
  const list = h("div", { class: "lyr-rows" });
  const rowEls: HTMLElement[] = [];
  const fmtT = (x: number | null) => (x == null ? "" : stamp(x).slice(1, -1));
  const renderRows = (focus?: number) => {
    rowEls.length = 0;
    list.replaceChildren(...rows.map((r, i) => {
      const time = h("input", { type: "text", class: "lyr-time", value: fmtT(r.t), placeholder: "––:––.––", spellcheck: false }) as HTMLInputElement;
      const text = h("input", { type: "text", class: "lyr-text", value: r.text, placeholder: "Text", spellcheck: false }) as HTMLInputElement;
      time.addEventListener("change", () => { rows[i].t = time.value.trim() ? parseTime(time.value) : null; time.value = fmtT(rows[i].t); draw(); });
      time.addEventListener("focus", () => select(i, false));
      time.addEventListener("dblclick", () => rows[i].t != null && seekTo(rows[i].t!));
      text.addEventListener("focus", () => select(i, false));
      text.addEventListener("input", () => { rows[i].text = text.value; draw(); });
      text.addEventListener("paste", (e) => {
        // mehrere Zeilen einfügen → mehrere Zeilen anlegen
        const s = e.clipboardData?.getData("text") ?? "";
        if (!s.includes("\n")) return;
        e.preventDefault();
        const parsed = toRows(s);
        head = [...head, ...parsed.head.filter((x) => !head.includes(x))];
        const before = text.value.slice(0, text.selectionStart ?? 0);
        const first = parsed.rows[0];
        rows[i] = { t: rows[i].t ?? first.t, text: before + first.text };
        rows.splice(i + 1, 0, ...parsed.rows.slice(1));
        renderRows(i + parsed.rows.length - 1);
        draw();
      });
      text.addEventListener("keydown", (e) => {
        if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); tap(); return; }
        if (e.key === "Enter") {
          e.preventDefault();
          const pos = text.selectionStart ?? text.value.length;
          const rest = text.value.slice(pos);
          rows[i].text = text.value.slice(0, pos);
          rows.splice(i + 1, 0, { t: null, text: rest });
          renderRows(i + 1);
          return;
        }
        if (e.key === "Backspace" && !text.value && rows.length > 1) { e.preventDefault(); rows.splice(i, 1); renderRows(Math.max(0, i - 1)); draw(); return; }
        if (e.key === "ArrowUp") { e.preventDefault(); focusRow(i - 1); }
        if (e.key === "ArrowDown") { e.preventDefault(); focusRow(i + 1); }
      });
      const del = h("button", { class: "lyr-del", title: "Zeile löschen", onclick: () => { if (rows.length > 1) { rows.splice(i, 1); renderRows(Math.min(i, rows.length - 1)); draw(); } } }, "✕");
      const el = h("div", { class: "lyr-row" + (i === sel ? " sel" : "") + (i === nowIdx ? " now" : "") }, time, text, del);
      rowEls.push(el);
      return el;
    }));
    if (focus != null) focusRow(focus);
  };
  const focusRow = (i: number) => {
    if (i < 0 || i >= rows.length) return;
    select(i, true);
    const inp = rowEls[i]?.querySelector(".lyr-text") as HTMLInputElement | null;
    inp?.focus();
  };
  const select = (i: number, scroll: boolean) => {
    sel = Math.max(0, Math.min(rows.length - 1, i));
    rowEls.forEach((el, k) => el.classList.toggle("sel", k === sel));
    if (scroll) rowEls[sel]?.scrollIntoView({ block: "nearest" });
    draw();
  };
  const setRowTime = (i: number, v: number | null) => {
    rows[i].t = v == null ? null : Math.max(0, Math.round(v * 100) / 100);
    const inp = rowEls[i]?.querySelector(".lyr-time") as HTMLInputElement | null;
    if (inp) inp.value = fmtT(rows[i].t);
    draw();
  };

  // ---- Abspielen ----
  const seekTo = (sec: number) => {
    const f = Math.max(0, sec) / dur();
    if (playing()) player.seekFrac(f);
    else player.play(id, f);
  };
  const tap = () => {
    if (!playing()) return toast("Erst den Titel abspielen (▶)");
    setRowTime(sel, player.currentTime);
    let j = sel + 1;
    while (j < rows.length && !rows[j].text.trim()) j++;
    focusRow(Math.min(j, rows.length - 1));
  };

  // ---- Zeitachse (scrollbar, zoombar; Marken ziehen) ----
  let pps = 60; // Pixel pro Sekunde
  const canvas = h("canvas", { class: "lyr-canvas" }) as HTMLCanvasElement;
  const inner = h("div", { class: "lyr-inner" }, canvas);
  const scroller = h("div", { class: "lyr-scroll" }, inner);
  let userScrolledAt = 0;
  const sizeInner = () => { inner.style.width = Math.max(scroller.clientWidth, Math.ceil(dur() * pps)) + "px"; };
  const timeAtX = (clientX: number) => (scroller.scrollLeft + clientX - scroller.getBoundingClientRect().left) / pps;
  function draw() {
    if (!canvas.isConnected) return;
    sizeInner();
    const dpr = window.devicePixelRatio || 1;
    const W = scroller.clientWidth, H = scroller.clientHeight - 2;
    canvas.style.left = scroller.scrollLeft + "px";
    canvas.style.width = W + "px";
    canvas.style.height = H + "px";
    if (canvas.width !== W * dpr || canvas.height !== H * dpr) { canvas.width = W * dpr; canvas.height = H * dpr; }
    const ctx = canvas.getContext("2d")!;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, W, H);
    const cs = getComputedStyle(document.documentElement);
    const ink = cs.getPropertyValue("--ink").trim() || "#fff";
    const weak = cs.getPropertyValue("--weak").trim() || "#999";
    const line = cs.getPropertyValue("--line").trim() || "rgba(255,255,255,.3)";
    const t0 = scroller.scrollLeft / pps;
    const D = dur();
    // Wellenform
    const peaks = store.peaksFor(id);
    const mid = H / 2 + 8;
    if (peaks && peaks.length > 4) {
      const bins = peaks.length / 2;
      ctx.fillStyle = line;
      for (let x = 0; x < W; x += 2) {
        const tt = t0 + x / pps;
        if (tt > D) break;
        const b = Math.min(bins - 1, Math.floor((tt / D) * bins));
        const hm = (peaks[b * 2] / 255) * (H / 2 - 14);
        ctx.fillRect(x, mid - hm, 1.4, hm * 2 || 1);
      }
    }
    // Sekunden-Raster
    ctx.fillStyle = weak;
    ctx.font = "10px system-ui";
    const step = pps >= 40 ? 1 : pps >= 15 ? 5 : 10;
    for (let s = Math.ceil(t0 / step) * step; s < t0 + W / pps; s += step) {
      const x = (s - t0) * pps;
      ctx.fillRect(x, 0, 1, s % (step * 5) === 0 ? 8 : 4);
      if (s % (step * 5) === 0) ctx.fillText(fmtTime(s), x + 3, 10);
    }
    // Zeilen-Marken
    rows.forEach((r, i) => {
      if (r.t == null) return;
      const x = (r.t - t0) * pps;
      if (x < -200 || x > W + 4) return;
      const on = i === nowIdx, s2 = i === sel;
      ctx.fillStyle = on || s2 ? ink : weak;
      ctx.fillRect(x, 14, s2 ? 2 : 1, H - 14);
      ctx.beginPath(); ctx.arc(x, 18, s2 ? 4 : 3, 0, Math.PI * 2); ctx.fill();
      ctx.font = (on ? "600 " : "") + "11px system-ui";
      const label = (r.text || "♪").slice(0, 28);
      ctx.fillText(label, x + 6, 30 + (i % 3) * 13);
    });
    // Abspielposition
    if (playing()) {
      const x = (player.currentTime - t0) * pps;
      ctx.fillStyle = ink;
      ctx.fillRect(x - 1, 0, 2, H);
    }
  }
  let drag: { row: number } | { scrub: true } | null = null;
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId);
    const tt = timeAtX(e.clientX);
    // Marke in der Nähe? → ziehen
    let best = -1, bd = 7;
    rows.forEach((r, i) => { if (r.t != null) { const d = Math.abs((r.t - tt) * pps); if (d < bd) { bd = d; best = i; } } });
    if (best >= 0) { drag = { row: best }; select(best, true); }
    else { drag = { scrub: true }; seekTo(tt); }
  });
  canvas.addEventListener("pointermove", (e) => {
    if (!drag) {
      const tt = timeAtX(e.clientX);
      const near = rows.some((r) => r.t != null && Math.abs((r.t - tt) * pps) < 7);
      canvas.style.cursor = near ? "ew-resize" : "pointer";
      return;
    }
    const tt = Math.max(0, Math.min(dur(), timeAtX(e.clientX)));
    if ("row" in drag) setRowTime(drag.row, tt);
    else seekTo(tt);
  });
  canvas.addEventListener("pointerup", () => { drag = null; });
  scroller.addEventListener("wheel", (e) => {
    e.preventDefault();
    if (e.ctrlKey || e.metaKey) {
      const tt = timeAtX(e.clientX);
      const before = e.clientX - scroller.getBoundingClientRect().left;
      pps = Math.max(8, Math.min(300, pps * (e.deltaY < 0 ? 1.25 : 0.8)));
      sizeInner();
      scroller.scrollLeft = tt * pps - before;
    } else scroller.scrollLeft += (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY);
    userScrolledAt = performance.now();
    draw();
  }, { passive: false });
  scroller.addEventListener("scroll", () => draw());
  scroller.addEventListener("pointerdown", () => (userScrolledAt = performance.now()));

  // ---- Live ----
  const live = h("div", { class: "lyr-live" }, "");
  const onTick = () => {
    if (!playing()) { if (nowIdx !== -1) { nowIdx = -1; rowEls.forEach((el) => el.classList.remove("now")); } draw(); return; }
    const timed = rows.map((r, i) => ({ t: r.t, i })).filter((x) => x.t != null).sort((a, b) => a.t! - b.t!);
    let idx = -1;
    for (const x of timed) { if (x.t! <= player.currentTime + 0.05) idx = x.i; else break; }
    if (idx !== nowIdx) {
      nowIdx = idx;
      rowEls.forEach((el, k) => el.classList.toggle("now", k === idx));
      if (idx >= 0 && document.activeElement?.closest(".lyr-rows") == null) rowEls[idx]?.scrollIntoView({ block: "center", behavior: "smooth" });
      live.textContent = idx >= 0 ? rows[idx].text || "♪" : "";
    }
    // Zeitachse folgt der Abspielposition (außer kurz nach eigenem Scrollen)
    if (!drag && performance.now() - userScrolledAt > 2500) {
      const x = player.currentTime * pps - scroller.scrollLeft;
      if (x < 40 || x > scroller.clientWidth - 60) scroller.scrollLeft = player.currentTime * pps - scroller.clientWidth * 0.3;
    }
    draw();
  };
  const unsub = player.onPos(onTick);
  const unsub2 = store.on("player", onTick);

  // ---- Holen ----
  const fetchBtn = h("button", { class: "btn" }, "Von LRCLIB holen") as HTMLButtonElement;
  let armed = 0;
  fetchBtn.addEventListener("click", async () => {
    const hasText = rows.some((r) => r.text.trim());
    if (hasText && Date.now() - armed > 4000) {
      armed = Date.now();
      return toast("Vorhandenen Text ersetzen? Nochmal klicken.", 4000);
    }
    fetchBtn.textContent = "suche …";
    try {
      const f = await api.lyricsFetch(id);
      if (!f || (!f.synced && !f.plain)) toast(f?.instrumental ? "LRCLIB: Instrumental – kein Text" : "Bei LRCLIB nichts gefunden", 5000);
      else {
        ({ head, rows } = toRows(f.synced ?? f.plain ?? ""));
        renderRows(0);
        draw();
        toast(f.synced ? "Mit Zeitmarken gefunden" : "Nur Text gefunden – Zeiten mit „Zeit setzen“ ergänzen", 5000);
      }
    } catch (e) { toast(String(e), 6000); }
    fetchBtn.textContent = "Von LRCLIB holen";
  });

  // ---- Als Text bearbeiten ----
  const raw = h("textarea", { spellcheck: false, rows: 14, style: { width: "100%", fontFamily: "var(--mono)", fontSize: "13px", display: "none" } }) as HTMLTextAreaElement;
  const rawBtn = h("button", { class: "btn" }, "Als Text") as HTMLButtonElement;
  rawBtn.addEventListener("click", () => {
    const on = raw.style.display === "none";
    if (on) { raw.value = fromRows(head, rows); raw.style.display = ""; list.style.display = "none"; rawBtn.textContent = "Als Zeilen"; }
    else { ({ head, rows } = toRows(raw.value)); raw.style.display = "none"; list.style.display = ""; rawBtn.textContent = "Als Text"; renderRows(); draw(); }
  });

  const tool = (label: string, title: string, fn: () => void) => h("button", { class: "btn", title, onclick: fn }, label);
  const s = sheet(`Songtext · ${store.label(t)}`, h("div", {},
    h("div", { class: "line", style: { flexWrap: "wrap", gap: "6px" } },
      tool("▶ Ab Anfang", "Titel von vorn abspielen", () => { player.play(id, 0); focusRow(0); }),
      tool("⏯", "Abspielen/Pause", () => (playing() ? player.toggle() : player.play(id, 0))),
      tool("−3 s", "3 Sekunden zurück", () => playing() && player.seekBy(-3)),
      h("button", { class: "btn primary", title: "Zeit des Players in die gewählte Zeile, dann nächste Zeile (Strg+Enter)", onclick: tap }, "Zeit setzen ⏎"),
      h("span", { class: "spacer" }),
      tool("−", "Zeitachse verkleinern (Strg+Mausrad)", () => { pps = Math.max(8, pps * 0.8); draw(); }),
      tool("+", "Zeitachse vergrößern (Strg+Mausrad)", () => { pps = Math.min(300, pps * 1.25); draw(); }),
      fetchBtn,
      rawBtn,
    ),
    scroller,
    live,
    h("div", { class: "line", style: { flexWrap: "wrap", gap: "6px", margin: "6px 0" } },
      h("span", { class: "note" }, "Gewählte Zeile:"),
      tool("−0,1 s", "früher", () => rows[sel].t != null && setRowTime(sel, rows[sel].t! - 0.1)),
      tool("+0,1 s", "später", () => rows[sel].t != null && setRowTime(sel, rows[sel].t! + 0.1)),
      tool("Abspielen ab hier", "", () => rows[sel].t != null && seekTo(rows[sel].t!)),
      tool("Zeit löschen", "", () => setRowTime(sel, null)),
      h("span", { class: "note", style: { marginLeft: "10px" } }, "Alle:"),
      tool("−0,5 s", "alle früher", () => { rows.forEach((r, i) => r.t != null && setRowTime(i, r.t - 0.5)); }),
      tool("+0,5 s", "alle später", () => { rows.forEach((r, i) => r.t != null && setRowTime(i, r.t + 0.5)); }),
      tool("Nach Zeit sortieren", "", () => { rows.sort((a, b) => (a.t ?? 1e9) - (b.t ?? 1e9)); renderRows(); draw(); }),
    ),
    list,
    raw,
    h("div", { class: "note" }, "Zeitachse: klicken/ziehen = spulen, Marken ziehen = Zeit verschieben, Mausrad = scrollen, Strg+Mausrad = zoomen. Zeilen: Enter = neue Zeile, Strg+Enter = Zeit setzen, Doppelklick auf die Zeit = dorthin springen. Gespeichert wird als .lrc neben der Datei.",
      cur?.source === "tag" ? " Der bisherige Text stammt aus dem Tag; Speichern legt eine .lrc an." : ""),
    h("div", { class: "foot" },
      cur?.source === "lrc" ? holdButton("Songtext löschen", async () => { await api.lyricsSave(id, ""); s.close(); store.emit("lyrics"); }, "halten") : "",
      h("span", { class: "spacer" }),
      h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
      h("button", { class: "btn primary", onclick: async () => {
        try {
          if (raw.style.display !== "none") ({ head, rows } = toRows(raw.value));
          await api.lyricsSave(id, rows.some((r) => r.text.trim() || r.t != null) ? fromRows(head, rows) : "");
          toast("Songtext gespeichert");
          s.close();
          store.emit("lyrics");
        } catch (e) { toast(String(e)); }
      } }, "Speichern"),
    ),
  ), { wide: true, onClose: () => { unsub(); typeof unsub2 === "function" && unsub2(); } });
  renderRows();
  requestAnimationFrame(() => { draw(); focusRow(0); });
}

/** Für mehrere Titel bei LRCLIB holen (vorhandene bleiben) */
export async function fetchLyricsMany(ids: number[]) {
  try {
    const r = await api.lyricsFetchMany(ids);
    toast(`Songtexte: ${r.found} gefunden (${r.synced} mit Zeitmarken)${r.missing ? `, ${r.missing} nicht gefunden` : ""}${r.skipped ? `, ${r.skipped} hatten schon einen` : ""}`, 7000);
    store.emit("lyrics");
  } catch (e) { toast(String(e)); }
}

// ---------- Anzeige im Inspektor ----------

export function lyricsPanel(id: number): HTMLElement {
  const box = h("div", {}, h("div", { class: "section", style: { padding: "18px 0 6px" } }, "Songtext"), h("div", { class: "note" }, "…"));
  api.lyricsGet(id).then((l) => {
    const lines = l ? parseLrc(l.text) : [];
    box.replaceChildren(
      h("div", { class: "section", style: { padding: "18px 0 6px" } }, "Songtext"),
      h("div", { class: "note" }, l ? `${l.synced ? "mit Zeitmarken" : "ohne Zeitmarken"} · ${l.source === "lrc" ? ".lrc-Datei" : "im Tag"}` : "Keiner"),
      l ? h("div", { class: "lyr-preview" }, lines.slice(0, 6).map((x) => x.text).join("\n") + (lines.length > 6 ? "\n…" : "")) : "",
      h("div", { class: "acts" },
        h("button", { class: "btn", onclick: () => editLyrics(id) }, l ? "Bearbeiten …" : "Hinzufügen / holen …"),
        !l ? h("button", { class: "btn", onclick: () => fetchLyricsMany([id]) }, "Von LRCLIB holen") : "",
      ),
    );
  }).catch(() => {});
  return box;
}

// ---------- Mitlaufende Anzeige (Player-Leiste) ----------

let panel: HTMLElement | null = null;

export function toggleLyricsPanel() {
  if (panel) { panel.remove(); panel = null; store.emit("player"); return; }
  const list = h("div", { class: "lyr-list" });
  const head = h("div", { class: "lyr-head" });
  panel = h("div", { class: "lyr-panel" }, head, list);
  document.body.append(panel);
  let forId = -1;
  let lines: Line[] = [];
  let shown = -2;
  const load = async () => {
    const t = player.current;
    forId = t?.id ?? -1;
    shown = -2;
    head.replaceChildren(
      h("span", { class: "lyr-title" }, t ? store.label(t) : "Nichts läuft"),
      h("span", { class: "spacer" }),
      t ? h("button", { class: "btn", onclick: () => editLyrics(t.id) }, "Bearbeiten") : "",
      h("button", { class: "btn", onclick: () => toggleLyricsPanel() }, "✕"),
    );
    list.replaceChildren(h("div", { class: "note" }, "…"));
    const l = t ? await api.lyricsGet(t.id).catch(() => null) : null;
    if (forId !== (player.current?.id ?? -1)) return;
    lines = l ? parseLrc(l.text) : [];
    list.replaceChildren(
      ...(lines.length
        ? lines.map((x) => h("div", { class: "lyr-line" + (x.t == null ? " plain" : ""), onclick: () => x.t != null && player.seekFrac(x.t / (player.duration || 1)) }, x.text || "♪"))
        : [h("div", { class: "note" }, "Kein Songtext. ", t ? h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); editLyrics(t.id); } }, "Holen oder einfügen …") : "")]),
    );
  };
  const tick = () => {
    if (!panel) return;
    if ((player.current?.id ?? -1) !== forId) { load(); return; }
    const i = currentLine(lines, player.currentTime);
    if (i === shown) return;
    shown = i;
    const els = list.children;
    for (let k = 0; k < els.length; k++) els[k].classList.toggle("on", k === i);
    const el = els[i] as HTMLElement | undefined;
    if (el) list.scrollTo({ top: el.offsetTop - list.clientHeight / 2 + el.clientHeight / 2, behavior: "smooth" });
  };
  const un1 = player.onPos(tick);
  const un2 = store.on("player", tick);
  const un3 = store.on("lyrics", () => load());
  const obs = new MutationObserver(() => { if (panel && !document.body.contains(panel)) { un1(); typeof un2 === "function" && un2(); typeof un3 === "function" && un3(); obs.disconnect(); } });
  obs.observe(document.body, { childList: true });
  load();
  store.emit("player");
}

export const lyricsPanelOpen = () => !!panel;

// Songtexte: anzeigen (mitlaufend), holen (LRCLIB), bearbeiten und Zeiten setzen (Tap-Sync, Feinjustierung)
import { api } from "./api";
import { player } from "./player";
import { store } from "./store";
import { h, holdButton, sheet, toast } from "./ui";

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

/** Zeitmarke am Anfang einer Zeile ersetzen/setzen */
function setStamp(line: string, t: number): string {
  return stamp(t) + line.replace(/^(\s*\[\d{1,3}:\d{1,2}(?:[.:]\d{1,3})?\])+/, "");
}

function shiftLine(line: string, d: number): string {
  return line.replace(/\[(\d{1,3}):(\d{1,2})(?:[.:](\d{1,3}))?\]/g, (_x, a, b, c) => {
    const frac = c ? Number(c) / Math.pow(10, c.length) : 0;
    return stamp(Math.max(0, Number(a) * 60 + Number(b) + frac + d));
  });
}

export async function editLyrics(id: number) {
  const t = store.tracks.get(id);
  if (!t) return;
  const cur = await api.lyricsGet(id).catch(() => null);
  const ta = h("textarea", { spellcheck: false, rows: 18, style: { width: "100%", fontFamily: "var(--mono)", fontSize: "13px", lineHeight: "1.55", resize: "vertical" } }) as HTMLTextAreaElement;
  ta.value = cur?.text ?? "";
  const live = h("div", { class: "lyr-live" }, "");
  const info = h("div", { class: "note" });

  const lines = () => ta.value.split("\n");
  const caretLine = () => ta.value.slice(0, ta.selectionStart).split("\n").length - 1;
  const selectLine = (i: number) => {
    const ls = lines();
    i = Math.max(0, Math.min(ls.length - 1, i));
    const start = ls.slice(0, i).reduce((s, l) => s + l.length + 1, 0);
    ta.focus();
    ta.setSelectionRange(start, start + ls[i].length);
    // ungefähr mitscrollen
    const lh = parseFloat(getComputedStyle(ta).lineHeight) || 20;
    ta.scrollTop = Math.max(0, i * lh - ta.clientHeight / 2);
  };
  const editLine = (i: number, f: (l: string) => string) => {
    const ls = lines();
    if (i < 0 || i >= ls.length) return;
    ls[i] = f(ls[i]);
    ta.value = ls.join("\n");
    selectLine(i);
  };
  const playing = () => player.current?.id === id;

  // Tap-Sync: Zeit des Players in die aktuelle Zeile, dann zur nächsten Textzeile
  const tap = () => {
    if (!playing()) return toast("Erst den Titel hier abspielen (▶ Ab Anfang)");
    let i = caretLine();
    const ls = lines();
    while (i < ls.length && !ls[i].replace(/^(\s*\[[^\]]*\])+/, "").trim() && /^\s*\[[a-z]+:/i.test(ls[i])) i++;
    editLine(i, (l) => setStamp(l, player.currentTime));
    // nächste nicht-leere Zeile wählen
    let j = i + 1;
    while (j < lines().length && !lines()[j].trim()) j++;
    selectLine(j);
  };

  const fetchBtn = h("button", { class: "btn", onclick: async () => {
    fetchBtn.textContent = "suche …";
    try {
      const f = await api.lyricsFetch(id);
      if (!f || (!f.synced && !f.plain)) toast(f?.instrumental ? "LRCLIB: Instrumental – kein Text" : "Bei LRCLIB nichts gefunden", 5000);
      else {
        if (ta.value.trim() && !confirmReplace()) return;
        ta.value = f.synced ?? f.plain ?? "";
        toast(f.synced ? "Mit Zeitmarken gefunden" : "Nur Text gefunden – Zeiten per „Zeit setzen“ ergänzen", 5000);
        refresh();
      }
    } catch (e) {
      toast(String(e), 6000);
    } finally {
      fetchBtn.textContent = "Von LRCLIB holen";
    }
  } }, "Von LRCLIB holen");
  // Ersetzen bestätigen ohne Rückfrage-Dialog: zweiter Klick innerhalb 4 s
  let armed = 0;
  const confirmReplace = () => {
    if (Date.now() - armed < 4000) return true;
    armed = Date.now();
    toast("Vorhandenen Text ersetzen? Nochmal auf „Von LRCLIB holen“ klicken.", 4000);
    fetchBtn.textContent = "Von LRCLIB holen";
    return false;
  };

  const refresh = () => {
    const parsed = parseLrc(ta.value);
    const n = parsed.length;
    const timed = parsed.filter((l) => l.t != null).length;
    info.textContent = n ? `${n} Zeilen · ${timed ? `${timed} mit Zeit` : "ohne Zeiten"}` : "Leer – Text einfügen oder von LRCLIB holen.";
  };
  ta.addEventListener("input", refresh);
  refresh();

  // Live-Vorschau während der Titel läuft
  const unsub = player.onPos(() => {
    if (!playing()) { live.textContent = ""; return; }
    const ls = parseLrc(ta.value);
    const i = currentLine(ls, player.currentTime);
    live.textContent = i >= 0 ? ls[i].text || "♪" : "";
  });

  ta.addEventListener("keydown", (e) => {
    // Strg/⌘+Enter = Zeit setzen
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); tap(); }
  });

  const tool = (label: string, title: string, fn: () => void) => h("button", { class: "btn", title, onclick: fn }, label);
  const s = sheet(`Songtext · ${store.label(t)}`, h("div", {},
    h("div", { class: "line", style: { flexWrap: "wrap", gap: "6px" } },
      fetchBtn,
      tool("▶ Ab Anfang", "Titel von vorn abspielen", () => { player.play(id, 0); selectLine(0); }),
      tool("⏯", "Abspielen/Pause", () => (playing() ? player.toggle() : player.play(id, 0))),
      tool("−3 s", "3 Sekunden zurück", () => playing() && player.seekBy(-3)),
      h("button", { class: "btn primary", title: "Zeit des Players in die markierte Zeile, dann nächste Zeile (Strg+Enter)", onclick: tap }, "Zeit setzen ⏎"),
    ),
    h("div", { class: "line", style: { flexWrap: "wrap", gap: "6px" } },
      h("span", { class: "note" }, "Markierte Zeile:"),
      tool("−0,1 s", "Zeile früher", () => editLine(caretLine(), (l) => shiftLine(l, -0.1))),
      tool("+0,1 s", "Zeile später", () => editLine(caretLine(), (l) => shiftLine(l, 0.1))),
      tool("Hierhin springen", "Player an die Zeit dieser Zeile", () => {
        const l = parseLrc(lines()[caretLine()] ?? "")[0];
        if (l?.t != null) { if (!playing()) player.play(id, 0).then(() => setTimeout(() => player.seekFrac(l.t! / (player.duration || 1)), 300)); else player.seekFrac(l.t / (player.duration || 1)); }
      }),
      h("span", { class: "note", style: { marginLeft: "10px" } }, "Alle:"),
      tool("−0,5 s", "Alle Zeiten früher", () => { ta.value = lines().map((l) => shiftLine(l, -0.5)).join("\n"); refresh(); }),
      tool("+0,5 s", "Alle Zeiten später", () => { ta.value = lines().map((l) => shiftLine(l, 0.5)).join("\n"); refresh(); }),
      tool("Zeiten entfernen", "Nur den Text behalten", () => { ta.value = lines().map((l) => l.replace(/^(\s*\[\d{1,3}:\d{1,2}(?:[.:]\d{1,3})?\])+\s?/, "")).join("\n"); refresh(); }),
    ),
    ta,
    live,
    info,
    h("div", { class: "note" }, "Text einfügen oder holen, dann „▶ Ab Anfang“ und bei jeder Zeile im Takt „Zeit setzen“ (Strg+Enter). Gespeichert wird als .lrc neben der Datei – das lesen Minimal Player, Connected und viele andere Player.",
      cur?.source === "tag" ? " Der bisherige Text stammt aus dem Tag der Datei; Speichern legt eine .lrc an." : ""),
    h("div", { class: "foot" },
      cur?.source === "lrc" ? holdButton("Songtext löschen", async () => { await api.lyricsSave(id, ""); s.close(); store.emit("lyrics"); }, "halten") : "",
      h("span", { class: "spacer" }),
      h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
      h("button", { class: "btn primary", onclick: async () => {
        try {
          await api.lyricsSave(id, ta.value);
          toast("Songtext gespeichert");
          s.close();
          store.emit("lyrics");
        } catch (e) { toast(String(e)); }
      } }, "Speichern"),
    ),
  ), { wide: true, onClose: () => unsub() });
  setTimeout(() => ta.focus(), 50);
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

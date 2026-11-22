// Pioneer DJ / AlphaTheta: Beatgrid und Cues im Inspektor, Export-Dialog (OneLibrary + rekordbox-XML).
// Alles nur sichtbar, wenn „Pioneer-Export“ in den Einstellungen eingeschaltet ist.
import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type PioneerCue, type PioneerOptions, type PioneerTrack } from "./api";
import { player } from "./player";
import { store } from "./store";
import { clear, fmtSize, h, holdButton, reportErrors, sheet, toast } from "./ui";
import { analyzeIds } from "./main";
import { L, pl, trMsg } from "./i18n";

export const pioneerOn = () => !!store.settings.pioneer;

/** Farben der Hot Cues (Index 1–8, 0 = keine) – gleiche Tabelle wie im Export */
export const CUE_COLORS = ["", "#e62828", "#ff8c00", "#ffdc00", "#28e214", "#10b176", "#305aff", "#8832ff", "#ff34b5"];
const COLOR_NAMES = [L("keine"), L("Rot"), "Orange", L("Gelb"), L("Grün"), L("Türkis"), L("Blau"), L("Violett"), "Pink"];
const letter = (k: number) => String.fromCharCode(64 + k);
const fmtMs = (ms: number) => {
  const s = Math.max(0, ms) / 1000;
  return `${Math.floor(s / 60)}:${(s % 60).toFixed(3).padStart(6, "0")}`;
};

function decode(b64: string | null): Uint8Array {
  if (!b64) return new Uint8Array();
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

const WINDOW_MS = 6000;

/** Beatgrid- und Cue-Editor für einen Titel (im Inspektor) */
export function pioneerPanel(trackId: number): HTMLElement {
  const box = h("div", { class: "pio" });
  let data: PioneerTrack | null = null;
  let frames: Uint8Array = new Uint8Array();
  let cursor = 0; // ms
  let raf = 0;
  const canvas = h("canvas", { class: "pio-wave", title: L("Klicken: Position setzen · Mausrad: verschieben") }) as HTMLCanvasElement;
  const info = h("div", { class: "note" });
  const cueBox = h("div", {});
  const t = () => store.tracks.get(trackId);
  const playingThis = () => player.current?.id === trackId;
  const pos = () => (playingThis() ? player.currentTime * 1000 : cursor);

  const draw = () => {
    if (!canvas.isConnected) return;
    const dpr = window.devicePixelRatio || 1;
    const w = canvas.clientWidth;
    const hh = canvas.clientHeight;
    if (canvas.width !== w * dpr) {
      canvas.width = w * dpr;
      canvas.height = hh * dpr;
    }
    const g = canvas.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, w, hh);
    const css = getComputedStyle(document.body);
    const ink = css.getPropertyValue("--ink").trim() || "#fff";
    const weak = css.getPropertyValue("--weak").trim() || "#999";
    const center = pos();
    const start = center - WINDOW_MS / 2;
    const msPerPx = WINDOW_MS / w;
    const n = frames.length / 4;
    // Wellenform (Bass rot, Mitten grün, Höhen blau – wie die Farbwellenform der Player)
    for (let x = 0; x < w; x++) {
      const ms = start + x * msPerPx;
      const i = Math.floor((ms / 1000) * 150);
      if (i < 0 || i >= n) continue;
      const pk = frames[i * 4] / 255;
      const r = frames[i * 4 + 1], gg = frames[i * 4 + 2], b = frames[i * 4 + 3];
      const m = Math.max(r, gg, b, 1);
      g.fillStyle = `rgb(${Math.round((r / m) * 235 + 20)},${Math.round((gg / m) * 235 + 20)},${Math.round((b / m) * 235 + 20)})`;
      const bh = pk * (hh / 2 - 4);
      g.fillRect(x, hh / 2 - bh, 1, bh * 2);
    }
    // Beatgrid
    const grid = data?.grid;
    if (grid && grid.bpm > 0) {
      const p = 60000 / grid.bpm;
      let k = Math.ceil((start - grid.downbeatMs) / p);
      for (;;) {
        const ms = grid.downbeatMs + k * p;
        if (ms > start + WINDOW_MS) break;
        const x = (ms - start) / msPerPx;
        const down = ((k % 4) + 4) % 4 === 0;
        g.fillStyle = down ? ink : weak;
        g.globalAlpha = down ? 0.95 : 0.45;
        g.fillRect(Math.round(x), 0, down ? 2 : 1, down ? hh : 8);
        if (!down) g.fillRect(Math.round(x), hh - 8, 1, 8);
        k++;
      }
      g.globalAlpha = 1;
    }
    // Cues
    for (const q of data?.cues ?? []) {
      const x = (q.startMs - start) / msPerPx;
      if (x < -20 || x > w + 20) continue;
      g.fillStyle = q.kind > 0 ? CUE_COLORS[q.color ?? 0] || ink : "#e62828";
      g.fillRect(Math.round(x), 0, 2, hh);
      if (q.endMs) {
        g.globalAlpha = 0.18;
        g.fillRect(x, 0, (q.endMs - q.startMs) / msPerPx, hh);
        g.globalAlpha = 1;
      }
      g.font = "600 11px 'Space Grotesk', sans-serif";
      g.fillText(q.kind > 0 ? letter(q.kind) : "M", x + 4, 12);
    }
    // Position
    g.fillStyle = ink;
    g.fillRect(Math.round(w / 2), 0, 1, hh);
    raf = requestAnimationFrame(draw);
  };

  canvas.addEventListener("click", (e) => {
    const r = canvas.getBoundingClientRect();
    const ms = pos() + ((e.clientX - r.left) / r.width - 0.5) * WINDOW_MS;
    cursor = Math.max(0, ms);
    if (playingThis()) player.seekFrac(cursor / 1000 / (player.duration || 1));
  });
  canvas.addEventListener("wheel", (e) => {
    e.preventDefault();
    cursor = Math.max(0, pos() + (e.deltaY > 0 ? 250 : -250) * (e.shiftKey ? 4 : 1));
    if (playingThis()) player.seekFrac(cursor / 1000 / (player.duration || 1));
  }, { passive: false });

  const setGrid = async (bpm: number, downbeat: number, what: string) => {
    try {
      await api.pioneerSetGrid(trackId, bpm, downbeat, what);
      store.emit("history");
      load();
    } catch (e) {
      toast(String(e));
    }
  };
  const setCue = async (q: PioneerCue) => {
    try {
      await api.pioneerSetCue(trackId, q);
      store.emit("history");
      load();
    } catch (e) {
      toast(String(e));
    }
  };

  const bpmInput = h("input", { type: "number", step: "0.01", min: "20", max: "400", style: { width: "92px" } }) as HTMLInputElement;
  bpmInput.addEventListener("change", () => {
    const g = data?.grid;
    const v = Number(bpmInput.value);
    if (g && v > 0) setGrid(v, g.downbeatMs, L("Tempo {0} BPM", v.toFixed(2)));
  });

  const gridButtons = h("div", { class: "pio-row" },
    h("button", { class: "btn", title: L("Taktanfang (Schlag 1) an die aktuelle Position legen"), onclick: () => {
      const g = data?.grid;
      setGrid(g?.bpm || t()?.bpm || 120, Math.round(pos() * 10) / 10, L("Downbeat gesetzt"));
    } }, L("Downbeat hier")),
    h("button", { class: "btn", title: L("Halbes Tempo"), onclick: () => data?.grid && setGrid(data.grid.bpm / 2, data.grid.downbeatMs, L("Halbes Tempo")) }, "½"),
    h("button", { class: "btn", title: L("Doppeltes Tempo"), onclick: () => data?.grid && setGrid(data.grid.bpm * 2, data.grid.downbeatMs, L("Doppeltes Tempo")) }, "×2"),
    h("button", { class: "btn", title: L("Grid 10 ms nach links"), onclick: () => data?.grid && setGrid(data.grid.bpm, data.grid.downbeatMs - 10, L("Grid verschoben")) }, "◀"),
    h("button", { class: "btn", title: L("Grid 10 ms nach rechts"), onclick: () => data?.grid && setGrid(data.grid.bpm, data.grid.downbeatMs + 10, L("Grid verschoben")) }, "▶"),
  );

  const renderCues = () => {
    clear(cueBox);
    const cues = data?.cues ?? [];
    const hot = h("div", { class: "pio-hot" },
      ...[1, 2, 3, 4, 5, 6, 7, 8].map((k) => {
        const q = cues.find((c) => c.kind === k);
        const b = h("button", { class: "pio-cue" + (q ? " set" : ""), title: q ? L("Hot Cue {0} bei {1} – klicken: hinspringen", letter(k), fmtMs(q.startMs)) : L("Hot Cue {0} an der aktuellen Position setzen", letter(k)) }, letter(k));
        if (q?.color) b.style.borderColor = CUE_COLORS[q.color];
        b.addEventListener("click", () => {
          if (q) {
            cursor = q.startMs;
            if (playingThis()) player.seekFrac(q.startMs / 1000 / (player.duration || 1));
          } else {
            setCue({ id: 0, kind: k, startMs: Math.round(pos()), endMs: null, color: k, comment: null });
          }
        });
        return b;
      }),
    );
    const rows = cues.map((q) => {
      const comment = h("input", { type: "text", value: q.comment ?? "", placeholder: L("Kommentar"), style: { flex: "1", minWidth: "0" } }) as HTMLInputElement;
      comment.addEventListener("change", () => setCue({ ...q, comment: comment.value.trim() || null }));
      const color = h("select", { title: L("Farbe") }, ...COLOR_NAMES.map((n, i) => h("option", { value: String(i), selected: (q.color ?? 0) === i }, n))) as HTMLSelectElement;
      color.addEventListener("change", () => setCue({ ...q, color: Number(color.value) || null }));
      return h("div", { class: "pio-cue-row" },
        h("span", { class: "pio-tag", style: { color: q.kind ? CUE_COLORS[q.color ?? 0] || "inherit" : "inherit" } }, q.kind ? letter(q.kind) : "M"),
        h("span", { class: "mono", style: { cursor: "pointer" }, onclick: () => { cursor = q.startMs; if (playingThis()) player.seekFrac(q.startMs / 1000 / (player.duration || 1)); } }, fmtMs(q.startMs) + (q.endMs ? ` → ${fmtMs(q.endMs)}` : "")),
        comment,
        q.kind ? color : "",
        h("button", { class: "btn", title: L("Löschen (Strg+Z macht es rückgängig)"), onclick: async () => { await api.pioneerDeleteCue(trackId, q.id); store.emit("history"); load(); } }, "×"),
      );
    });
    cueBox.append(
      hot,
      h("div", { class: "pio-row" },
        h("button", { class: "btn", onclick: () => setCue({ id: 0, kind: 0, startMs: Math.round(pos()), endMs: null, color: null, comment: null }) }, L("Memory Cue hier")),
        h("span", { class: "note" }, L("Hot Cue: freie Taste setzt, belegte springt")),
      ),
      ...rows,
    );
  };

  const load = async () => {
    try {
      data = await api.pioneerTrack(trackId);
    } catch (e) {
      clear(box);
      box.append(h("div", { class: "note" }, trMsg(String(e))));
      return;
    }
    frames = decode(data.frames);
    const g = data.grid;
    if (!playingThis() && cursor === 0 && g) cursor = g.downbeatMs;
    bpmInput.value = g ? g.bpm.toFixed(2) : "";
    info.textContent = g
      ? L("{0} BPM · erster Downbeat {1}{2}", g.bpm.toFixed(2), fmtMs(g.downbeatMs), g.edited ? L(" · von Hand korrigiert") : "")
      : L("Noch kein Beatgrid.");
    renderCues();
    if (!data.frames) {
      box.replaceChildren(
        h("div", { class: "section", style: { padding: "18px 0 6px" } }, L("Pioneer: Beatgrid & Cues")),
        h("div", { class: "note" }, L("Für diesen Titel fehlt noch die Pioneer-Analyse (Beatgrid, Wellenform).")),
        h("button", { class: "btn", onclick: () => analyzeIds([trackId], true) }, L("Jetzt analysieren")),
        cueBox,
      );
    }
  };

  box.append(
    h("div", { class: "section", style: { padding: "18px 0 6px" } }, L("Pioneer: Beatgrid & Cues")),
    canvas,
    info,
    h("div", { class: "pio-row" }, h("label", { class: "note" }, L("Tempo")), bpmInput, h("span", { class: "note" }, "BPM")),
    gridButtons,
    cueBox,
  );
  const unsub = store.on("pioneer", () => (box.isConnected ? load() : unsub()));
  load();
  raf = requestAnimationFrame(draw);
  void raf;
  return box;
}

// ---------- Export-Dialog ----------

/** Playlisten auf einen Stick für Pioneer DJ / AlphaTheta-Player exportieren */
export async function pioneerExportDialog(preselect: number[]) {
  if (!pioneerOn()) {
    toast(L("Pioneer-Export zuerst in den Einstellungen einschalten"));
    return;
  }
  const checks = new Map<number, HTMLInputElement>();
  const dest = h("input", { type: "text", value: localStorage.getItem("ml.pioneerDest") ?? "", placeholder: L("Laufwerk, z. B. E:\\ oder /Volumes/USB"), style: { width: "100%" } }) as HTMLInputElement;
  const conv = h("input", { type: "checkbox", checked: localStorage.getItem("ml.pioneerConvert") === "1" }) as HTMLInputElement;
  const pv = h("div", { class: "note", style: { lineHeight: "1.6" } }, L("Laufwerk und Playlisten wählen …"));
  const opts = (): PioneerOptions => ({
    dest: dest.value.trim(),
    playlists: [...checks].filter(([, c]) => c.checked).map(([i]) => i),
    unsupported: conv.checked ? "convert" : "skip",
  });
  let seq = 0;
  const preview = async () => {
    const o = opts();
    if (!o.dest || !o.playlists.length) {
      pv.textContent = L("Laufwerk und Playlisten wählen …");
      return;
    }
    const my = ++seq;
    try {
      const p = await api.pioneerPreview(o);
      if (my !== seq) return;
      clear(pv);
      const line = (s: string, strong = false) => h("div", { style: strong ? { color: "var(--ink)" } : {} }, s);
      pv.append(
        line(L("{0} Titel · {1} · neu zu kopieren: {2} ({3})", p.tracks, fmtSize(p.bytes), p.toCopy, fmtSize(p.copyBytes)), true),
        line(p.free !== null ? L("Frei auf dem Laufwerk: {0}{1}", fmtSize(p.free), p.copyBytes > p.free ? L(" – reicht nicht!") : "") : L("Laufwerk nicht erreichbar")),
        line(p.missingAnalysis.length ? L("{0} Titel ohne Beatgrid/Wellenform – werden vor dem Export analysiert", p.missingAnalysis.length) : L("Alle Titel analysiert")),
        p.unsupported.length
          ? line(L("{0} Titel in Formaten, die die Player nicht abspielen ({1}) – {2}", p.unsupported.length, [...new Set(p.unsupported.map((u) => u[2].toUpperCase()))].join(", "), conv.checked ? (p.ffmpeg ? L("werden nach MP3 umgewandelt") : L("Umwandeln braucht ffmpeg (im Ripper laden) – werden übersprungen")) : L("werden übersprungen")), true)
          : "",
        p.duplicatesInPlaylists ? line(L("{0}× kommt ein Titel in einer Playlist doppelt vor – die Player-Bibliothek speichert ihn dort nur einmal", p.duplicatesInPlaylists)) : "",
        line(p.existingLibrary ? L("Auf dem Laufwerk liegt schon eine Bibliothek (z. B. von rekordbox) – sie wird ergänzt, vorhandene Einträge bleiben unverändert.") : L("Auf dem Laufwerk wird eine neue Bibliothek angelegt.")),
      );
    } catch (e) {
      pv.textContent = trMsg(String(e));
    }
  };
  const plBox = h("div", { style: { maxHeight: "200px", overflowY: "auto" } },
    ...store.playlists.map((p) => {
      const c = h("input", { type: "checkbox", checked: preselect.includes(p.id) }) as HTMLInputElement;
      c.addEventListener("change", preview);
      checks.set(p.id, c);
      return h("div", {}, h("label", { class: "check", style: { padding: "4px 0" } }, c, (p.rules ? "◇ " : "") + p.name, h("span", { class: "sub" }, L(" · {0} Titel", p.count))));
    }),
  );
  dest.addEventListener("change", preview);
  conv.addEventListener("change", () => { localStorage.setItem("ml.pioneerConvert", conv.checked ? "1" : "0"); preview(); });
  const s = sheet(L("Export für Pioneer DJ / AlphaTheta-Player (OneLibrary)"), h("div", {},
    h("div", { class: "note" }, L("Für CDJ-3000, CDJ-3000X, XDJ-AZ, OPUS-QUAD, OMNIS-DUO und weitere OneLibrary-Geräte. Kopiert Musik, Beatgrids, Wellenformen, Cues und Playlisten auf den Stick. Ältere Player, die nur das klassische Format lesen, werden nicht unterstützt.")),
    h("div", { class: "card", style: { marginTop: "12px" } }, h("h3", {}, L("Playlisten")), plBox),
    h("div", { class: "card" }, h("h3", {}, L("Laufwerk")),
      h("div", { class: "line" }, h("div", { class: "grow" }, dest),
        h("button", { class: "btn", onclick: async () => { const d = await open({ directory: true }); if (d) { dest.value = d as string; preview(); } } }, L("Wählen …")))),
    h("div", { class: "line" }, h("label", { class: "check" }, conv, L("Nicht abspielbare Formate mit ffmpeg nach MP3 (320 kbit/s) umwandeln statt überspringen"))),
    h("div", { class: "card" }, h("h3", {}, L("Vorschau")), pv),
    h("div", { class: "foot" },
      h("button", { class: "btn", onclick: () => xmlDialog(opts().playlists) }, L("rekordbox-XML speichern …")),
      h("span", { class: "spacer" }),
      h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")),
      holdButton(L("Exportieren"), async () => {
        const o = opts();
        if (!o.dest || !o.playlists.length) return toast(L("Laufwerk und Playlisten wählen"));
        localStorage.setItem("ml.pioneerDest", o.dest);
        s.close();
        try {
          const r = await api.pioneerExport(o);
          reportErrors(r.errors, L("Export fertig: {0} Titel, {1} Playlist{2}", r.tracks, r.playlists, pl(r.playlists, "en")) +
            `${r.copied ? L(", {0} kopiert", r.copied) : ""}${r.converted ? L(", {0} umgewandelt", r.converted) : ""}${r.skipped ? L(", {0} übersprungen", r.skipped) : ""}`);
        } catch (e) {
          toast(String(e), 9000);
        }
      }, L("halten")),
    ),
  ), { wide: true });
  preview();
}

/** rekordbox-XML (Rückfallebene) */
export async function xmlDialog(playlists: number[]) {
  if (!playlists.length) return toast(L("Mindestens eine Playlist wählen"));
  const file = await save({ defaultPath: "rekordbox.xml", filters: [{ name: "rekordbox xml", extensions: ["xml"] }] });
  if (!file) return;
  try {
    await api.pioneerXml(playlists, file);
    toast(L("rekordbox-XML gespeichert. In rekordbox unter Einstellungen → Erweitert → rekordbox xml einbinden."), 9000);
  } catch (e) {
    toast(String(e));
  }
}

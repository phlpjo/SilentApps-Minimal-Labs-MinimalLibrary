// Datei-Operationen, die aus mehreren Ansichten genutzt werden
import { open } from "@tauri-apps/plugin-dialog";
import { api, type DupCheck } from "./api";
import { store } from "./store";
import { basename, h, reportErrors, sheet, toast } from "./ui";

const normPath = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();

/** Musikordner, in dem ein Pfad liegt (längster Treffer) */
export function rootOf(path: string, roots = store.settings.roots): string | null {
  const p = normPath(path) + "/";
  let best: string | null = null;
  for (const r of roots) if (p.startsWith(normPath(r) + "/") && (!best || r.length > best.length)) best = r;
  return best;
}

const REASON: Record<string, string> = { metadata: "gleiche Tags", filename: "ähnlicher Dateiname", audio: "gleicher Klang" };

/**
 * Vor dem Kopieren/Verschieben in einen anderen Musikordner (oder Tab): gibt es die Titel dort schon?
 * Bei Treffern immer nachfragen. Ergebnis: Titel, die übertragen werden sollen – oder null (abgebrochen).
 */
export async function dupGate(ids: number[], dir: string, tab?: string): Promise<number[] | null> {
  let hits: DupCheck[];
  try {
    hits = await api.dupCheck(ids, dir, tab);
  } catch (e) {
    toast(String(e));
    return ids;
  }
  if (!hits.length) return ids;
  return new Promise((resolve) => {
    let done = false;
    const finish = (v: number[] | null) => {
      if (done) return;
      done = true;
      s.close();
      resolve(v);
    };
    const dupIds = new Set(hits.map((x) => x.trackId));
    const rows = hits.slice(0, 50).map((x) => {
      const t = store.tracks.get(x.trackId);
      const why = x.reasons.map((r) => REASON[r] ?? r).join(", ") + (x.audioSimilarity ? ` (${Math.round(x.audioSimilarity * 100)} %)` : "");
      return h("div", { class: "dup-hit" },
        h("div", {}, t ? `${t.artist ?? "?"} – ${store.label(t)}` : `Titel ${x.trackId}`),
        ...x.matches.map((m) => h("div", { class: "note", style: { fontFamily: "var(--mono)", fontSize: "11px" } }, `schon da: ${m.path}`)),
        h("div", { class: "note" }, why),
      );
    });
    const s = sheet(`${dupIds.size} von ${ids.length} Titel${ids.length === 1 ? "" : "n"} gibt es am Ziel schon`, h("div", {},
      h("div", { style: { maxHeight: "46vh", overflowY: "auto" } }, ...rows, hits.length > 50 ? h("div", { class: "note" }, `… und ${hits.length - 50} weitere`) : ""),
      h("div", { class: "foot" },
        h("button", { class: "btn", onclick: () => finish(null) }, "Abbrechen"),
        h("span", { class: "spacer" }),
        h("button", { class: "btn", onclick: () => finish(ids) }, "Trotzdem alle"),
        h("button", { class: "btn primary", onclick: () => finish(ids.filter((i) => !dupIds.has(i))) }, "Doppelte überspringen"),
      ),
    ), { onClose: () => finish(null) });
  });
}

/** Titel in einen Ordner verschieben (oder kopieren) – mit Verlauf und Rückgängig unter „Sortieren“.
 *  Geht es in einen anderen Musikordner, wird vorher auf Duplikate am Ziel geprüft. */
export async function moveTracksTo(ids: number[], dir: string, copy = false) {
  if (!ids.length) return;
  const target = rootOf(dir + "/x") ?? dir;
  const crossing = ids.some((id) => {
    const t = store.tracks.get(id);
    return t && rootOf(t.path) !== target;
  });
  if (crossing) {
    const ok = await dupGate(ids, dir);
    if (!ok) return;
    if (!ok.length) return toast("Nichts übertragen – alle Titel gibt es am Ziel schon");
    ids = ok;
  }
  const r = await api.moveTracksTo(ids, dir, copy).catch((e) => ({ moved: 0, errors: [String(e)], batchId: 0 }));
  reportErrors(r.errors, `${r.moved} Datei${r.moved === 1 ? "" : "en"} ${copy ? "kopiert" : "verschoben"} → ${basename(dir)}`);
  // Ziel außerhalb der Musikordner → als Musikordner aufnehmen
  if (!store.settings.roots.some((root) => dir.toLowerCase().startsWith(root.toLowerCase()))) {
    await store.saveSettings({ roots: [...store.settings.roots, dir] });
  }
  if (!copy) {
    ids.forEach((id) => store.selection.delete(id));
    store.emit("selection");
  }
  await store.loadTracks();
  store.loadPlaylists();
  store.emit("dirs");
}

export async function moveTracksPick(ids: number[], copy = false) {
  const d = await open({ directory: true, defaultPath: store.settings.roots[0] });
  if (d) await moveTracksTo(ids, d as string, copy);
}

export function renameFileDialog(id: number) {
  const t = store.tracks.get(id);
  if (!t) return;
  const file = basename(t.path);
  const dot = file.lastIndexOf(".");
  const stem = dot > 0 ? file.slice(0, dot) : file;
  const extn = dot > 0 ? file.slice(dot) : "";
  const inp = h("input", { type: "text", value: stem, style: { width: "100%" } });
  const go = async () => {
    const name = inp.value.trim();
    s.close();
    if (!name || name === stem) return;
    const r = await api.renameTrack(id, name).catch((e) => ({ moved: 0, errors: [String(e)], batchId: 0 }));
    reportErrors(r.errors, "Umbenannt");
    await store.refreshTracks([id]);
    store.emit("dirs");
  };
  inp.addEventListener("keydown", (e) => e.key === "Enter" && go());
  const s = sheet("Datei umbenennen", h("div", {},
    h("div", { class: "line" }, h("div", { class: "grow" }, inp), h("span", { class: "sub" }, extn)),
    h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"), h("button", { class: "btn primary", onclick: go }, "Umbenennen")),
  ));
  setTimeout(() => inp.select());
}

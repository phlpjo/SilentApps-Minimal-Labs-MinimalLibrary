// Datei-Operationen, die aus mehreren Ansichten genutzt werden
import { open } from "@tauri-apps/plugin-dialog";
import { api, type DupCheck } from "./api";
import { store } from "./store";
import { basename, h, reportErrors, sheet, toast } from "./ui";
import { L, pl } from "./i18n";

const normPath = (p: string) => p.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();

/** Musikordner, in dem ein Pfad liegt (längster Treffer) */
export function rootOf(path: string, roots = store.settings.roots): string | null {
  const p = normPath(path) + "/";
  let best: string | null = null;
  for (const r of roots) if (p.startsWith(normPath(r) + "/") && (!best || r.length > best.length)) best = r;
  return best;
}

const REASON: Record<string, string> = { metadata: L("gleiche Tags"), filename: L("ähnlicher Dateiname"), audio: L("gleicher Klang") };

/**
 * Vor dem Kopieren/Verschieben in einen anderen Musikordner (oder Tab): gibt es die Titel dort schon?
 * Bei Treffern immer nachfragen. Ergebnis: Titel, die übertragen werden sollen – oder null (abgebrochen).
 */
/** from/names: Titel aus einem anderen Tab (Namen, weil sie hier nicht in der Bibliothek sind) */
export async function dupGate(ids: number[], dir: string, tab?: string, from?: string, names?: Record<number, string>): Promise<number[] | null> {
  let hits: DupCheck[];
  try {
    hits = await api.dupCheck(ids, dir, tab, from);
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
        h("div", {}, names?.[x.trackId] ?? (t ? `${t.artist ?? "?"} – ${store.label(t)}` : L("Titel {0}", x.trackId))),
        ...x.matches.map((m) => h("div", { class: "note", style: { fontFamily: "var(--mono)", fontSize: "11px" } }, L("schon da: {0}", m.path))),
        h("div", { class: "note" }, why),
      );
    });
    const s = sheet(L("{0} von {1} Titel{2} gibt es am Ziel schon", dupIds.size, ids.length, pl(ids.length, "n")), h("div", {},
      h("div", { style: { maxHeight: "46vh", overflowY: "auto" } }, ...rows, hits.length > 50 ? h("div", { class: "note" }, L("… und {0} weitere", hits.length - 50)) : ""),
      h("div", { class: "foot" },
        h("button", { class: "btn", onclick: () => finish(null) }, L("Abbrechen")),
        h("span", { class: "spacer" }),
        h("button", { class: "btn", onclick: () => finish(ids) }, L("Trotzdem alle")),
        h("button", { class: "btn primary", onclick: () => finish(ids.filter((i) => !dupIds.has(i))) }, L("Doppelte überspringen")),
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
    if (!ok.length) return toast(L("Nichts übertragen – alle Titel gibt es am Ziel schon"));
    ids = ok;
  }
  const r = await api.moveTracksTo(ids, dir, copy).catch((e) => ({ moved: 0, errors: [String(e)], batchId: 0 }));
  reportErrors(r.errors, L("{0} Datei{1} {2} → {3}", r.moved, pl(r.moved, "en"), copy ? L("kopiert") : L("verschoben"), basename(dir)));
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
    reportErrors(r.errors, L("Umbenannt"));
    await store.refreshTracks([id]);
    store.emit("dirs");
  };
  inp.addEventListener("keydown", (e) => e.key === "Enter" && go());
  const s = sheet(L("Datei umbenennen"), h("div", {},
    h("div", { class: "line" }, h("div", { class: "grow" }, inp), h("span", { class: "sub" }, extn)),
    h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")), h("button", { class: "btn primary", onclick: go }, L("Umbenennen"))),
  ));
  setTimeout(() => inp.select());
}

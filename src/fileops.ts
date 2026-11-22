// Datei-Operationen, die aus mehreren Ansichten genutzt werden
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { store } from "./store";
import { basename, h, reportErrors, sheet } from "./ui";

/** Titel in einen Ordner verschieben (oder kopieren) – mit Verlauf und Rückgängig unter „Sortieren“. */
export async function moveTracksTo(ids: number[], dir: string, copy = false) {
  if (!ids.length) return;
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

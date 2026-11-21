import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type DirEntry } from "../api";
import { player } from "../player";
import { store } from "../store";
import { TrackList } from "../tracklist";
import { basename, clear, contextMenu, dirname, fmtDuration, h, holdButton, reportErrors, sheet, toast, type MenuItem } from "../ui";
import { moveTracksTo } from "../fileops";
import { analyzeIds } from "../main";
import { trackMenu } from "./library";

const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();
const DIR_MIME = "application/x-ml-dir";
const IDS_MIME = "application/x-ml-ids";

/** Ordner wie im Windows-Explorer – mit Wellenformen, Tags, Analyse. */
export function foldersView(startPath?: string): { el: HTMLElement; destroy: () => void } {
  const roots = () => store.settings.roots;
  // aufgeklappte Ordner: normalisiert → echter Pfad
  const expanded = new Map<string, string>((JSON.parse(localStorage.getItem("ml.expanded") ?? "[]") as string[]).map((p) => [norm(p), p]));
  const saveExpanded = () => localStorage.setItem("ml.expanded", JSON.stringify([...expanded.values()]));
  const expand = (p: string) => expanded.set(norm(p), p);
  const children = new Map<string, DirEntry[]>();
  let current = startPath ? dirname(startPath) : localStorage.getItem("ml.folder") ?? roots()[0] ?? "";
  if (current && !roots().some((r) => norm(current).startsWith(norm(r)))) current = roots()[0] ?? "";
  let recursive = localStorage.getItem("ml.recursive") === "1";

  const tree = h("div", { class: "tree" });
  const crumbs = h("div", { class: "crumbs" });
  const info = h("span", { class: "sub" });
  const tiles = h("div", { class: "tiles" });
  const recBox = h("input", { type: "checkbox", checked: recursive });
  recBox.addEventListener("change", () => {
    recursive = recBox.checked;
    localStorage.setItem("ml.recursive", recursive ? "1" : "0");
    showTracks();
  });

  const list = new TrackList({
    defaultSort: "pos",
    menu: (ids) => trackMenu(ids),
    emptyText: "Keine Musik in diesem Ordner. Titel aus der Liste auf einen Ordner links ziehen, um sie hierher zu verschieben.",
  });
  list.sort = "title";

  // ---------- Daten ----------
  const load = async (path: string, force = false) => {
    if (!force && children.has(norm(path))) return children.get(norm(path))!;
    const c = await api.dirList(path).catch(() => [] as DirEntry[]);
    children.set(norm(path), c);
    return c;
  };

  const reloadAll = async () => {
    const paths = [...roots(), ...expanded.values(), current].filter(Boolean);
    children.clear();
    await Promise.all(paths.map((p) => load(p, true)));
    renderTree();
    renderTiles();
  };

  // ---------- Baum ----------
  const node = (name: string, path: string, depth: number, hasChildren: boolean, count: number | null, isRoot: boolean): HTMLElement[] => {
    const key = norm(path);
    const open = expanded.has(key);
    const caret = h("span", { class: "caret" }, hasChildren ? (open ? "▾" : "▸") : "");
    const row = h("div", {
      class: "tnode" + (norm(current) === key ? " active" : ""),
      style: { paddingLeft: 8 + depth * 14 + "px" },
      title: path,
      draggable: isRoot ? "false" : "true",
    }, caret, h("span", { class: "tname" }, name), count ? h("span", { class: "count" }, String(count)) : "");
    caret.addEventListener("click", (e) => { e.stopPropagation(); toggle(path); });
    row.addEventListener("click", () => openFolder(path));
    row.addEventListener("dblclick", () => toggle(path));
    row.addEventListener("contextmenu", (e) => { e.preventDefault(); contextMenu(e.clientX, e.clientY, folderMenu(path, isRoot)); });
    if (!isRoot) row.addEventListener("dragstart", (e) => { e.dataTransfer!.setData(DIR_MIME, path); e.dataTransfer!.effectAllowed = "move"; });
    dropTarget(row, path, () => { if (hasChildren && !open) toggle(path); });
    const out: HTMLElement[] = [row];
    if (open) for (const c of children.get(key) ?? []) out.push(...node(c.name, c.path, depth + 1, c.hasChildren, c.tracks, false));
    return out;
  };

  const renderTree = () => {
    if (!roots().length) {
      tree.replaceChildren(h("div", { class: "note", style: { padding: "12px" } }, "Noch kein Musikordner – in den Einstellungen hinzufügen."));
      return;
    }
    const nodes: HTMLElement[] = [];
    for (const r of roots()) {
      const count = [...store.tracks.values()].filter((t) => norm(t.path).startsWith(norm(r) + "\\")).length;
      nodes.push(...node(basename(r.replace(/[\\/]+$/, "")) || r, r, 0, (children.get(norm(r)) ?? []).length > 0, count, true));
    }
    tree.replaceChildren(...nodes);
  };

  const toggle = async (path: string) => {
    const k = norm(path);
    if (expanded.has(k)) expanded.delete(k);
    else {
      expand(path);
      await load(path);
    }
    saveExpanded();
    renderTree();
  };

  // Ziehen auf einen Ordner: Titel verschieben (Strg = kopieren) oder Ordner verschieben
  let hoverTimer = 0;
  function dropTarget(el: HTMLElement, path: string, onHover?: () => void) {
    el.addEventListener("dragover", (e) => {
      const types = e.dataTransfer?.types ?? [];
      if (!types.includes(IDS_MIME) && !types.includes(DIR_MIME)) return;
      e.preventDefault();
      e.dataTransfer!.dropEffect = e.ctrlKey && types.includes(IDS_MIME) ? "copy" : "move";
      if (!el.classList.contains("drop")) {
        el.classList.add("drop");
        clearTimeout(hoverTimer);
        if (onHover) hoverTimer = window.setTimeout(onHover, 800); // aufklappen beim Drüberhalten
      }
    });
    el.addEventListener("dragleave", () => { el.classList.remove("drop"); clearTimeout(hoverTimer); });
    el.addEventListener("drop", async (e) => {
      e.preventDefault();
      el.classList.remove("drop");
      clearTimeout(hoverTimer);
      const dir = e.dataTransfer!.getData(DIR_MIME);
      if (dir) {
        if (norm(dir) === norm(path)) return;
        try {
          const np = await api.dirMove(dir, path);
          if (norm(current).startsWith(norm(dir))) current = np + current.slice(dir.length);
          toast(`„${basename(dir)}“ → ${basename(path)}`);
          await store.loadTracks();
          store.loadPlaylists();
          expand(path);
          saveExpanded();
          await reloadAll();
          showTracks();
        } catch (err) {
          toast(String(err));
        }
        return;
      }
      const ids: number[] = JSON.parse(e.dataTransfer!.getData(IDS_MIME) || "[]");
      await moveTracksTo(ids, path, e.ctrlKey);
    });
  }

  // ---------- Ordner öffnen ----------
  const openFolder = async (path: string) => {
    current = path;
    localStorage.setItem("ml.folder", path);
    store.selection.clear();
    store.emit("selection");
    // Pfad bis hierher aufklappen
    for (const r of roots()) {
      if (!norm(path).startsWith(norm(r))) continue;
      const parts = path.slice(r.replace(/[\\/]+$/, "").length).split(/[\\/]/).filter(Boolean);
      let p = r.replace(/[\\/]+$/, "");
      expand(p);
      await load(p);
      for (const part of parts.slice(0, -1)) {
        p += (p.includes("/") && !p.includes("\\") ? "/" : "\\") + part;
        expand(p);
        await load(p);
      }
    }
    saveExpanded();
    await load(path);
    renderTree();
    renderCrumbs();
    renderTiles();
    showTracks();
    // Im Explorer hinzugekommene Dateien automatisch aufnehmen
    const added = await api.dirImportUntracked(path).catch(() => [] as number[]);
    if (added.length) {
      await store.loadTracks();
      toast(`${added.length} neue Datei${added.length === 1 ? "" : "en"} im Ordner aufgenommen`);
      if (store.settings.autoAnalyze) analyzeIds(added);
    }
  };

  const renderCrumbs = () => {
    clear(crumbs);
    const root = roots().find((r) => norm(current).startsWith(norm(r)));
    if (!root) return;
    const base = root.replace(/[\\/]+$/, "");
    const parts = current.slice(base.length).split(/[\\/]/).filter(Boolean);
    const sep = base.includes("/") && !base.includes("\\") ? "/" : "\\";
    let p = base;
    const segs: [string, string][] = [[basename(base) || base, base]];
    for (const part of parts) {
      p += sep + part;
      segs.push([part, p]);
    }
    segs.forEach(([name, path], i) => {
      if (i) crumbs.append(h("span", { class: "csep" }, "/"));
      const c = h("span", { class: "crumb" + (i === segs.length - 1 ? " last" : "") }, name);
      c.addEventListener("click", () => openFolder(path));
      dropTarget(c, path);
      crumbs.append(c);
    });
  };

  const renderTiles = () => {
    clear(tiles);
    const subs = children.get(norm(current)) ?? [];
    const isRoot = roots().some((r) => norm(r) === norm(current));
    if (!isRoot && current) {
      const up = h("div", { class: "tile up", title: "Übergeordneter Ordner" }, "‹ Zurück");
      up.addEventListener("click", () => openFolder(dirname(current)));
      dropTarget(up, dirname(current));
      tiles.append(up);
    }
    for (const d of subs) {
      const t = h("div", { class: "tile", title: d.path, draggable: "true" }, h("span", { class: "tname" }, d.name), d.tracks ? h("span", { class: "count" }, String(d.tracks)) : "");
      t.addEventListener("click", () => openFolder(d.path));
      t.addEventListener("contextmenu", (e) => { e.preventDefault(); contextMenu(e.clientX, e.clientY, folderMenu(d.path, false)); });
      t.addEventListener("dragstart", (e) => { e.dataTransfer!.setData(DIR_MIME, d.path); e.dataTransfer!.effectAllowed = "move"; });
      dropTarget(t, d.path);
      tiles.append(t);
    }
    const nf = h("div", { class: "tile new" }, "+ Neuer Ordner");
    nf.addEventListener("click", () => newFolder(current));
    tiles.append(nf);
  };

  const showTracks = () => {
    const c = norm(current);
    const ids: number[] = [];
    let dur = 0;
    for (const t of store.tracks.values()) {
      const p = norm(t.path);
      const inside = recursive ? p.startsWith(c + "\\") : norm(dirname(t.path)) === c;
      if (inside) {
        ids.push(t.id);
        dur += t.duration;
      }
    }
    list.setIds(ids);
    player.queue = list.visibleIds;
    info.textContent = `${ids.length} Titel · ${fmtDuration(dur)}`;
  };

  // ---------- Ordner-Aktionen ----------
  const folderMenu = (path: string, isRoot: boolean): MenuItem[] => [
    { label: "Öffnen", action: () => openFolder(path) },
    { label: "Neuer Unterordner …", action: () => newFolder(path) },
    ...(isRoot ? [] : [{ label: "Umbenennen …", hint: "F2", action: () => renameFolder(path) }]),
    { label: "Im Explorer zeigen", action: () => revealItemInDir(path) },
    { label: "Alle Titel darin abspielen", action: () => {
      const ids = [...store.tracks.values()].filter((t) => norm(t.path).startsWith(norm(path) + "\\")).map((t) => t.id);
      if (ids.length) player.play(ids[0], 0, ids);
    } },
    ...(isRoot ? [] : [{ label: "", sep: true }, { label: "In den Papierkorb …", action: () => trashFolder(path) }]),
  ];

  const newFolder = (parent: string) => {
    const inp = h("input", { type: "text", value: "Neuer Ordner", style: { width: "100%" } });
    const go = async () => {
      s.close();
      try {
        const p = await api.dirCreate(parent, inp.value.trim() || "Neuer Ordner");
        expand(parent);
        saveExpanded();
        await load(parent, true);
        renderTree();
        renderTiles();
        toast(`Ordner „${basename(p)}“ angelegt`);
      } catch (e) {
        toast(String(e));
      }
    };
    inp.addEventListener("keydown", (e) => e.key === "Enter" && go());
    const s = sheet(`Neuer Ordner in „${basename(parent)}“`, h("div", {}, inp,
      h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"), h("button", { class: "btn primary", onclick: go }, "Anlegen"))));
    setTimeout(() => inp.select());
  };

  const renameFolder = (path: string) => {
    const inp = h("input", { type: "text", value: basename(path), style: { width: "100%" } });
    const go = async () => {
      s.close();
      if (!inp.value.trim() || inp.value.trim() === basename(path)) return;
      player.release([...store.tracks.values()].filter((t) => norm(t.path).startsWith(norm(path) + "\\")).map((t) => t.id));
      try {
        const np = await api.dirRename(path, inp.value.trim());
        if (norm(current).startsWith(norm(path))) current = np + current.slice(path.length);
        // aufgeklappte Pfade unterhalb mitziehen
        for (const [k, v] of [...expanded]) {
          if (k === norm(path) || k.startsWith(norm(path) + "\\")) {
            expanded.delete(k);
            expand(np + v.slice(path.length));
          }
        }
        saveExpanded();
        await store.loadTracks();
        store.loadPlaylists();
        await reloadAll();
        renderCrumbs();
        showTracks();
      } catch (e) {
        toast(String(e));
      }
    };
    inp.addEventListener("keydown", (e) => e.key === "Enter" && go());
    const s = sheet("Ordner umbenennen", h("div", {}, inp,
      h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"), h("button", { class: "btn primary", onclick: go }, "Umbenennen"))));
    setTimeout(() => inp.select());
  };

  const trashFolder = (path: string) => {
    const n = [...store.tracks.values()].filter((t) => norm(t.path).startsWith(norm(path) + "\\")).length;
    const s = sheet(`„${basename(path)}“ in den Papierkorb`, h("div", {},
      h("div", { class: "note" }, `Der Ordner mit allem Inhalt${n ? ` (${n} Titel)` : ""} landet im Windows-Papierkorb.`),
      h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
        holdButton("In den Papierkorb", async () => {
          s.close();
          try {
            await api.dirTrash(path);
            if (norm(current).startsWith(norm(path))) current = dirname(path);
            await store.loadTracks();
            store.loadPlaylists();
            await reloadAll();
            openFolder(current);
          } catch (e) {
            reportErrors([String(e)]);
          }
        }))));
  };

  const onKey = (e: KeyboardEvent) => {
    const tag = (e.target as HTMLElement).tagName;
    if (tag === "INPUT" || document.querySelector(".sheet-bg")) return;
    if (e.key === "Backspace" && current && !roots().some((r) => norm(r) === norm(current))) openFolder(dirname(current));
  };
  document.addEventListener("keydown", onKey);

  const unsubs = [
    store.on("tracks", () => { showTracks(); renderTree(); }),
    store.on("dirs", () => reloadAll()),
    store.on("settings", () => reloadAll()),
  ];

  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Ordner"), crumbs, info, h("div", { class: "spacer" }),
      h("label", { class: "check" }, recBox, "Unterordner einbeziehen"),
      h("button", { class: "btn", onclick: () => current && revealItemInDir(current) }, "Im Explorer"),
    ),
    h("div", { class: "folders" },
      tree,
      h("div", { class: "folder-main" }, tiles, list.el),
    ),
  );

  (async () => {
    await Promise.all([...roots(), ...expanded.values()].map((p) => load(p)));
    if (current) await openFolder(current);
    else renderTree();
  })();

  return { el, destroy: () => { unsubs.forEach((u) => u()); list.destroy(); document.removeEventListener("keydown", onKey); } };
}

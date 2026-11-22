import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type DirEntry } from "../api";
import { player } from "../player";
import { store } from "../store";
import { TrackList } from "../tracklist";
import { basename, clear, contextMenu, dirname, fmtDuration, h, holdButton, reportErrors, sheet, toast, type MenuItem } from "../ui";
import { moveTracksTo } from "../fileops";
import { beginDrag, dragging, dropTarget } from "../dnd";
import { analyzeIds } from "../main";
import { textMatches, trackMenu } from "./library";
import { SHOW_IN, TRASH, FILE_MANAGER } from "../platform";

const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();

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
  let query = "";
  const search = h("input", { type: "search", placeholder: "In diesem Ordner suchen …", style: { width: "210px", flex: "none" } });
  search.addEventListener("input", () => {
    query = search.value;
    showTracks();
    renderTiles();
  });
  search.addEventListener("keydown", (e) => {
    if (e.key === "Escape") {
      search.value = query = "";
      showTracks();
      renderTiles();
    }
  });
  const recBox = h("input", { type: "checkbox", checked: recursive });
  recBox.addEventListener("change", () => {
    recursive = recBox.checked;
    localStorage.setItem("ml.recursive", recursive ? "1" : "0");
    showTracks();
  });

  const list = new TrackList({
    columns: "folders",
    defaultSort: "pos",
    menu: (ids) => trackMenu(ids),
    emptyText: "Keine Musik in diesem Ordner. Titel aus der Liste auf einen Ordner links ziehen, um sie hierher zu verschieben.",
  });
  list.sort = (localStorage.getItem("ml.folderSort") as typeof list.sort) ?? "trackNo";

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
    }, caret, h("span", { class: "tname" }, name), count ? h("span", { class: "count" }, String(count)) : "");
    caret.addEventListener("click", (e) => { e.stopPropagation(); toggle(path); });
    row.addEventListener("click", () => !dragging && openFolder(path));
    row.addEventListener("dblclick", () => toggle(path));
    row.addEventListener("contextmenu", (e) => { e.preventDefault(); contextMenu(e.clientX, e.clientY, folderMenu(path, isRoot)); });
    if (!isRoot) row.addEventListener("pointerdown", (e) => beginDrag(e, () => ({ kind: "dir", path })));
    folderDrop(row, path, () => { if (hasChildren && !expanded.has(key)) toggle(path); });
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
  function folderDrop(el: HTMLElement, path: string, onHover?: () => void) {
    dropTarget(el, {
      accepts: (p) => p.kind === "ids" || (p.kind === "dir" && norm(p.path) !== norm(path) && !norm(path).startsWith(norm(p.path) + "\\")),
      enter: () => {
        clearTimeout(hoverTimer);
        if (onHover) hoverTimer = window.setTimeout(onHover, 800); // aufklappen beim Drüberhalten
      },
      leave: () => clearTimeout(hoverTimer),
      drop: async (p, { copy }) => {
        clearTimeout(hoverTimer);
        if (p.kind === "dir") {
          const dir = p.path;
          try {
            const np = await api.dirMove(dir, path);
            if (norm(current).startsWith(norm(dir))) current = np + current.slice(dir.length);
            toast(`„${basename(dir)}“ → ${basename(path)}`);
            await store.loadTracks();
            store.loadPlaylists();
            expand(path);
            saveExpanded();
            await reloadAll();
            renderCrumbs();
            showTracks();
          } catch (err) {
            toast(String(err));
          }
          return;
        }
        await moveTracksTo(p.ids, path, copy);
      },
    });
  }

  // ---------- Ordner öffnen ----------
  const openFolder = async (path: string) => {
    if (norm(path) !== norm(current)) search.value = query = "";
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
      folderDrop(c, path);
      crumbs.append(c);
    });
  };

  const renderTiles = () => {
    clear(tiles);
    const q = query.toLowerCase().trim();
    const subs = (children.get(norm(current)) ?? []).filter((d) => !q || q.split(/\s+/).every((w) => d.name.toLowerCase().includes(w)));
    const isRoot = roots().some((r) => norm(r) === norm(current));
    if (!isRoot && current) {
      const up = h("div", { class: "tile up", title: "Übergeordneter Ordner" }, "‹ Zurück");
      up.addEventListener("click", () => openFolder(dirname(current)));
      folderDrop(up, dirname(current));
      tiles.append(up);
    }
    for (const d of subs) {
      const t = h("div", { class: "tile", title: d.path }, h("span", { class: "tname" }, d.name), d.tracks ? h("span", { class: "count" }, String(d.tracks)) : "");
      t.addEventListener("click", () => !dragging && openFolder(d.path));
      t.addEventListener("contextmenu", (e) => { e.preventDefault(); contextMenu(e.clientX, e.clientY, folderMenu(d.path, false)); });
      t.addEventListener("pointerdown", (e) => beginDrag(e, () => ({ kind: "dir", path: d.path })));
      folderDrop(t, d.path);
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
    let total = 0;
    const words = query.toLowerCase().split(/\s+/).filter(Boolean);
    for (const t of store.tracks.values()) {
      const p = norm(t.path);
      const inside = recursive ? p.startsWith(c + "\\") : norm(dirname(t.path)) === c;
      if (inside) total++;
      // Suche nur im aktuellen Ordner (mit Unterordnern, wenn eingeschaltet)
      if (inside && textMatches(t, words, true)) {
        ids.push(t.id);
        dur += t.duration;
      }
    }
    list.setIds(ids);
    player.queue = list.visibleIds;
    info.textContent = words.length ? `${ids.length} von ${total} Titeln · ${fmtDuration(dur)}` : `${ids.length} Titel · ${fmtDuration(dur)}`;
  };

  // ---------- Ordner-Aktionen ----------
  const folderMenu = (path: string, isRoot: boolean): MenuItem[] => [
    { label: "Öffnen", action: () => openFolder(path) },
    { label: "Neuer Unterordner …", action: () => newFolder(path) },
    ...(isRoot ? [] : [{ label: "Umbenennen …", hint: "F2", action: () => renameFolder(path) }]),
    { label: SHOW_IN, action: () => revealItemInDir(path) },
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
      h("div", { class: "note" }, `Der Ordner mit allem Inhalt${n ? ` (${n} Titel)` : ""} landet im ${TRASH}.`),
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
    if (e.key === "f" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      search.focus();
      search.select();
      return;
    }
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
    h("div", { class: "head nowrap" }, h("h1", {}, "Ordner"), crumbs, info,
      search,
      h("label", { class: "check", title: "Titel aus allen Unterordnern mit anzeigen und durchsuchen" }, recBox, "Unterordner"),
      h("button", { class: "btn", onclick: () => current && revealItemInDir(current) }, FILE_MANAGER),
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

  return { el, destroy: () => { localStorage.setItem("ml.folderSort", list.sort); unsubs.forEach((u) => u()); list.destroy(); document.removeEventListener("keydown", onKey); } };
}

import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type DirEntry } from "../api";
import { player } from "../player";
import { store } from "../store";
import { TrackList } from "../tracklist";
import { basename, clear, contextMenu, dirname, fmtDuration, h, holdButton, reportErrors, sheet, toast, type MenuItem } from "../ui";
import { dupGate, moveTracksTo } from "../fileops";
import { MY_TAB } from "../tabs";
import { beginDrag, dragging, dropTarget } from "../dnd";
import { analyzeIds, recordNav, setViewPath } from "../main";
import { searchWords, textMatches, trackMenu } from "./library";
import { SHOW_IN, TRASH, FILE_MANAGER } from "../platform";
import { L, pl } from "../i18n";

const norm = (p: string) => p.replace(/[\\/]+$/, "").replace(/\//g, "\\").toLowerCase();

/** Ordner wie im Windows-Explorer – mit Wellenformen, Tags, Analyse. */
export function foldersView(startPath?: string): { el: HTMLElement; destroy: () => void } {
  const roots = () => store.settings.roots;
  // aufgeklappte Ordner: normalisiert → echter Pfad
  const expanded = new Map<string, string>((JSON.parse(localStorage.getItem("ml.expanded") ?? "[]") as string[]).map((p) => [norm(p), p]));
  const saveExpanded = () => localStorage.setItem("ml.expanded", JSON.stringify([...expanded.values()]));
  const expand = (p: string) => expanded.set(norm(p), p);
  const children = new Map<string, DirEntry[]>();
  // startPath: Datei (→ ihr Ordner, Titel markieren) oder Ordner (z. B. beim Zurückgehen)
  const startTrack = startPath ? [...store.tracks.values()].find((t) => norm(t.path) === norm(startPath)) : undefined;
  let current = startTrack ? dirname(startTrack.path) : startPath ?? localStorage.getItem("ml.folder") ?? roots()[0] ?? "";
  if (current && !roots().some((r) => norm(current).startsWith(norm(r)))) current = roots()[0] ?? "";
  let recursive = localStorage.getItem("ml.recursive") === "1";

  const tree = h("div", { class: "tree" });
  const crumbs = h("div", { class: "crumbs" });
  const info = h("span", { class: "sub" });
  const tiles = h("div", { class: "tiles" });
  let query = "";
  const search = h("input", { type: "search", placeholder: L("Ordner durchsuchen …  (Strg+F)"), title: L("Sucht in diesem Ordner und allen Unterordnern"), style: { width: "230px", flex: "none" } });
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
    emptyText: L("Keine Musik in diesem Ordner. Titel aus der Liste auf einen Ordner links ziehen, um sie hierher zu verschieben."),
  });
  list.sort = (localStorage.getItem("ml.folderSort") as typeof list.sort) ?? "trackNo";

  // ---------- Daten ----------
  const load = async (path: string, force = false) => {
    if (!force && children.has(norm(path))) return children.get(norm(path))!;
    const c = await api.dirList(path).catch(() => [] as DirEntry[]);
    children.set(norm(path), c);
    return c;
  };

  let countT = 0;
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
      tree.replaceChildren(h("div", { class: "note", style: { padding: "12px" } }, L("Noch kein Musikordner – in den Einstellungen hinzufügen.")));
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
      accepts: (p) => p.kind === "ids" || p.kind === "tab" || (p.kind === "dir" && norm(p.path) !== norm(path) && !norm(path).startsWith(norm(p.path) + "\\")),
      enter: () => {
        clearTimeout(hoverTimer);
        if (onHover) hoverTimer = window.setTimeout(onHover, 800); // aufklappen beim Drüberhalten
      },
      leave: () => clearTimeout(hoverTimer),
      drop: async (p, { copy, e }) => {
        clearTimeout(hoverTimer);
        if (p.kind === "tab") {
          // aus einem anderen Tab: verschieben oder kopieren?
          const run = (cp: boolean) => fromOtherTab(p, path, cp);
          contextMenu(e.clientX, e.clientY, [
            { label: L("Hierher verschieben"), action: () => run(false) },
            { label: L("Hierher kopieren"), action: () => run(true) },
            { label: "", sep: true },
            { label: L("Abbrechen"), action: () => {} },
          ]);
          return;
        }
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
        if (p.kind === "ids") await moveTracksTo(p.ids, path, copy);
      },
    });
  }

  /** Titel aus einem anderen Tab in diesen Ordner übernehmen (mit Duplikatprüfung) */
  async function fromOtherTab(p: { from: string; fromName: string; ids: number[]; names: Record<number, string> }, path: string, copy: boolean) {
    const ok = await dupGate(p.ids, path, MY_TAB, p.from, p.names);
    if (!ok) return;
    if (!ok.length) return toast(L("Nichts übertragen – alle Titel gibt es dort schon"));
    const skip = p.ids.filter((i) => !ok.includes(i));
    try {
      const r = await api.transferToTab(p.ids, MY_TAB, path, copy, skip, p.from);
      reportErrors(r.errors, L("{0} Titel {1} → „{2}“", r.moved, copy ? L("kopiert") : L("verschoben"), basename(path)));
      await store.loadTracks();
      await reloadAll();
      showTracks();
    } catch (err) {
      toast(String(err));
    }
  }

  // ---------- Ordner öffnen ----------
  let ready = false;
  const openFolder = async (path: string) => {
    if (ready && norm(path) !== norm(current)) recordNav(); // jeder Ordnerwechsel ist ein Schritt für „Zurück“
    setViewPath(path);
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
      toast(L("{0} neue Datei{1} im Ordner aufgenommen", added.length, pl(added.length, "en")));
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
      const up = h("div", { class: "tile up", title: L("Übergeordneter Ordner") }, L("‹ Zurück"));
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
    const nf = h("div", { class: "tile new" }, L("+ Neuer Ordner"));
    nf.addEventListener("click", () => newFolder(current));
    tiles.append(nf);
  };

  const showTracks = () => {
    const c = norm(current);
    const ids: number[] = [];
    let dur = 0;
    let total = 0;
    const words = searchWords(query);
    for (const t of store.tracks.values()) {
      const p = norm(t.path);
      // Beim Suchen immer mit allen Unterordnern
      const deep = recursive || words.length > 0;
      const inside = deep ? p.startsWith(c + "\\") : norm(dirname(t.path)) === c;
      if (inside) total++;
      // Suche nur im aktuellen Ordner (mit Unterordnern, wenn eingeschaltet)
      if (inside && textMatches(t, words, true)) {
        ids.push(t.id);
        dur += t.duration;
      }
    }
    list.setIds(ids);
    player.queue = list.visibleIds;
    info.textContent = words.length ? L("{0} von {1} Titeln · {2}", ids.length, total, fmtDuration(dur)) : L("{0} Titel · {1}", ids.length, fmtDuration(dur));
  };

  // ---------- Ordner-Aktionen ----------
  const folderMenu = (path: string, isRoot: boolean): MenuItem[] => [
    { label: L("Öffnen"), action: () => openFolder(path) },
    { label: L("Neuer Unterordner …"), action: () => newFolder(path) },
    ...(isRoot ? [] : [{ label: L("Umbenennen …"), hint: "F2", action: () => renameFolder(path) }]),
    { label: SHOW_IN, action: () => revealItemInDir(path) },
    { label: L("Alle Titel darin abspielen"), action: () => {
      const ids = [...store.tracks.values()].filter((t) => norm(t.path).startsWith(norm(path) + "\\")).map((t) => t.id);
      if (ids.length) player.play(ids[0], 0, ids);
    } },
    ...(isRoot ? [] : [{ label: "", sep: true }, { label: L("In den Papierkorb …"), action: () => trashFolder(path) }]),
  ];

  const newFolder = (parent: string) => {
    const inp = h("input", { type: "text", value: L("Neuer Ordner"), style: { width: "100%" } });
    const go = async () => {
      s.close();
      try {
        const p = await api.dirCreate(parent, inp.value.trim() || L("Neuer Ordner"));
        expand(parent);
        saveExpanded();
        await load(parent, true);
        renderTree();
        renderTiles();
        toast(L("Ordner „{0}“ angelegt", basename(p)));
      } catch (e) {
        toast(String(e));
      }
    };
    inp.addEventListener("keydown", (e) => e.key === "Enter" && go());
    const s = sheet(L("Neuer Ordner in „{0}“", basename(parent)), h("div", {}, inp,
      h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")), h("button", { class: "btn primary", onclick: go }, L("Anlegen")))));
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
    const s = sheet(L("Ordner umbenennen"), h("div", {}, inp,
      h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")), h("button", { class: "btn primary", onclick: go }, L("Umbenennen")))));
    setTimeout(() => inp.select());
  };

  const trashFolder = (path: string) => {
    const n = [...store.tracks.values()].filter((t) => norm(t.path).startsWith(norm(path) + "\\")).length;
    const s = sheet(L("„{0}“ in den Papierkorb", basename(path)), h("div", {},
      h("div", { class: "note" }, L("Der Ordner mit allem Inhalt{0} landet im {1}.", n ? L(" ({0} Titel)", n) : "", TRASH)),
      h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")),
        holdButton(L("In den Papierkorb"), async () => {
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
    store.on("tracks", () => {
      showTracks();
      renderTree();
      // Zahlen im Ordnerbaum frisch holen (z. B. während ein Scan läuft) – höchstens alle 4 s
      if (!countT) countT = window.setTimeout(() => { countT = 0; reloadAll(); }, 4000);
    }),
    store.on("dirs", () => reloadAll()),
    store.on("settings", () => reloadAll()),
  ];

  // Ordnerbaum: Breite am Trennstrich ziehen (wird gemerkt)
  const splitter = h("div", { class: "splitter", title: L("Breite ziehen") });
  const foldersBox = h("div", { class: "folders" }, tree, splitter, h("div", { class: "folder-main" }, list.el));
  try { const w = Number(localStorage.getItem("ml.treeW")); if (w) foldersBox.style.setProperty("--tree-w", w + "px"); } catch {}
  splitter.addEventListener("pointerdown", (e) => {
    e.preventDefault();
    const x0 = e.clientX;
    const w0 = tree.getBoundingClientRect().width;
    let w = w0;
    const move = (ev: PointerEvent) => {
      w = Math.max(160, Math.min(window.innerWidth * 0.6, w0 + ev.clientX - x0));
      foldersBox.style.setProperty("--tree-w", Math.round(w) + "px");
    };
    const up = () => {
      window.removeEventListener("pointermove", move, true);
      window.removeEventListener("pointerup", up, true);
      document.body.classList.remove("is-resizing");
      try { localStorage.setItem("ml.treeW", String(Math.round(w))); } catch {}
    };
    document.body.classList.add("is-resizing");
    window.addEventListener("pointermove", move, true);
    window.addEventListener("pointerup", up, true);
  });
  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head nowrap" }, h("h1", {}, L("Ordner")), crumbs, info,
      search,
      h("label", { class: "check", title: L("Titel aus allen Unterordnern mit anzeigen und durchsuchen") }, recBox, L("Unterordner")),
      h("button", { class: "btn", onclick: () => current && revealItemInDir(current) }, FILE_MANAGER),
    ),
    foldersBox,
  );

  (async () => {
    await Promise.all([...roots(), ...expanded.values()].map((p) => load(p)));
    if (current) await openFolder(current);
    else renderTree();
    ready = true;
    if (startTrack) requestAnimationFrame(() => list.reveal(startTrack.id));
  })();

  return { el, destroy: () => { localStorage.setItem("ml.folderSort", list.sort); unsubs.forEach((u) => u()); list.destroy(); document.removeEventListener("keydown", onKey); } };
}

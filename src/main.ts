import { lyricsPanelOpen, toggleLyricsPanel } from "./lyrics";
import { isStarred, toggleStar } from "./star";
import "./style.css";
import { IS_MAC } from "./platform";
import { applyTheme, isLight } from "./theme";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type LibraryChange } from "./api";
import { Inspector } from "./inspector";
import { player } from "./player";
import { store } from "./store";
import { albumLink, artistLinks, onAlbumClick, clear, contextMenu, fmtTime, h, onArtistClick, sheet, toast } from "./ui";
import { drawWave, waveElement } from "./waveform";
import { beginDrag, dragging, dropTarget } from "./dnd";
import { duplicatesView } from "./views/duplicates";
import { libraryView, trackMenu } from "./views/library";
import { foldersView } from "./views/folders";
import { historyView, undo } from "./views/history";
import { ripperView } from "./views/ripper";
import { syncView, startSyncWatch, syncReadyCount } from "./views/sync";
import { equalizerView } from "./views/equalizer";
import { searchView } from "./views/search";
import { startUpdater, updater } from "./updater";
import { icon } from "./icons";
import { MY_TAB, audioOwner, reportPlaying, tabStrip } from "./tabs";
import { pioneerExportDialog, pioneerOn, xmlDialog } from "./pioneer";
import { currentOutputLabel, outputPopup, restoreOutput } from "./outputs";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { organizeView } from "./views/organize";
import { exportDialog, playlistView, smartEditor } from "./views/playlist";
import { settingsView } from "./views/settings";
import logo from "./logo.svg";
import { L, pl, LOCALE, trMsg, LANG, shown } from "./i18n";
api.setLang(LANG).catch(() => {});

type ViewName = "library" | "search" | "folders" | "history" | "ripper" | "sync" | "equalizer" | "playlist" | "duplicates" | "organize" | "settings";

const app = document.getElementById("app")!;
const win = getCurrentWindow();
const backBtn = h("button", { class: "tb-btn back", title: IS_MAC ? L("Zurück (⌘[)") : L("Zurück (Alt+←)"), onclick: () => goBack() }, "‹");
function updateBack() {
  backBtn.disabled = backStack.length === 0;
}
const maxBtn = h("button", { class: "tb-btn", title: L("Maximieren"), onclick: () => win.toggleMaximize() }, "▢");
const titlebar = h("header", { id: "titlebar", "data-tauri-drag-region": "" },
  backBtn,
  tabStrip(),
  h("div", { class: "tb-title", "data-tauri-drag-region": "" }),
  IS_MAC ? null : h("div", { class: "tb-ctrls" },
    h("button", { class: "tb-btn", title: L("Minimieren"), onclick: () => win.minimize() }, "—"),
    maxBtn,
    h("button", { class: "tb-btn close", title: L("Schließen"), onclick: () => win.close() }, "✕"),
  ),
);
titlebar.addEventListener("dblclick", (e) => {
  if ((e.target as HTMLElement).hasAttribute("data-tauri-drag-region")) win.toggleMaximize();
});
win.onResized(async () => {
  const m = await win.isMaximized();
  maxBtn.textContent = m ? "❐" : "▢";
  maxBtn.title = m ? L("Wiederherstellen") : L("Maximieren");
  document.body.classList.toggle("maximized", m);
});
// Fensterrahmen-Farbe an Hell/Dunkel anpassen
const syncFrame = () => win.setTheme(isLight() ? "light" : "dark").catch(() => {});
document.addEventListener("themechange", syncFrame);
syncFrame();
// Maus-Zurück-Taste und Alt+←
window.addEventListener("mouseup", (e) => {
  if (e.button === 3) {
    e.preventDefault();
    goBack();
  }
});
window.addEventListener("keydown", (e) => {
  if ((!IS_MAC && e.altKey && e.key === "ArrowLeft") || (IS_MAC && e.metaKey && (e.key === "[" || e.key === "ArrowLeft"))) {
    e.preventDefault();
    goBack();
  }
});
const side = h("nav", { id: "side" });
const main = h("main", { id: "main" });
const insp = h("aside", { id: "inspector" });
const playerBar = h("footer", { id: "player" });
app.append(titlebar, side, main, insp, playerBar);
new Inspector(insp);

let current: { name: ViewName; arg?: number[]; path?: string; destroy: () => void } | null = null;
type HistEntry = { name: ViewName; arg?: number[]; path?: string; scroll: number[]; session: Record<string, string | null> };
const backStack: HistEntry[] = [];
/** Ansichts-Zustand, der beim Zurückgehen wiederhergestellt wird (Suche, Filter, Sortierung, erweiterte Suche) */
const SESSION_KEYS = ["ml.query", "ml.filter", "ml.sort", "ml.adv"];

/** Aktuellen Stand auf den Zurück-Stapel legen – vor jeder Bewegung (Ansicht, Ordner, Suche per Klick) */
export function recordNav() {
  if (!current) return;
  backStack.push({
    name: current.name,
    arg: current.arg,
    path: current.path,
    scroll: captureScroll(),
    session: Object.fromEntries(SESSION_KEYS.map((k) => [k, sessionStorage.getItem(k)])),
  });
  if (backStack.length > 80) backStack.shift();
  updateBack();
}

/** Ansicht meldet ihren Ort (z. B. aktueller Ordner), damit „Zurück“ genau dorthin führt */
export function setViewPath(path: string) {
  if (current) current.path = path;
}

/** Scrollpositionen der Ansicht merken (Liste, Baum, Inhalt) */
function captureScroll(): number[] {
  return [...main.querySelectorAll<HTMLElement>(".list-body, .view-scroll, .tree, .tiles")].map((e) => e.scrollTop);
}

/** … und nach dem Neuaufbau wiederherstellen (mehrfach, weil Inhalte teils nachladen) */
function restoreScroll(pos: number[]) {
  if (!pos.some((p) => p > 0)) return;
  const apply = () => {
    const els = [...main.querySelectorAll<HTMLElement>(".list-body, .view-scroll, .tree, .tiles")];
    pos.forEach((p, i) => {
      const el = els[i];
      if (el && el.scrollTop !== p) {
        el.scrollTop = p;
        el.dispatchEvent(new Event("scroll"));
      }
    });
  };
  [0, 60, 200, 450, 900].forEach((ms) => setTimeout(apply, ms));
}

/** Zurück zur vorherigen Ansicht (Zurück-Taste der Maus, Alt+←, ‹ oben links) */
export function goBack() {
  const prev = backStack.pop();
  if (prev) {
    for (const [k, v] of Object.entries(prev.session)) v === null ? sessionStorage.removeItem(k) : sessionStorage.setItem(k, v);
    nav(prev.name, prev.arg, prev.path, true);
    restoreScroll(prev.scroll);
  }
  updateBack();
}

export function nav(name: ViewName, arg?: number[], path?: string, fromBack = false) {
  if (current && !fromBack && !(current.name === name && JSON.stringify(current.arg ?? null) === JSON.stringify(arg ?? null) && !path)) recordNav();
  current?.destroy();
  clear(main);
  let v: { el: HTMLElement; destroy: () => void };
  if (name !== "library") store.selection.clear();
  switch (name) {
    case "library": v = libraryView(); break;
    case "folders": v = foldersView(path); break;
    case "search": v = searchView(); break;
    case "history": v = historyView(); break;
    case "ripper": v = ripperView(); break;
    case "sync": v = syncView(); break;
    case "equalizer": v = equalizerView(); break;
    case "playlist": v = playlistView(arg![0]); break;
    case "duplicates": v = duplicatesView(); break;
    case "organize": v = organizeView(arg); break;
    case "settings": v = settingsView(); break;
  }
  main.appendChild(v.el);
  current = { name, arg, path, destroy: v.destroy };
  updateBack();
  app.classList.toggle("no-insp", !(name === "library" || name === "search" || name === "playlist" || name === "folders"));
  store.emit("selection");
  renderSide();
}

/** Klick auf Interpret/Album: Bibliothek in diesem Feld durchsuchen (Zurück führt an die alte Stelle) */
function searchField(field: "artist" | "album", text: string) {
  recordNav();
  sessionStorage.setItem("ml.query", text);
  sessionStorage.setItem("ml.filter", field);
  nav("library", undefined, undefined, true);
}
export const searchArtist = (name: string) => searchField("artist", name);
onArtistClick(searchArtist);
onAlbumClick((name) => searchField("album", name));

/** Laufenden Titel an seinem Speicherort zeigen (Ordneransicht, markiert und hingescrollt) */
function showPlaying() {
  const t = player.current;
  if (!t || dragging) return;
  nav("folders", undefined, t.path);
}

// ---------- Seitenleiste ----------
const plList = h("div", { class: "pl-list" });
const status = h("div", { class: "status" });

/** Beim Ziehen kurz über dem Menüpunkt halten → Ansicht wechselt, Ziehen geht weiter */
let springTimer = 0;
/** Sync: Punkt, wenn ein Ziel angeschlossen und bereit ist */
function syncBadge(): string | undefined {
  const n = syncReadyCount();
  return n ? `● ${n}` : undefined;
}

/** Titel auf „+ Neu“ ziehen → neue Playlist anlegen, Titel kommen direkt hinein */
function newDrop(el: HTMLElement) {
  dropTarget(el, {
    accepts: (p) => p.kind === "ids",
    drop: (p) => p.kind === "ids" && newPlaylistFromIds(p.ids),
  });
  return el;
}

function springLoaded(el: HTMLElement, view: ViewName) {
  dropTarget(el, {
    accepts: (p) => p.kind === "ids" || p.kind === "dir",
    enter: () => {
      clearTimeout(springTimer);
      if (current?.name === view) return;
      springTimer = window.setTimeout(() => nav(view), 600);
    },
    leave: () => clearTimeout(springTimer),
    drop: () => {
      clearTimeout(springTimer);
      if (current?.name !== view) nav(view);
      toast(L("Zum Verschieben auf einen Ordner ziehen"));
    },
  });
  return el;
}

function navItem(label: string, active: boolean, onclick: () => void, count?: string) {
  return h("div", { class: "nav-item" + (active ? " active" : ""), onclick }, h("span", {}, label), count ? h("span", { class: "count" }, count) : null);
}

function renderSide() {
  const n = current?.name;
  if (!side.firstChild) {
    side.append(
      h("div", { class: "brand" }, h("div", { class: "bl" }, h("img", { src: logo, alt: "" }), h("span", {}, "Minimal Library")), darkSwitch()),
      h("div", { class: "nav", id: "mainnav" }),
      h("div", { class: "section" }, L("Playlisten")),
      h("div", { class: "section", style: { padding: "0 20px 8px", justifyContent: "flex-start", gap: "14px" } },
        newDrop(h("span", { class: "act", title: L("Neue Playlist (Titel hierher ziehen = direkt hinein)"), onclick: () => newPlaylistFromIds([]) }, L("+ Neu"))),
        h("span", { class: "act", title: L("Smarte Playlist aus Regeln"), onclick: () => smartEditor() }, L("+ Smart")),
        h("span", { class: "act", title: "M3U/M3U8 importieren", onclick: importPlaylist }, "Import …"),
      ),
      plList,
      newDrop(h("div", { class: "pl-new-drop" }, L("+ Neue Playlist mit diesen Titeln"))),
      status,
    );
  }
  const mainnav = side.querySelector("#mainnav")!;
  mainnav.replaceChildren(
    navItem(L("Bibliothek"), n === "library", () => nav("library"), store.tracks.size.toLocaleString(LOCALE)),
    navItem(L("Erweiterte Suche"), n === "search", () => nav("search")),
    springLoaded(navItem(L("Ordner"), n === "folders", () => nav("folders")), "folders"),
    navItem("Sync", n === "sync", () => nav("sync"), syncBadge()),
    navItem("Equalizer", n === "equalizer", () => nav("equalizer")),
    navItem("Ripper", n === "ripper", () => nav("ripper")),
    navItem(L("Sortieren"), n === "organize", () => nav("organize", n === "library" ? [...store.selection] : undefined)),
    navItem(L("Duplikate"), n === "duplicates", () => nav("duplicates")),
    navItem(L("Verlauf"), n === "history", () => nav("history")),
    navItem(L("Einstellungen"), n === "settings", () => nav("settings"), updater.state === "available" ? L("● Update") : undefined),
  );
  plList.replaceChildren(
    ...store.playlists.map((p) => {
      const it = navItem((p.rules ? "◇ " : "") + shown(p.name), n === "playlist" && current?.arg?.[0] === p.id, () => nav("playlist", [p.id]), String(p.count));
      // Titel auf die Playlist ziehen → hinzufügen (smarte Playlisten füllen sich selbst)
      dropTarget(it, {
        accepts: (pl) => pl.kind === "ids" && !p.rules,
        drop: async (pl) => {
          if (pl.kind !== "ids") return;
          await api.playlistAddItems(p.id, pl.ids);
          await store.loadPlaylists();
          toast(L("{0} Titel → {1}", pl.ids.length, p.name));
          if (current?.name === "playlist" && current.arg?.[0] === p.id) nav("playlist", [p.id]);
        },
      });
      it.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        contextMenu(e.clientX, e.clientY, [
          { label: L("Öffnen"), action: () => nav("playlist", [p.id]) },
          { label: L("Exportieren …"), action: () => exportDialog([p.id]) },
          ...(pioneerOn() ? [{ label: L("Für Pioneer-Player exportieren …"), action: () => pioneerExportDialog([p.id]) }, { label: L("rekordbox-XML speichern …"), action: () => xmlDialog([p.id]) }] : []),
          ...(p.rules ? [{ label: L("Regeln bearbeiten …"), action: () => smartEditor(p) }] : []),
        ]);
      });
      return it;
    }),
  );
}

/** Dunkelmodus-Schalter (überschreibt die Windows-Einstellung) */
function darkSwitch() {
  const sun = icon("sun", 11);
  sun.classList.add("sun");
  const moon = icon("moon", 11);
  moon.classList.add("moon");
  // Symbol sitzt im Schalterknopf: Sonne = hell, Mond = dunkel
  const sw = h("div", { class: "switch theme-toggle", title: L("Hell / Dunkel") }, h("span", { class: "track" }, h("span", { class: "knob" }, sun, moon)));
  const upd = () => sw.classList.toggle("on", !isLight());
  sw.addEventListener("click", () => applyTheme(isLight() ? "dark" : "light"));
  document.addEventListener("themechange", upd);
  upd();
  return sw;
}

async function importPlaylist() {
  const f = await open({ filters: [{ name: "Playlist", extensions: ["m3u", "m3u8"] }] });
  if (!f) return;
  const [id, n] = await api.playlistImport(f as string);
  await store.loadTracks();
  await store.loadPlaylists();
  toast(L("{0} Titel importiert", n));
  nav("playlist", [id]);
}

export function newPlaylistFromIds(ids: number[]) {
  const name = h("input", { type: "text", value: "Neue Playlist", style: { width: "100%" } });
  const go = async () => {
    const id = await api.playlistCreate(name.value.trim() || L("Neue Playlist"), null, ids.length ? ids : null);
    await store.loadPlaylists();
    s.close();
    void id;
    toast(ids.length ? L("{0} Titel → {1}", ids.length, name.value) : L("Playlist „{0}“ angelegt", name.value));
  };
  name.addEventListener("keydown", (e) => e.key === "Enter" && go());
  const s = sheet(ids.length ? L("Neue Playlist mit {0} Titel{1}", ids.length, pl(ids.length, "n")) : L("Neue Playlist"),
    h("div", {}, name, h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")), h("button", { class: "btn primary", onclick: go }, L("Anlegen")))));
  setTimeout(() => name.select());
}

// ---------- Hintergrund-Aufgaben + Fortschritt ----------
const TASKS: Record<string, string> = { sync: "Sync", rip: "Ripper", tools: L("Werkzeuge laden"), scan: L("Scannen"), analyze: L("Analysieren"), duplicates: L("Duplikate"), organize: L("Sortieren"), export: L("Exportieren"), pioneer: L("Pioneer-Export"), lyrics: L("Songtexte holen") };
let busy = 0;
let hideTimer = 0;
function setStatus(task: string, done: number, total: number, msg: string) {
  clearTimeout(hideTimer);
  const pct = total ? Math.round((done / total) * 100) : 0;
  const cancellable = task === "analyze" || task === "export" || task === "rip" || task === "sync" || task === "pioneer" || task === "lyrics";
  status.replaceChildren(
    h("div", { class: "srow" }, h("span", {}, `${TASKS[task] ?? task}${total ? ` ${done.toLocaleString(LOCALE)} / ${total.toLocaleString(LOCALE)}` : ""}`),
      cancellable && done < total ? h("span", { class: "act", style: { cursor: "pointer" }, onclick: () => api.cancel() }, L("Abbrechen")) : null),
    h("div", { class: "bar" }, h("i", { style: { width: (total ? pct : 100) + "%", opacity: total ? "1" : "0.4" } })),
    h("div", { class: "msg" }, trMsg(msg)),
  );
  if ((total && done >= total) || msg === "Fertig") hideTimer = window.setTimeout(() => status.replaceChildren(), 4000);
}

/** explicit = vom Benutzer angestoßen: auch Titel aus Cloud-Ordnern („nur bei Bedarf“) */
export async function analyzeIds(ids: number[] | null, explicit = false) {
  busy++;
  try {
    const n = await api.analyze(ids, explicit);
    if (n) toast(L("{0} Titel analysiert", n));
  } catch (e) {
    toast(String(e));
  } finally {
    busy--;
  }
}

// Musikordner entfernt: Analyse wurde gestoppt → für die übrigen Titel neu anstoßen
store.on("pruned", () => {
  toast(L("Musikordner entfernt – seine Titel sind nicht mehr in der Bibliothek."));
  setTimeout(() => {
    if (store.settings.autoAnalyze && [...store.tracks.values()].some((t) => !t.analyzed || (t.cutoff == null && !t.analysisError)) && !busy) analyzeIds(null);
  }, 1200);
});

export async function scanNow() {
  if (!store.settings.roots.length) return;
  try {
    const r = await api.scan();
    await store.loadTracks();
    await store.loadPlaylists();
    if (r.added || r.removed || r.updated) toast(L("Scan: {0} neu, {1} geändert, {2} entfernt", r.added, r.updated, r.removed));
    if (r.errors.length) toast(L("{0} Dateien konnten nicht gelesen werden", r.errors.length));
    if (store.settings.autoAnalyze && [...store.tracks.values()].some((t) => !t.analyzed || (t.cutoff == null && !t.analysisError)) && !busy) analyzeIds(null);
  } catch (e) {
    toast(String(e));
  }
}

// Analyse-Ergebnisse gebündelt nachladen
let analyzedIds: number[] = [];
let analyzedTimer = 0;
listen<number>("analyzed", (e) => {
  analyzedIds.push(e.payload);
  if (!analyzedTimer)
    analyzedTimer = window.setTimeout(async () => {
      const ids = analyzedIds;
      analyzedIds = [];
      analyzedTimer = 0;
      await store.refreshTracks(ids);
      store.emit("pioneer");
    }, 700);
});
listen<{ task: string; done: number; total: number; message: string }>("progress", (e) => setStatus(e.payload.task, e.payload.done, e.payload.total, e.payload.message));
store.on("tracks", () => store.emit("history"));

// Infobereich (Tray): Steuerung + Titel im Tooltip
listen<string>("tray", (e) => {
  if (audioOwner() !== MY_TAB) return; // nur der Tab, in dem die Musik läuft
  if (e.payload === "toggle") player.toggle();
  else if (e.payload === "next") player.next();
});
let trayText = "";
store.on("player", () => {
  const t = player.current;
  if (audioOwner() !== MY_TAB) return;
  reportPlaying(player.playing);
  const text = t ? `${t.artist ? t.artist + " – " : ""}${store.label(t)}${player.playing ? "" : " (Pause)"}` : "";
  if (text !== trayText) {
    trayText = text;
    api.setTrayTitle(text).catch(() => {});
  }
});

// Musikordner-Überwachung: Änderungen aus dem Explorer sofort übernehmen
// Änderungen sammeln: bei Cloud-Ordnern kommen sie in schneller Folge – nur einmal neu laden
let pendingChange: LibraryChange | null = null;
let changeTimer = 0;
// Playlisten am Handy geändert (Minimal Player Connected)
listen("playlists-changed", () => store.loadPlaylists());
// Neue Musikquelle vom Handy angelegt
listen("settings-changed", async () => { await store.loadSettings(); store.emit("settings"); });

listen<LibraryChange>("library-changed", (e) => {
  const c = e.payload;
  if (pendingChange) {
    pendingChange.added.push(...c.added);
    pendingChange.moved += c.moved;
    pendingChange.removed += c.removed;
    pendingChange.updated += c.updated;
  } else pendingChange = { ...c, added: [...c.added] };
  clearTimeout(changeTimer);
  changeTimer = window.setTimeout(() => {
    const all = pendingChange!;
    pendingChange = null;
    applyLibraryChange(all);
  }, 1200);
});

async function applyLibraryChange(c: LibraryChange) {
  await store.loadTracks();
  store.emit("dirs");
  if (c.moved || c.removed || c.added.length) store.loadPlaylists();
  const parts = [
    c.added.length ? L("{0} neu", c.added.length) : "",
    c.moved ? L("{0} verschoben", c.moved) : "",
    c.removed ? L("{0} entfernt", c.removed) : "",
  ].filter(Boolean);
  if (parts.length) toast(L("Musikordner: {0}", parts.join(", ")));
  if (c.added.length && store.settings.autoAnalyze) analyzeIds(c.added);
}

// Dateien/Ordner ins Fenster ziehen → zur Bibliothek
getCurrentWebview().onDragDropEvent(async (e) => {
  if (e.payload.type !== "drop" || !e.payload.paths.length) return;
  const r = await api.addPaths(e.payload.paths);
  if (r.newRoots.length) store.settings = await api.getSettings();
  await store.loadTracks();
  toast(L("{0} Titel hinzugefügt{1}{2}", r.ids.length, r.newRoots.length ? L(" · neuer Musikordner: {0}", r.newRoots.join(", ")) : "", r.skipped ? L(" · {0} Datei{1} außerhalb der Musikordner nicht aufgenommen", r.skipped, pl(r.skipped, "en")) : ""), 7000);
  if (r.ids.length) analyzeIds(r.ids);
});

// ---------- Player-Leiste ----------
function buildPlayer() {
  const cover = h("div", { class: "pcover" });
  const t1 = h("div", { class: "t1" }, "–");
  const t2 = h("div", { class: "t2" }, "");
  const playBtn = h("button", { class: "ibtn play", title: L("Abspielen / Pause (Leertaste)"), onclick: () => player.toggle() }, icon("play"));
  const { wrap, canvas } = waveElement({ duration: () => player.duration, onSeek: (f) => player.current && player.seekFrac(f), cls: "big" });
  wrap.style.height = "56px";
  canvas.style.height = "56px";
  canvas.style.width = "100%";
  const tCur = h("span", {}, "0:00");
  const tDur = h("span", {}, "0:00");
  const vol = h("input", { type: "range", min: "0", max: "1", step: "0.01", value: String(player.volumeLevel) });
  vol.addEventListener("input", () => player.setVolume(Number(vol.value)));
  const shuffleBtn = h("button", { class: "ibtn", title: L("Zufallswiedergabe"), onclick: () => player.setShuffle(!player.shuffle) }, icon("shuffle"));
  const repeatBtn = h("button", { class: "ibtn", onclick: () => player.cycleRepeat() }, icon("repeat"));
  const devBtn = h("button", { class: "btn dev", title: L("Ausgabegerät: dieser PC oder Lautsprecher im Heimnetz") }, L("Dieser PC"));
  devBtn.addEventListener("click", () => outputPopup(devBtn));
  // Lautstärke: größere Fläche, Mausrad = ±5 %
  const volBox = h("div", { class: "vol-box", title: L("Lautstärke (Mausrad)") }, vol);
  volBox.addEventListener("wheel", (e) => {
    e.preventDefault();
    const v = Math.max(0, Math.min(1, player.volumeLevel + (e.deltaY < 0 ? 0.05 : -0.05)));
    player.setVolume(Math.round(v * 100) / 100);
    vol.value = String(player.volumeLevel);
  }, { passive: false });
  const nowInfo = h("div", { style: { minWidth: "0" } }, t1, t2);
  const lyrBtn = h("button", { class: "ibtn", title: L("Songtext (mitlaufend)"), onclick: () => toggleLyricsPanel() }, icon("lyrics"));
  const starBtn = h("button", { class: "ibtn star", title: L("Stern (Playlist „Stern“)"), onclick: () => player.current && toggleStar([player.current.id]) }, icon("star"));
  // Rechtsklick auf den laufenden Titel: gleiches Menü wie in der Liste
  for (const el of [cover, nowInfo]) {
    // Laufenden Titel ziehen: auf Ordner (verschieben) oder Playlist (hinzufügen)
    el.addEventListener("pointerdown", (e) => {
      if (!player.current || (e.target as HTMLElement).closest(".artist-link")) return;
      e.preventDefault();
      const id = player.current.id;
      beginDrag(e, () => ({ kind: "ids", ids: [id] }));
    });
    el.style.cursor = "grab";
    el.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      const t = player.current;
      if (t) contextMenu(e.clientX, e.clientY - 40, trackMenu([t.id]));
    });
  }
  // Klick auf Cover oder Titel: zum Speicherort des laufenden Titels
  for (const el of [cover, t1]) {
    el.addEventListener("click", showPlaying);
    el.title = L("Klicken: Speicherort zeigen");
  }
  playerBar.append(
    cover,
    h("div", { style: { display: "flex", alignItems: "center", gap: "6px", minWidth: "0" } }, nowInfo, starBtn),
    h("div", { class: "ctrls" },
      shuffleBtn,
      h("button", { class: "ibtn", title: L("Zurück"), onclick: () => player.prev() }, icon("prev")),
      playBtn,
      h("button", { class: "ibtn", title: L("Weiter"), onclick: () => player.next() }, icon("next")),
      repeatBtn,
      lyrBtn,
    ),
    h("div", { class: "bigwave" }, wrap, h("div", { class: "time" }, tCur, tDur)),
    h("div", { class: "vol" }, devBtn, volBox),
  );
  let coverFor = -1;
  const update = () => {
    const t = player.current;
    playBtn.replaceChildren(icon(player.playing ? "pause" : "play"));
    repeatBtn.replaceChildren(icon(player.repeat === "one" ? "repeat1" : "repeat"));
    repeatBtn.classList.toggle("on", player.repeat !== "off");
    repeatBtn.title = player.repeat === "off" ? L("Wiederholen: aus") : player.repeat === "all" ? L("Wiederholen: alle") : L("Wiederholen: dieser Titel");
    t1.textContent = t ? store.label(t) : "–";
    if (t) t2.replaceChildren(artistLinks(t.artist, L("Unbekannter Künstler")), t.album ? " · " : "", albumLink(t.album));
    else t2.textContent = L("Doppelklick oder Klick in eine Wellenform");
    tDur.textContent = fmtTime(player.duration);
    if (t && coverFor !== t.id) {
      coverFor = t.id;
      cover.replaceChildren();
      if (t.hasCover) api.getCover(t.id).then((u) => u && coverFor === t.id && cover.replaceChildren(h("img", { src: u })));
    }
    drawWave(canvas, t ? store.peaksFor(t.id) : undefined, t ? player.frac() : -1);
    tCur.textContent = fmtTime(player.currentTime);
    shuffleBtn.classList.toggle("on", player.shuffle);
    lyrBtn.classList.toggle("on", lyricsPanelOpen());
    const st = !!t && isStarred(t.id);
    starBtn.replaceChildren(icon(st ? "starOn" : "star"));
    starBtn.classList.toggle("on", st);
    starBtn.style.visibility = t ? "visible" : "hidden";
    devBtn.textContent = (player.device ? "▸ " : "") + currentOutputLabel();
    devBtn.classList.toggle("on", !!player.device);
    if (document.activeElement !== vol) vol.value = String(player.volumeLevel);
  };
  store.on("player", update);
  store.on("peaks", update);
  store.on("stars", update);
  document.addEventListener("themechange", update);
  new ResizeObserver(update).observe(canvas);
  player.onPos((id, f) => {
    drawWave(canvas, store.peaksFor(id), f);
    tCur.textContent = fmtTime(player.currentTime);
    tDur.textContent = fmtTime(player.duration);
  });
  update();
}

// Globale Tasten
document.addEventListener("keydown", (e) => {
  const tag = (e.target as HTMLElement).tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || document.querySelector(".sheet-bg")) return;
  if (e.key === "z" && (e.ctrlKey || e.metaKey) && !e.shiftKey) {
    e.preventDefault();
    undo(null);
    return;
  }
  if (e.code === "Space") {
    e.preventDefault();
    player.toggle();
  } else if (e.key === "ArrowRight" && !e.shiftKey) {
    player.seekBy(5);
  } else if (e.key === "ArrowLeft" && !e.shiftKey) {
    player.seekBy(-5);
  }
});
// Rechtsklick-Menü des Browsers aus
document.addEventListener("contextmenu", (e) => {
  if (!(e.target as HTMLElement).closest("input, textarea")) e.preventDefault();
});

store.on("playlists", renderSide);
store.on("sync", renderSide);
store.on("update", renderSide);
store.on("tracks", renderSide);

async function start() {
  buildPlayer();
  await store.loadSettings();
  await store.loadTracks();
  await store.loadPlaylists();
  if (!store.settings.roots.length && store.tracks.size === 0) {
    nav("settings");
    toast(L("Willkommen! Füge zuerst deinen Musikordner hinzu."), 6000);
  } else {
    nav("library");
    scanNow();
  }
  // Alter Playlist-Ordner (nur .m3u8) ist durch Sync ersetzt
  if (store.settings.playlistDir) store.saveSettings({ playlistDir: null });
  startSyncWatch();
  if (MY_TAB === "main") startUpdater();
  restoreOutput();
}
start();

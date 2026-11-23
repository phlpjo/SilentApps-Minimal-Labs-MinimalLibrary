import { lyricsPanelOpen, toggleLyricsPanel } from "./lyrics";
import { isStarred, toggleStar } from "./star";
import "./style.css";
import { IS_MAC } from "./platform";
import { applyTheme, isLight } from "./theme";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type LibraryChange, type Playlist } from "./api";
import { Inspector } from "./inspector";
import { player } from "./player";
import { store } from "./store";
import { albumLink, artistLinks, onAlbumClick, clear, contextMenu, fmtTime, h, onArtistClick, sheet, toast } from "./ui";
import { drawWave, waveElement } from "./waveform";
import { beginDrag, dragging, dropTarget, externalDrag, type Payload } from "./dnd";
import { duplicatesView } from "./views/duplicates";
import { libraryView, trackMenu } from "./views/library";
import { foldersView } from "./views/folders";
import { historyView, undo } from "./views/history";
import { ripperView } from "./views/ripper";
import { syncView, startSyncWatch, syncReadyCount } from "./views/sync";
import { equalizerView } from "./views/equalizer";
import { searchView } from "./views/search";
import { startUpdater, updater } from "./updater";
import { loadLicense, requireLicense, unlocked, type Feature } from "./license";
import { icon, navIcon } from "./icons";
import { MY_TAB, audioOwner, reportPlaying, tabStrip } from "./tabs";
import { pioneerExportDialog, pioneerOn, xmlDialog } from "./pioneer";
import { currentOutputLabel, outputPopup, restoreOutput } from "./outputs";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { organizeView } from "./views/organize";
import { exportDialog, playlistView, renamePlaylist, smartEditor } from "./views/playlist";
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
const sideBtn = h("button", { class: "tb-btn tb-icon", title: L("Seitenleiste ein-/ausklappen"), onclick: () => setSideClosed(!app.classList.contains("side-closed")) }, icon("panelLeft", 17));
const inspBtn = h("button", { class: "tb-btn tb-icon", title: L("Infoleiste ein-/ausklappen"), onclick: () => setInspClosed(!app.classList.contains("insp-closed")) }, icon("panelRight", 17));
const titlebar = h("header", { id: "titlebar", "data-tauri-drag-region": "" },
  sideBtn,
  backBtn,
  tabStrip(),
  h("div", { class: "tb-title", "data-tauri-drag-region": "" }),
  // Hell/Dunkel ganz rechts in der Titelleiste (Windows: vor den Fenstersymbolen)
  inspBtn,
  h("div", { class: "tb-theme" }, darkSwitch()),
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
  const gate = GATED[name];
  if (gate && !requireLicense(gate)) return;
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

function navItem(ico: string, label: string, active: boolean, onclick: (e: MouseEvent) => void, count?: string, locked = false) {
  return h("div", { class: "nav-item" + (active ? " active" : "") + (locked ? " locked" : ""), title: locked ? label + " · " + L("Vollversion") : label, onclick },
    h("span", { class: "ni" }, navIcon(ico), h("span", { class: "nl" }, label)),
    locked ? h("span", { class: "count lock" }, icon("lock", 12)) : count ? h("span", { class: "count" }, count) : null);
}

/** Ansichten der Vollversion (nach der Testphase gesperrt) */
const GATED: Partial<Record<ViewName, Feature>> = { search: "search", sync: "sync", ripper: "ripper", organize: "organize" };
const isLocked = (v: ViewName) => !!GATED[v] && !unlocked();

// ---------- Seitenleisten ein-/ausklappen ----------
const SIDE_KEY = "ml.sideClosed";
const INSP_KEY = "ml.inspClosed";
function setSideClosed(closed: boolean) {
  app.classList.toggle("side-closed", closed);
  setFly(false);
  try { localStorage.setItem(SIDE_KEY, closed ? "1" : ""); } catch {}
}
function setInspClosed(closed: boolean) {
  app.classList.toggle("insp-closed", closed);
  try { localStorage.setItem(INSP_KEY, closed ? "1" : ""); } catch {}
}
try {
  if (localStorage.getItem(SIDE_KEY)) app.classList.add("side-closed");
  if (localStorage.getItem(INSP_KEY)) app.classList.add("insp-closed");
} catch {}
/** Eingeklappt: Playlisten als Ausklapp-Menü neben dem Symbol (Klick oder beim Ziehen kurz darüber halten) */
let flyTimer = 0;
let flyByDrag = false;
function setFly(open: boolean, anchor?: HTMLElement) {
  // Platz nach unten zu knapp → Menü weiter oben öffnen (bis ca. 440 px hoch)
  if (open && anchor) app.style.setProperty("--fly-top", Math.max(44, Math.min(anchor.getBoundingClientRect().top - 8, window.innerHeight - 110 - 440)) + "px");
  app.classList.toggle("pl-fly", open);
  if (!open) flyByDrag = false;
}
document.addEventListener("pointerdown", (e) => {
  if (!app.classList.contains("pl-fly")) return;
  const t = e.target as HTMLElement;
  if (!t.closest(".pl-box") && !t.closest(".pl-fold") && !t.closest(".menu") && !t.closest(".sheet-bg")) setFly(false);
}, true);
new MutationObserver(() => {
  if (!document.body.classList.contains("is-dragging")) {
    clearTimeout(flyTimer);
    if (flyByDrag) setTimeout(() => setFly(false), 250);
  }
}).observe(document.body, { attributes: true, attributeFilter: ["class"] });

// Mehrfachauswahl in der Playlist-Liste (Strg/⌘ bzw. Umschalt + Klick)
const plSel = new Set<number>();
let plAnchor = -1;

async function deletePlaylists(ids: number[]) {
  for (const id of ids) await api.playlistDelete(id).catch((e) => toast(String(e)));
  plSel.clear();
  await store.loadPlaylists();
  if (current?.name === "playlist" && ids.includes(current.arg?.[0] ?? -1)) nav("library");
  else renderSide();
}

function playlistMenu(e: MouseEvent, p: Playlist) {
  e.preventDefault();
  const multi = plSel.has(p.id) && plSel.size > 1;
  if (!multi) { plSel.clear(); plSel.add(p.id); plAnchor = store.playlists.findIndex((x) => x.id === p.id); renderSide(); }
  const ids = multi ? store.playlists.filter((x) => plSel.has(x.id)).map((x) => x.id) : [p.id];
  const pio = pioneerOn() ? [{ label: L("Für Pioneer-Player exportieren …"), action: () => pioneerExportDialog(ids) }, { label: L("rekordbox-XML speichern …"), action: () => xmlDialog(ids) }] : [];
  contextMenu(e.clientX, e.clientY, multi
    ? [
        { label: L("{0} Playlisten exportieren …", ids.length), action: () => exportDialog(ids) },
        ...pio,
        { label: "", sep: true },
        { label: L("{0} Playlisten löschen", ids.length), hold: true, action: () => deletePlaylists(ids) },
      ]
    : [
        { label: L("Öffnen"), action: () => nav("playlist", [p.id]) },
        p.rules ? { label: L("Regeln bearbeiten …"), action: () => smartEditor(p) } : { label: L("Umbenennen …"), action: () => renamePlaylist(p) },
        { label: L("Exportieren …"), action: () => exportDialog(ids) },
        ...pio,
        { label: "", sep: true },
        { label: L("Löschen"), hold: true, action: () => deletePlaylists(ids) },
      ]);
}

/** Logo-Zeichen ohne Kreis als „M“ im Schriftzug */
function logoM() {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "45.5 44 18 20");
  svg.setAttribute("class", "logo-m");
  svg.setAttribute("aria-label", "M");
  svg.innerHTML = '<g fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round"><path d="M47.5,46 V62"/><path d="M53,46 V62"/><path d="M57.5,47.5 L61.5,61.5"/></g>';
  return svg;
}

function plFold() {
  const box = h("div", { class: "nav pl-fold" });
  const it = navItem("playlists", L("Playlisten"), false, () => setFly(!app.classList.contains("pl-fly"), it));
  box.append(it);
  dropTarget(it, {
    accepts: (p) => p.kind === "ids" && app.classList.contains("side-closed"),
    enter: () => {
      clearTimeout(flyTimer);
      flyTimer = window.setTimeout(() => { flyByDrag = true; setFly(true, it); }, 350);
    },
    leave: () => clearTimeout(flyTimer),
    drop: () => {},
  });
  return box;
}

/** Playlist an neue Stelle ziehen (eigene Reihenfolge) */
async function movePlaylist(id: number, target: number, after: boolean) {
  const ids = store.playlists.map((p) => p.id).filter((x) => x !== id);
  const at = ids.indexOf(target);
  if (at < 0) return;
  ids.splice(after ? at + 1 : at, 0, id);
  await api.playlistOrder(ids);
  await store.loadPlaylists();
}

function renderSide() {
  const n = current?.name;
  if (!side.firstChild) {
    side.append(
      // Logo wie gehabt; im Schriftzug ersetzt das Logo-Zeichen (ohne Kreis) das „M“
      h("div", { class: "brand", title: "Minimal Library" }, h("div", { class: "bl" }, h("img", { src: logo, alt: "" }), h("span", { class: "wordmark" }, logoM(), "inimal Library"))),
      h("div", { class: "nav", id: "mainnav" }),
      // eingeklappt: ein Punkt für alle Playlisten – öffnet sie als Ausklapp-Menü
      plFold(),
      h("div", { class: "pl-box" },
        h("div", { class: "section pl-sec" }, L("Playlisten")),
        h("div", { class: "section pl-sec", style: { padding: "0 20px 8px", justifyContent: "flex-start", gap: "14px" } },
          newDrop(h("span", { class: "act", title: L("Neue Playlist (Titel hierher ziehen = direkt hinein)"), onclick: () => newPlaylistFromIds([]) }, L("+ Neu"))),
          h("span", { class: "act", title: L("Smarte Playlist aus Regeln"), onclick: () => smartEditor() }, L("+ Smart")),
          h("span", { class: "act", title: L("M3U/M3U8 importieren"), onclick: importPlaylist }, L("+ Import")),
        ),
        plList,
        newDrop(h("div", { class: "pl-new-drop" }, L("+ Neue Playlist mit diesen Titeln"))),
      ),
      status,
    );
  }
  const mainnav = side.querySelector("#mainnav")!;
  mainnav.replaceChildren(
    navItem("library", L("Bibliothek"), n === "library", () => nav("library"), store.tracks.size.toLocaleString(LOCALE)),
    navItem("search", L("Erweiterte Suche"), n === "search", () => nav("search"), undefined, isLocked("search")),
    springLoaded(navItem("folders", L("Ordner"), n === "folders", () => nav("folders")), "folders"),
    navItem("sync", "Sync", n === "sync", () => nav("sync"), syncBadge(), isLocked("sync")),
    navItem("equalizer", "Equalizer", n === "equalizer", () => nav("equalizer")),
    navItem("ripper", "Ripper", n === "ripper", () => nav("ripper"), undefined, isLocked("ripper")),
    navItem("organize", L("Sortieren"), n === "organize", () => nav("organize", n === "library" ? [...store.selection] : undefined), undefined, isLocked("organize")),
    navItem("duplicates", L("Duplikate"), n === "duplicates", () => nav("duplicates")),
    navItem("history", L("Verlauf"), n === "history", () => nav("history")),
    navItem("settings", L("Einstellungen"), n === "settings", () => nav("settings"), updater.state === "available" ? L("● Update") : undefined),
  );
  for (const id of [...plSel]) if (!store.playlists.some((p) => p.id === id)) plSel.delete(id);
  plList.replaceChildren(
    ...store.playlists.map((p, i) => {
      const star = p.name === "Stern" && !p.rules;
      const it = navItem(p.rules ? "smart" : star ? "star" : "playlist", shown(p.name), n === "playlist" && current?.arg?.[0] === p.id, (e) => {
        if (dragging) return;
        if (e.ctrlKey || e.metaKey) {
          if (plSel.has(p.id)) plSel.delete(p.id); else plSel.add(p.id);
          plAnchor = i;
          renderSide();
          return;
        }
        if (e.shiftKey && plAnchor >= 0) {
          const [a, b] = [Math.min(plAnchor, i), Math.max(plAnchor, i)];
          plSel.clear();
          store.playlists.slice(a, b + 1).forEach((x) => plSel.add(x.id));
          renderSide();
          return;
        }
        plSel.clear();
        plSel.add(p.id);
        plAnchor = i;
        if (app.classList.contains("pl-fly")) setFly(false);
        nav("playlist", [p.id]);
      }, String(p.count));
      // Ziehen: Playlist umsortieren
      it.addEventListener("pointerdown", (e) => beginDrag(e, () => ({ kind: "pl", id: p.id, name: shown(p.name) })));
      if (plSel.size > 1 && plSel.has(p.id)) it.classList.add("multi");
      // Titel auf die Playlist ziehen → hinzufügen (smarte Playlisten füllen sich selbst)
      let after = false;
      dropTarget(it, {
        accepts: (pl) => (pl.kind === "ids" && !p.rules) || (pl.kind === "pl" && pl.id !== p.id),
        over: (pl, e) => {
          if (pl.kind !== "pl") return;
          const r = it.getBoundingClientRect();
          after = e.clientY > r.top + r.height / 2;
          it.classList.toggle("ins-before", !after);
          it.classList.toggle("ins-after", after);
        },
        leave: () => it.classList.remove("ins-before", "ins-after"),
        drop: async (pl) => {
          if (pl.kind === "pl") return movePlaylist(pl.id, p.id, after);
          if (pl.kind !== "ids") return;
          await api.playlistAddItems(p.id, pl.ids);
          await store.loadPlaylists();
          toast(L("{0} Titel → {1}", pl.ids.length, shown(p.name)));
          if (current?.name === "playlist" && current.arg?.[0] === p.id) nav("playlist", [p.id]);
        },
      });
      it.addEventListener("contextmenu", (e) => playlistMenu(e, p));
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
  const name = h("input", { type: "text", value: L("Neue Playlist"), style: { width: "100%" } });
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

// Titel wurden aus einem anderen Tab hierher gezogen: Ordneransicht öffnen, dort auf einen Ordner ablegen
listen<Payload & { to?: string }>("tab-drag", (e) => {
  // Ereignisse gehen an alle Fenster – nur der Ziel-Tab übernimmt
  if (e.payload.kind !== "tab" || e.payload.to !== MY_TAB) return;
  nav("folders");
  setTimeout(() => externalDrag(e.payload), 150);
});

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
  await loadLicense();
  store.on("license", () => renderSide());
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

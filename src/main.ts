import "./style.css";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { api } from "./api";
import { Inspector } from "./inspector";
import { player } from "./player";
import { store } from "./store";
import { clear, contextMenu, fmtTime, h, sheet, toast } from "./ui";
import { drawWave, waveElement } from "./waveform";
import { duplicatesView } from "./views/duplicates";
import { libraryView } from "./views/library";
import { organizeView } from "./views/organize";
import { exportDialog, playlistView, smartEditor } from "./views/playlist";
import { settingsView } from "./views/settings";
import logo from "./logo.svg";

type ViewName = "library" | "playlist" | "duplicates" | "organize" | "settings";

const app = document.getElementById("app")!;
const side = h("nav", { id: "side" });
const main = h("main", { id: "main" });
const insp = h("aside", { id: "inspector" });
const playerBar = h("footer", { id: "player" });
app.append(side, main, insp, playerBar);
new Inspector(insp);

let current: { name: ViewName; arg?: number[]; destroy: () => void } | null = null;

export function nav(name: ViewName, arg?: number[]) {
  current?.destroy();
  clear(main);
  let v: { el: HTMLElement; destroy: () => void };
  if (name !== "playlist" && name !== "library") store.selection.clear();
  switch (name) {
    case "library": v = libraryView(); break;
    case "playlist": store.selection.clear(); v = playlistView(arg![0]); break;
    case "duplicates": v = duplicatesView(); break;
    case "organize": v = organizeView(arg); break;
    case "settings": v = settingsView(); break;
  }
  main.appendChild(v.el);
  current = { name, arg, destroy: v.destroy };
  app.classList.toggle("no-insp", !(name === "library" || name === "playlist"));
  store.emit("selection");
  renderSide();
}

// ---------- Seitenleiste ----------
const plList = h("div", { class: "pl-list" });
const status = h("div", { class: "status" });

function navItem(label: string, active: boolean, onclick: () => void, count?: string) {
  return h("div", { class: "nav-item" + (active ? " active" : ""), onclick }, h("span", {}, label), count ? h("span", { class: "count" }, count) : null);
}

function renderSide() {
  const n = current?.name;
  if (!side.firstChild) {
    side.append(
      h("div", { class: "brand" }, h("img", { src: logo, alt: "" }), h("span", {}, "Minimal Library")),
      h("div", { class: "nav", id: "mainnav" }),
      h("div", { class: "section" }, "Playlisten"),
      h("div", { class: "section", style: { padding: "0 20px 8px", justifyContent: "flex-start", gap: "14px" } },
        h("span", { class: "act", title: "Neue Playlist", onclick: () => newPlaylistFromIds([]) }, "+ Neu"),
        h("span", { class: "act", title: "Smarte Playlist aus Regeln", onclick: () => smartEditor() }, "+ Smart"),
        h("span", { class: "act", title: "M3U/M3U8 importieren", onclick: importPlaylist }, "Import …"),
      ),
      plList,
      status,
    );
  }
  const mainnav = side.querySelector("#mainnav")!;
  mainnav.replaceChildren(
    navItem("Bibliothek", n === "library", () => nav("library"), store.tracks.size.toLocaleString("de")),
    navItem("Duplikate", n === "duplicates", () => nav("duplicates")),
    navItem("Sortieren", n === "organize", () => nav("organize", n === "library" ? [...store.selection] : undefined)),
    navItem("Einstellungen", n === "settings", () => nav("settings")),
  );
  plList.replaceChildren(
    ...store.playlists.map((p) => {
      const it = navItem((p.rules ? "◇ " : "") + p.name, n === "playlist" && current?.arg?.[0] === p.id, () => nav("playlist", [p.id]), String(p.count));
      if (!p.rules) {
        it.addEventListener("dragover", (e) => {
          if (e.dataTransfer?.types.includes("application/x-ml-ids")) {
            e.preventDefault();
            it.classList.add("drop");
          }
        });
        it.addEventListener("dragleave", () => it.classList.remove("drop"));
        it.addEventListener("drop", async (e) => {
          e.preventDefault();
          it.classList.remove("drop");
          const ids: number[] = JSON.parse(e.dataTransfer!.getData("application/x-ml-ids") || "[]");
          await api.playlistAddItems(p.id, ids);
          await store.loadPlaylists();
          toast(`${ids.length} Titel → ${p.name}`);
        });
      }
      it.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        contextMenu(e.clientX, e.clientY, [
          { label: "Öffnen", action: () => nav("playlist", [p.id]) },
          { label: "Exportieren …", action: () => exportDialog([p.id]) },
          ...(p.rules ? [{ label: "Regeln bearbeiten …", action: () => smartEditor(p) }] : []),
        ]);
      });
      return it;
    }),
  );
}

async function importPlaylist() {
  const f = await open({ filters: [{ name: "Playlist", extensions: ["m3u", "m3u8"] }] });
  if (!f) return;
  const [id, n] = await api.playlistImport(f as string);
  await store.loadTracks();
  await store.loadPlaylists();
  toast(`${n} Titel importiert`);
  nav("playlist", [id]);
}

export function newPlaylistFromIds(ids: number[]) {
  const name = h("input", { type: "text", value: "Neue Playlist", style: { width: "100%" } });
  const go = async () => {
    const id = await api.playlistCreate(name.value.trim() || "Neue Playlist", null, ids.length ? ids : null);
    await store.loadPlaylists();
    s.close();
    if (ids.length) toast(`${ids.length} Titel → ${name.value}`);
    else nav("playlist", [id]);
  };
  name.addEventListener("keydown", (e) => e.key === "Enter" && go());
  const s = sheet(ids.length ? `Neue Playlist mit ${ids.length} Titel${ids.length === 1 ? "" : "n"}` : "Neue Playlist",
    h("div", {}, name, h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"), h("button", { class: "btn primary", onclick: go }, "Anlegen"))));
  setTimeout(() => name.select());
}

// ---------- Hintergrund-Aufgaben + Fortschritt ----------
const TASKS: Record<string, string> = { scan: "Scannen", analyze: "Analysieren", duplicates: "Duplikate", organize: "Sortieren", export: "Exportieren" };
let busy = 0;
let hideTimer = 0;
function setStatus(task: string, done: number, total: number, msg: string) {
  clearTimeout(hideTimer);
  const pct = total ? Math.round((done / total) * 100) : 0;
  const cancellable = task === "analyze" || task === "export";
  status.replaceChildren(
    h("div", { class: "srow" }, h("span", {}, `${TASKS[task] ?? task}${total ? ` ${done.toLocaleString("de")} / ${total.toLocaleString("de")}` : ""}`),
      cancellable && done < total ? h("span", { class: "act", style: { cursor: "pointer" }, onclick: () => api.cancel() }, "Abbrechen") : null),
    h("div", { class: "bar" }, h("i", { style: { width: (total ? pct : 100) + "%", opacity: total ? "1" : "0.4" } })),
    h("div", { class: "msg" }, msg),
  );
  if ((total && done >= total) || msg === "Fertig") hideTimer = window.setTimeout(() => status.replaceChildren(), 4000);
}

export async function analyzeIds(ids: number[] | null) {
  busy++;
  try {
    const n = await api.analyze(ids);
    if (n) toast(`${n} Titel analysiert`);
  } catch (e) {
    toast(String(e));
  } finally {
    busy--;
  }
}

export async function scanNow() {
  if (!store.settings.roots.length) return;
  try {
    const r = await api.scan();
    await store.loadTracks();
    await store.loadPlaylists();
    if (r.added || r.removed || r.updated) toast(`Scan: ${r.added} neu, ${r.updated} geändert, ${r.removed} entfernt`);
    if (r.errors.length) toast(`${r.errors.length} Dateien konnten nicht gelesen werden`);
    if (store.settings.autoAnalyze && [...store.tracks.values()].some((t) => !t.analyzed) && !busy) analyzeIds(null);
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
    }, 700);
});
listen<{ task: string; done: number; total: number; message: string }>("progress", (e) => setStatus(e.payload.task, e.payload.done, e.payload.total, e.payload.message));

// Dateien/Ordner ins Fenster ziehen → zur Bibliothek
getCurrentWebview().onDragDropEvent(async (e) => {
  if (e.payload.type !== "drop" || !e.payload.paths.length) return;
  const ids = await api.addPaths(e.payload.paths);
  await store.loadTracks();
  toast(`${ids.length} Titel hinzugefügt`);
  if (ids.length) analyzeIds(ids);
});

// ---------- Player-Leiste ----------
function buildPlayer() {
  const cover = h("div", { class: "pcover" });
  const t1 = h("div", { class: "t1" }, "–");
  const t2 = h("div", { class: "t2" }, "");
  const playBtn = h("button", { class: "btn play", onclick: () => player.toggle() }, "Abspielen");
  const { wrap, canvas } = waveElement({ duration: () => player.duration, onSeek: (f) => player.current && player.seekFrac(f), cls: "big" });
  wrap.style.height = "56px";
  canvas.style.height = "56px";
  canvas.style.width = "100%";
  const tCur = h("span", {}, "0:00");
  const tDur = h("span", {}, "0:00");
  const vol = h("input", { type: "range", min: "0", max: "1", step: "0.01", value: String(player.audio.volume) });
  vol.addEventListener("input", () => player.setVolume(Number(vol.value)));
  playerBar.append(
    cover,
    h("div", { style: { minWidth: "0" } }, t1, t2),
    h("div", { class: "ctrls" },
      h("button", { class: "btn", onclick: () => player.prev() }, "Zurück"),
      playBtn,
      h("button", { class: "btn", onclick: () => player.next() }, "Weiter"),
    ),
    h("div", { class: "bigwave" }, wrap, h("div", { class: "time" }, tCur, tDur)),
    h("div", { class: "vol" }, "Lautst.", vol),
  );
  let coverFor = -1;
  const update = () => {
    const t = player.current;
    playBtn.textContent = player.playing ? "Pause" : "Abspielen";
    t1.textContent = t ? store.label(t) : "–";
    t2.textContent = t ? [t.artist, t.album].filter(Boolean).join(" · ") : "Doppelklick oder Klick in eine Wellenform";
    tDur.textContent = fmtTime(player.duration);
    if (t && coverFor !== t.id) {
      coverFor = t.id;
      cover.replaceChildren();
      if (t.hasCover) api.getCover(t.id).then((u) => u && coverFor === t.id && cover.replaceChildren(h("img", { src: u })));
    }
    drawWave(canvas, t ? store.peaksFor(t.id) : undefined, t ? player.frac() : -1);
    tCur.textContent = fmtTime(player.audio.currentTime);
  };
  store.on("player", update);
  store.on("peaks", update);
  document.addEventListener("themechange", update);
  new ResizeObserver(update).observe(canvas);
  player.onPos((id, f) => {
    drawWave(canvas, store.peaksFor(id), f);
    tCur.textContent = fmtTime(player.audio.currentTime);
    tDur.textContent = fmtTime(player.duration);
  });
  update();
}

// Globale Tasten
document.addEventListener("keydown", (e) => {
  const tag = (e.target as HTMLElement).tagName;
  if (tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT" || document.querySelector(".sheet-bg")) return;
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
store.on("tracks", renderSide);

async function start() {
  buildPlayer();
  await store.loadSettings();
  await store.loadTracks();
  await store.loadPlaylists();
  if (!store.settings.roots.length && store.tracks.size === 0) {
    nav("settings");
    toast("Willkommen! Füge zuerst deinen Musikordner hinzu.", 6000);
  } else {
    nav("library");
    scanNow();
  }
}
start();

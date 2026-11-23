import { IS_WEB } from "./platform";
// Tabs: jede Registerkarte ist eine eigene, komplette Instanz (eigenes Fenster, eigene Bibliothek).
// Jeder Tab ist eine eigene Wiedergabe; mehrere laufen parallel, solange sie verschiedene Ausgänge nutzen.
import { emit, emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { api } from "./api";
import { icon } from "./icons";
import { cancelDrag, dropTarget } from "./dnd";
import { store } from "./store";
import { contextMenu, h, holdButton, sheet, toast } from "./ui";
import { L, shown } from "./i18n";

export const MY_TAB = getCurrentWindow().label;
const OWNER = "ml.audioOwner";
const CLAIM = "ml.audioClaim";

/** Tab, in dem gerade Musik läuft (bzw. zuletzt lief) */
export function audioOwner(): string {
  try {
    return localStorage.getItem(OWNER) || "main";
  } catch {
    return "main";
  }
}

/** Dieser Tab spielt jetzt – Tabs auf demselben Ausgang hören auf */
export function claimAudio(device = "local") {
  try {
    localStorage.setItem(OWNER, MY_TAB);
    localStorage.setItem(CLAIM, `${MY_TAB}:${Date.now()}:${device}`);
  } catch {
    /* Speicher nicht verfügbar – Abstimmung läuft trotzdem über das Ereignis */
  }
  playingNow = true;
  try {
    localStorage.setItem("ml.audioPlaying", "1");
  } catch {
    /* egal */
  }
  emit("audio-claim", { label: MY_TAB, device }).catch(() => {});
  render();
}

/** Läuft gerade Musik (im Besitzer-Tab)? Steuert das Lautsprecher-Symbol */
let playingNow = (() => {
  try {
    // Beim Programmstart (erstes Fenster) läuft noch nichts
    if (MY_TAB === "main" && !sessionStorage.getItem("ml.started")) {
      sessionStorage.setItem("ml.started", "1");
      localStorage.setItem("ml.audioPlaying", "0");
    }
    return localStorage.getItem("ml.audioPlaying") === "1";
  } catch {
    return false;
  }
})();
export function reportPlaying(playing: boolean) {
  if (audioOwner() !== MY_TAB || playing === playingNow) return;
  playingNow = playing;
  try {
    localStorage.setItem("ml.audioPlaying", playing ? "1" : "0");
  } catch {
    /* egal */
  }
  emit("audio-state", { label: MY_TAB, playing }).catch(() => {});
  render();
}

/** Ein anderer Tab hat übernommen → `stop` aufrufen */
/** Name dieses Tabs */
export function myTabName(): string {
  return shown(tabs.find((t) => t.label === MY_TAB)?.name ?? L("Bibliothek"));
}

export function onOtherTabPlays(stop: (device: string) => void) {
  listen<{ label: string; device: string }>("audio-claim", (e) => {
    if (e.payload.label !== MY_TAB) stop(e.payload.device);
    render();
  });
}

const strip = h("div", { class: "tabs" });
let tabs: { label: string; name: string }[] = [];

function rename(label: string, name: string) {
  const inp = h("input", { type: "text", value: name, style: { width: "100%" } }) as HTMLInputElement;
  const go = async () => {
    await api.tabRename(label, inp.value);
    s.close();
  };
  inp.addEventListener("keydown", (e) => e.key === "Enter" && go());
  const s = sheet(L("Tab umbenennen"), h("div", {}, inp,
    h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")), h("button", { class: "btn primary", onclick: go }, L("Umbenennen")))));
  setTimeout(() => inp.select());
}

function close(label: string, name: string) {
  const s = sheet(L("Tab „{0}“ schließen", name), h("div", {},
    h("div", { class: "note" }, L("Die Bibliothek dieses Tabs (Musikordner-Liste, Analysen, Playlisten, Einstellungen) wird gelöscht. Deine Musikdateien bleiben unberührt.")),
    h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")),
      holdButton(L("Tab schließen"), async () => {
        s.close();
        try {
          await api.tabClose(label);
        } catch (e) {
          toast(String(e));
        }
      }, L("halten")))));
}

function render() {
  const owner = audioOwner();
  strip.replaceChildren(
    ...tabs.map((t) => {
      const el = h("div", { class: "tab" + (t.label === MY_TAB ? " on" : ""), title: t.label === MY_TAB ? L("Doppelklick: umbenennen · Rechtsklick: mehr") : L("Zu „{0}“ wechseln", shown(t.name)) },
        t.label === owner && playingNow && tabs.length > 1 ? h("span", { class: "spk", title: L("Hier läuft die Musik") }, icon("speaker", 13)) : "",
        h("span", { class: "tname" }, shown(t.name)),
      );
      // Titel auf einen anderen Tab ziehen → Tab wechselt, dort auf einen Ordner ablegen (verschieben/kopieren)
      if (t.label !== MY_TAB) {
        let timer = 0;
        dropTarget(el, {
          accepts: (p) => p.kind === "ids",
          enter: (p) => {
            clearTimeout(timer);
            if (p.kind === "ids") timer = window.setTimeout(() => dragToTab(t, p.ids), 450);
          },
          leave: () => clearTimeout(timer),
          drop: (p) => {
            clearTimeout(timer);
            if (p.kind === "ids") dragToTab(t, p.ids);
          },
        });
      }
      el.addEventListener("click", () => t.label !== MY_TAB && api.tabShow(t.label).catch((e) => toast(String(e))));
      el.addEventListener("dblclick", () => rename(t.label, t.name));
      el.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        contextMenu(e.clientX, e.clientY, [
          { label: L("Umbenennen …"), action: () => rename(t.label, t.name) },
          ...(t.label !== "main" ? [{ label: L("Tab schließen …"), action: () => close(t.label, t.name) }] : []),
        ]);
      });
      return el;
    }),
    IS_WEB ? "" : h("button", { class: "tab-add", title: L("Neuer Tab – eine zweite, leere Bibliothek"), onclick: () => api.tabCreate().catch((e) => toast(String(e))) }, "+"),
  );
}

/** Gezogene Titel an einen anderen Tab übergeben und dorthin wechseln */
function dragToTab(t: { label: string; name: string }, ids: number[]) {
  const names: Record<number, string> = {};
  for (const id of ids) {
    const x = store.tracks.get(id);
    if (x) names[id] = `${x.artist ?? "?"} – ${store.label(x)}`;
  }
  const me = tabs.find((x) => x.label === MY_TAB);
  cancelDrag();
  emitTo(t.label, "tab-drag", { kind: "tab", to: t.label, from: MY_TAB, fromName: shown(me?.name ?? ""), ids, names })
    .then(() => api.tabShow(t.label))
    .catch((e) => toast(String(e)));
}

async function load() {
  tabs = await api.tabsList().catch(() => [{ label: "main", name: L("Bibliothek") }]);
  render();
}

/** Tab-Leiste für die Titelleiste */
export function tabStrip(): HTMLElement {
  load();
  listen("tabs-changed", load);
  listen<{ label: string; playing: boolean }>("audio-state", (e) => {
    playingNow = e.payload.playing;
    render();
  });
  return strip;
}

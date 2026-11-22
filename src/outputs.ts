// Ausgabegeräte: Soundkarten dieses PCs + Lautsprecher im Heimnetz (Sonos/DLNA).
// Bekannte Netzwerkgeräte werden gemerkt und sofort angezeigt; die Suche läuft im Hintergrund.

import { api, type CastDevice } from "./api";
import { player } from "./player";
import { store } from "./store";
import { h, toast } from "./ui";

export interface LocalOut { id: string; label: string }

let cache: CastDevice[] = JSON.parse(localStorage.getItem("ml.castCache") ?? "[]");
const found = new Set<string>();
let searching: Promise<CastDevice[]> | null = null;
let lastSearch = 0;

export function cachedDevices() {
  return cache;
}
export function isFound(id: string) {
  return found.has(id);
}
export function isSearching() {
  return !!searching;
}

/** Heimnetz durchsuchen (läuft nur einmal gleichzeitig) */
export function discover(): Promise<CastDevice[]> {
  if (searching) return searching;
  lastSearch = Date.now();
  store.emit("outputs");
  searching = api
    .castDevices()
    .then((list) => {
      list.forEach((d) => found.add(d.id));
      // gemerkte Liste aktualisieren (neue vorne, alte behalten)
      const byId = new Map(cache.map((d) => [d.id, d]));
      list.forEach((d) => byId.set(d.id, d));
      cache = [...byId.values()].sort((a, b) => a.name.localeCompare(b.name, "de"));
      localStorage.setItem("ml.castCache", JSON.stringify(cache));
      return list;
    })
    .catch((e) => {
      toast(String(e));
      return [] as CastDevice[];
    })
    .finally(() => {
      searching = null;
      store.emit("outputs");
    });
  return searching;
}

/** Nicht öfter als alle 20 s neu suchen, außer erzwungen */
export function discoverIfStale(force = false) {
  if (force || Date.now() - lastSearch > 20_000) discover();
}

let localCache: LocalOut[] | null = null;
export async function localOutputs(refresh = false): Promise<LocalOut[]> {
  if (localCache && !refresh) return localCache;
  const out: LocalOut[] = [{ id: "default", label: "Dieser PC (Standard)" }];
  try {
    const devs = await navigator.mediaDevices.enumerateDevices();
    let n = 0;
    for (const d of devs) {
      if (d.kind !== "audiooutput" || d.deviceId === "default" || d.deviceId === "communications") continue;
      n++;
      out.push({ id: d.deviceId, label: d.label || `Ausgang ${n}` });
    }
  } catch {
    /* keine Geräteliste */
  }
  localCache = out;
  return out;
}

/** Gerätenamen sind erst nach einer Freigabe lesbar (Windows fragt einmal nach dem Mikrofon – es wird nichts aufgenommen). */
export async function unlockLabels() {
  try {
    const s = await navigator.mediaDevices.getUserMedia({ audio: true });
    s.getTracks().forEach((t) => t.stop());
  } catch {
    toast("Ohne Freigabe bleiben die Gerätenamen verborgen – die Auswahl funktioniert trotzdem.");
  }
  await localOutputs(true);
  store.emit("outputs");
}

export function labelsHidden() {
  return !!localCache && localCache.length > 1 && localCache.slice(1).every((o) => /^Ausgang \d+$/.test(o.label));
}

/** Netzwerkgerät wählen – falls es diese Sitzung noch nicht gefunden wurde, erst suchen */
export async function selectCast(d: CastDevice) {
  if (!found.has(d.id)) {
    toast(`Suche „${d.name}“ …`);
    await discover();
    if (!found.has(d.id)) {
      toast(`„${d.name}“ ist gerade nicht erreichbar.`);
      return;
    }
  }
  localStorage.setItem("ml.output", JSON.stringify({ kind: "cast", id: d.id }));
  await player.setDevice(cache.find((x) => x.id === d.id) ?? d);
}

export async function selectLocal(id: string) {
  localStorage.setItem("ml.output", JSON.stringify({ kind: "local", id }));
  await player.setLocalSink(id);
}

/** Beim Start: zuletzt gewählte Ausgabe wiederherstellen (Netzwerkgerät erst nach erfolgreicher Suche, ohne Abspielen) */
export async function restoreOutput() {
  const o = JSON.parse(localStorage.getItem("ml.output") ?? "null") as { kind: string; id: string } | null;
  if (player.localSink !== "default") await player.setLocalSink(player.localSink);
  if (o?.kind === "cast") {
    await discover();
    const d = cache.find((x) => x.id === o.id);
    if (d && found.has(d.id) && !player.device) await player.setDevice(d);
  }
}

export function currentOutputLabel(): string {
  if (player.device) return player.device.name;
  if (player.localSink === "default") return "Dieser PC";
  return localCache?.find((o) => o.id === player.localSink)?.label ?? "Dieser PC";
}

/** Auswahl-Fenster am Player: lokale Ausgänge sofort, Heimnetz aus dem Gedächtnis + Live-Suche */
export function outputPopup(anchor: HTMLElement) {
  document.querySelectorAll(".out-pop").forEach((m) => m.remove());
  const box = h("div", { class: "menu out-pop" });
  document.body.appendChild(box);
  const place = () => {
    const r = anchor.getBoundingClientRect();
    const b = box.getBoundingClientRect();
    box.style.left = Math.max(8, Math.min(r.right - b.width, window.innerWidth - b.width - 8)) + "px";
    box.style.top = Math.max(8, r.top - b.height - 8) + "px";
  };
  const row = (label: string, on: boolean, hint: string, action: () => void, dim = false) =>
    h("div", { class: "option" + (dim ? " dim" : ""), onclick: () => { close(); action(); } },
      h("span", {}, (on ? "✓ " : "  ") + label), hint ? h("small", {}, hint) : "");
  const render = async () => {
    const locals = await localOutputs();
    box.replaceChildren(
      h("div", { class: "pop-title" }, "Dieser PC"),
      ...locals.map((o) => row(o.id === "default" ? "Dieser PC" : o.label, !player.device && player.localSink === o.id, o.id === "default" ? "Standard" : "", () => selectLocal(o.id))),
      h("hr"),
      h("div", { class: "pop-title" }, "Heimnetz", isSearching() ? h("span", { class: "searching" }, "  sucht …") : ""),
      ...cache.map((d) =>
        row(d.name, player.device?.id === d.id, (player.device?.id === d.id ? "streamt · " : "") + (d.kind === "sonos" ? "Sonos" : "DLNA"), () => selectCast(d), !isFound(d.id) && isSearching()),
      ),
      cache.length ? "" : h("div", { class: "option dim" }, isSearching() ? "Suche Geräte …" : "Keine Geräte gefunden"),
      h("div", { class: "option small", onclick: () => discoverIfStale(true) }, h("span", {}, "  Neu suchen")),
    );
    place();
  };
  const unsub = store.on("outputs", render);
  const unsub2 = store.on("player", render);
  const off = (e: Event) => {
    if (!(e.target as HTMLElement).closest(".out-pop") && e.target !== anchor) close();
  };
  const esc = (e: KeyboardEvent) => e.key === "Escape" && close();
  function close() {
    box.remove();
    unsub();
    unsub2();
    document.removeEventListener("pointerdown", off, true);
    document.removeEventListener("keydown", esc, true);
  }
  setTimeout(() => {
    document.addEventListener("pointerdown", off, true);
    document.addEventListener("keydown", esc, true);
  });
  render();
  discoverIfStale();
}

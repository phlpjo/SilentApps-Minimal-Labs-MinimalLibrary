// Ausgabegeräte: Soundkarten dieses PCs + Lautsprecher im Heimnetz (Sonos/DLNA).
// Bekannte Netzwerkgeräte werden gemerkt und sofort angezeigt; die Suche läuft im Hintergrund.

import { api, type CastDevice } from "./api";
import { player } from "./player";
import { store } from "./store";
import { h, holdButton, toast } from "./ui";
import { L } from "./i18n";
import { IS_MAC } from "./platform";
import { MY_SESSION, busyBy, house, outNameOf, sessionCmd, takeOver, whoOf, type House, type Session } from "./remotectl";

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
let hidden = false;
let unlockTried = false;
// Einmal gelesene Gerätenamen merken (IDs bleiben für die App gleich)
const knownLabels: Record<string, string> = (() => {
  try { return JSON.parse(localStorage.getItem("ml.outLabels") ?? "{}"); } catch { return {}; }
})();

async function readOutputs(): Promise<{ list: LocalOut[]; hidden: boolean }> {
  const list: LocalOut[] = [{ id: "default", label: L("Dieser PC (Standard)") }];
  let n = 0;
  let anon = false;
  try {
    const devs = await navigator.mediaDevices.enumerateDevices();
    for (const d of devs) {
      if (d.kind !== "audiooutput") continue;
      if (!d.deviceId) { anon = true; continue; } // ohne Freigabe: nur ein namenloser Eintrag
      if (d.deviceId === "default" || d.deviceId === "communications") continue;
      n++;
      const label = d.label || knownLabels[d.deviceId] || "";
      if (d.label && knownLabels[d.deviceId] !== d.label) knownLabels[d.deviceId] = d.label;
      if (!label) anon = true;
      list.push({ id: d.deviceId, label: label || L("Ausgang {0}", n) });
    }
  } catch {
    /* keine Geräteliste */
  }
  try { localStorage.setItem("ml.outLabels", JSON.stringify(knownLabels)); } catch { /* egal */ }
  return { list, hidden: anon };
}

/** Alle Soundkarten dieses PCs (Standard-Ausgabe zuerst). Unter Windows werden fehlende Namen einmal pro Start automatisch freigeschaltet. */
export async function localOutputs(refresh = false): Promise<LocalOut[]> {
  if (localCache && !refresh) return localCache;
  let r = await readOutputs();
  if (r.hidden && !IS_MAC && !unlockTried) {
    unlockTried = true;
    if (await openAndClose()) r = await readOutputs();
  }
  localCache = r.list;
  hidden = r.hidden;
  return localCache;
}

/** Kurz Zugriff anfordern und sofort beenden – nur damit die Gerätenamen lesbar werden; es wird nichts aufgenommen */
async function openAndClose(): Promise<boolean> {
  try {
    const s = await navigator.mediaDevices.getUserMedia({ audio: true });
    s.getTracks().forEach((t) => t.stop());
    return true;
  } catch {
    return false;
  }
}

/** Gerätenamen von Hand freigeben (falls die automatische Freigabe nicht ging) */
export async function unlockLabels() {
  if (!(await openAndClose())) toast(L("Ohne Freigabe bleiben die Gerätenamen verborgen – die Auswahl funktioniert trotzdem."));
  await localOutputs(true);
  store.emit("outputs");
}

export function labelsHidden() {
  return hidden && !IS_MAC;
}

// Kopfhörer/USB-Karte angesteckt oder entfernt → Liste neu
try {
  navigator.mediaDevices?.addEventListener("devicechange", async () => {
    await localOutputs(true);
    store.emit("outputs");
  });
} catch { /* egal */ }

/** Netzwerkgerät wählen – falls es diese Sitzung noch nicht gefunden wurde, erst suchen */
export async function selectCast(d: CastDevice) {
  if (!found.has(d.id)) {
    toast(L("Suche „{0}“ …", d.name));
    await discover();
    if (!found.has(d.id)) {
      toast(L("„{0}“ ist gerade nicht erreichbar.", d.name));
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
  if (player.localSink === "default") return L("Dieser PC");
  return localCache?.find((o) => o.id === player.localSink)?.label ?? L("Dieser PC");
}

/** Auswahl-Fenster am Player: oben was im Haus läuft (live, steuerbar), darunter jeder Ausgang genau einmal */
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
  let hs: House = { computer: "", sessions: [], outputs: [] };
  let arming = ""; // Ausgang, der zum Übernehmen bereitsteht
  const fmt = (sec?: number) => (sec && sec > 0 ? `${Math.floor(sec / 60)}:${String(Math.floor(sec % 60)).padStart(2, "0")}` : "");

  // Ein Ausgang: frei → antippen; belegt → antippen zeigt „Übernehmen“ zum Gedrückthalten
  const row = (id: string, label: string, on: boolean, hint: string, select: () => Promise<void> | void, dim = false) => {
    const other = on ? undefined : busyBy(hs, id);
    const sub = other ? L("belegt · {0}", whoOf(other, hs.computer)) : hint;
    if (other && arming === id) {
      return h("div", { class: "option armed" }, h("span", {}, "  " + label),
        holdButton(L("Übernehmen"), () => { close(); takeOver(hs, id, select); }, L("halten · {0} pausiert", whoOf(other, hs.computer))));
    }
    return h("div", { class: "option" + (dim ? " dim" : "") + (other ? " busy" : ""), onclick: (e: Event) => {
      if (other) { e.stopPropagation(); arming = id; render(); return; }
      close(); select();
    } }, h("span", {}, (on ? "✓ " : "  ") + label), sub ? h("small", {}, sub) : "");
  };

  // Läuft gerade woanders (andere Tabs, Handys): live mit Tasten
  const card = (s: Session) => {
    const btn = (label: string, title: string, cmd: Record<string, unknown>) =>
      h("button", { class: "btn small", title, disabled: !s.controllable, onclick: (e: Event) => { e.stopPropagation(); sessionCmd(s.id, cmd); setTimeout(refresh, 700); } }, label);
    const vol = typeof s.volume === "number" ? s.volume : 0.5;
    const where = outNameOf(s);
    return h("div", { class: "phone-now" },
      h("div", {}, (s.kind === "phone" ? "📱 " : "🖥 ") + whoOf(s, hs.computer) + (where ? " → " + where : ""),
        h("small", {}, s.title ? (s.playing ? " · " : L(" · pausiert · ")) + (s.artist ? s.artist + " – " : "") + s.title + (s.dur ? `  ${fmt(s.pos)} / ${fmt(s.dur)}` : "") : L(" · spielt nichts"))),
      s.controllable
        ? h("div", { class: "phone-ctl" },
          btn("⏮", L("Zurück"), { cmd: "prev" }),
          btn(s.playing ? "⏸" : "▶", L("Abspielen / Pause"), { cmd: "toggle" }),
          btn("⏭", L("Weiter"), { cmd: "next" }),
          btn("−", L("Leiser"), { cmd: "volume", vol: Math.max(0, vol - 0.1) }),
          btn("+", L("Lauter"), { cmd: "volume", vol: Math.min(1, vol + 0.1) }))
        : h("small", { class: "note" }, L("nur ansehen")),
    );
  };

  const render = async () => {
    const locals = await localOutputs();
    const others = hs.sessions.filter((s) => s.id !== MY_SESSION && !s.self && (s.title || s.playing));
    box.replaceChildren(
      ...(others.length ? [h("div", { class: "pop-title" }, L("Läuft gerade im Haus")), ...others.map(card), h("hr")] : []),
      h("div", { class: "pop-title" }, L("Diese Wiedergabe spielt auf …")),
      ...locals.map((o) => row("local:" + o.id, o.id === "default" ? L("Dieser PC") : o.label, !player.device && player.localSink === o.id, o.id === "default" ? L("Standard") : "", () => selectLocal(o.id))),
      h("div", { class: "pop-title" }, L("Heimnetz"), isSearching() ? h("span", { class: "searching" }, L("  sucht …")) : ""),
      ...cache.map((d) =>
        row(d.id, d.name, player.device?.id === d.id, (player.device?.id === d.id ? L("streamt · ") : "") + (d.kind === "sonos" ? "Sonos" : "DLNA"), () => selectCast(d), !isFound(d.id) && isSearching()),
      ),
      cache.length ? "" : h("div", { class: "option dim" }, isSearching() ? L("Suche Geräte …") : L("Keine Geräte gefunden")),
      h("div", { class: "option small", onclick: (e: Event) => { e.stopPropagation(); discoverIfStale(true); } }, h("span", {}, L("  Neu suchen"))),
    );
    place();
  };
  const refresh = async () => {
    hs = await house();
    if (document.body.contains(box)) render();
  };
  const timer = window.setInterval(() => (document.body.contains(box) ? refresh() : clearInterval(timer)), 1500);
  const unsub = store.on("outputs", render);
  const unsub2 = store.on("player", render);
  const off = (e: Event) => {
    if (!(e.target as HTMLElement).closest(".out-pop") && e.target !== anchor) close();
  };
  const esc = (e: KeyboardEvent) => e.key === "Escape" && close();
  function close() {
    box.remove();
    clearInterval(timer);
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
  refresh();
  discoverIfStale();
}

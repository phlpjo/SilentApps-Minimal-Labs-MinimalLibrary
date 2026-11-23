// Wiedergaben im ganzen Haus: jeder Tab dieses PCs und jedes gekoppelte Handy ist eine eigene Wiedergabe.
// Jede meldet, was sie wo spielt; jede kann (mit Erlaubnis) die anderen steuern.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { cachedDevices, currentOutputLabel, localOutputs, selectCast, selectLocal } from "./outputs";
import { player } from "./player";
import { store } from "./store";
import { MY_TAB, audioOwner, myTabName } from "./tabs";
import { toast } from "./ui";
import { L } from "./i18n";

export interface Session {
  /** "pc:<tab>" oder "phone:<name>" */
  id: string;
  kind: "pc" | "phone";
  device: string;
  tabName?: string;
  playing?: boolean;
  title?: string;
  artist?: string;
  pos?: number;
  dur?: number;
  volume?: number;
  /** PC: {id,name}; Handy: Name als Text, Kennung in outputId */
  output?: { id: string; name: string } | string;
  outputId?: string;
  controllable?: boolean;
  self?: boolean;
}
export interface House { computer: string; sessions: Session[]; outputs: { id: string; name: string; kind?: string }[] }

export const MY_SESSION = "pc:" + MY_TAB;

/** Gleicher Ausgang? Sonos-Kennungen kommen mal mit, mal ohne „uuid:“ */
export const outKey = (id: string) => id.replace(/^uuid:/, "").split("::")[0].toLowerCase();
export const outOf = (s: Session) => (typeof s.output === "object" && s.output ? s.output.id : s.outputId ?? "");
export const outNameOf = (s: Session) => (typeof s.output === "object" && s.output ? s.output.name : (s.output as string) ?? "");
/** Anzeige: „Dieser PC · Tab“ bzw. Name des Handys */
export const whoOf = (s: Session, computer: string) =>
  s.kind === "pc" ? (s.tabName ? `${computer} · ${s.tabName}` : computer) : s.device;

const myOutput = () => (player.device ? player.device.id : "local:" + player.localSink);

let last = "";
let lastAt = 0;
async function report() {
  const t = player.current;
  // Tabs ohne Titel melden nichts – außer dem Haupt-Tab (er liefert die Ausgänge des PCs)
  if (!t && MY_TAB !== audioOwner() && MY_TAB !== "main") return;
  const locals = await localOutputs();
  const outputs = [
    ...locals.map((o) => ({ id: "local:" + o.id, name: o.id === "default" ? L("Dieser PC") : o.label, kind: "pc" })),
    ...cachedDevices().map((d) => ({ id: d.id, name: d.name, kind: d.kind })),
  ];
  const st = {
    tabName: myTabName(),
    playing: player.playing,
    trackId: t?.id ?? null,
    title: t ? store.label(t) : "",
    artist: t?.artist ?? "",
    album: t?.album ?? "",
    hasCover: !!t?.hasCover,
    pos: Math.round(player.currentTime * 10) / 10,
    dur: Math.round(player.duration * 10) / 10,
    volume: Math.round(player.volumeLevel * 100) / 100,
    shuffle: player.shuffle,
    repeat: player.repeat,
    output: { id: myOutput(), name: currentOutputLabel() },
    outputs,
    queue: MY_TAB === "main" ? player.queue.slice(0, 2000) : [],
    index: t ? player.queue.indexOf(t.id) : -1,
  };
  // Position ändert sich ständig – nur bei echten Änderungen sofort, sonst alle 3 s
  const key = JSON.stringify({ ...st, pos: 0 });
  if (key === last && Date.now() - lastAt < 3000) return;
  last = key;
  lastAt = Date.now();
  invoke("now_report", { state: st }).catch(() => {});
}

export function startRemoteControl() {
  setInterval(report, 1000);
  store.on("player", report);
  listen<Record<string, unknown>>("remote-control", async (e) => {
    const c = e.payload;
    // Für diesen Tab? Ohne Ziel: der Tab, der gerade spielt (ältere Handy-Versionen)
    if (typeof c.session === "string" ? c.session !== MY_SESSION : audioOwner() !== MY_TAB) return;
    const num = (k: string) => (typeof c[k] === "number" ? (c[k] as number) : NaN);
    switch (c.cmd) {
      case "toggle": player.toggle(); break;
      case "play": if (!player.playing) player.toggle(); break;
      case "pause":
        if (player.playing) player.toggle();
        if (c.takeover && c.by) toast(L("„{0}“ übernommen von {1}", currentOutputLabel(), String(c.by)));
        break;
      case "next": player.next(); break;
      case "prev": player.prev(); break;
      case "seek": if (isFinite(num("pos")) && player.duration > 0) player.seekFrac(num("pos") / player.duration); break;
      case "volume": if (isFinite(num("vol"))) player.setVolume(Math.max(0, Math.min(1, num("vol")))); break;
      case "shuffle": player.setShuffle(!!c.on); break;
      case "output": await setOutput(String(c.id ?? "local")); break;
      case "queue": {
        // Wiedergabe vom Handy übernehmen: Warteschlange, Titel, Stelle (Kennungen der Haupt-Bibliothek)
        const ids = (Array.isArray(c.ids) ? (c.ids as number[]) : []).filter((id) => store.tracks.has(id));
        if (!ids.length) break;
        if (typeof c.output === "string") await setOutput(c.output);
        const idx = Math.max(0, Math.min(ids.length - 1, num("index") || 0));
        const t = store.tracks.get(ids[idx])!;
        const pos = isFinite(num("pos")) ? num("pos") : 0;
        await player.play(ids[idx], t.duration > 0 ? pos / t.duration : 0, ids);
        if (c.play === false && player.playing) player.toggle();
        break;
      }
    }
    lastAt = 0;
    report();
  });
  report();
}

/** "local" = aktuelle Soundkarte, "local:<id>" = bestimmte Soundkarte, sonst Lautsprecher im Heimnetz */
export async function setOutput(id: string) {
  if (id === "local" || id === "local:") {
    if (player.device) await selectLocal(player.localSink);
    return;
  }
  if (id.startsWith("local:")) {
    const sink = id.slice(6);
    if (player.device || player.localSink !== sink) await selectLocal(sink);
    return;
  }
  const d = cachedDevices().find((x) => outKey(x.id) === outKey(id));
  if (d && player.device?.id !== d.id) await selectCast(d);
}

/** Alle Wiedergaben im Haus */
export function house(): Promise<House> {
  return invoke<House>("house").catch(() => ({ computer: "", sessions: [], outputs: [] }));
}

/** Befehl an eine Wiedergabe (anderer Tab oder Handy) */
export function sessionCmd(session: string, cmd: Record<string, unknown>) {
  return invoke("session_command", { session, cmd }).catch(() => {});
}

/** Wer spielt gerade auf diesem Ausgang (außer mir)? */
export function busyBy(h: House, outputId: string): Session | undefined {
  const k = outKey(outputId);
  return h.sessions.find((s) => s.id !== MY_SESSION && s.playing && outKey(outOf(s)) === k && !(k.startsWith("local:") && s.kind === "phone"));
}

/** Ausgang übernehmen: die andere Wiedergabe pausiert (mit Hinweis), dann hierher umschalten */
export async function takeOver(h: House, outputId: string, select: () => Promise<void> | void) {
  const other = busyBy(h, outputId);
  if (other) await sessionCmd(other.id, { cmd: "pause", takeover: true });
  await select();
}

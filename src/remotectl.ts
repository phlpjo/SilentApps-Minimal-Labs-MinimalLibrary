// Fernsteuerung wie bei Spotify: der PC meldet, was wo läuft; das Handy greift ein – und umgekehrt.
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { cachedDevices, currentOutputLabel, selectCast, selectLocal } from "./outputs";
import { player } from "./player";
import { store } from "./store";
import { MY_TAB, audioOwner } from "./tabs";
import { L } from "./i18n";

export interface PhoneNow {
  name: string;
  playing?: boolean;
  title?: string;
  artist?: string;
  pos?: number;
  dur?: number;
  volume?: number;
  output?: string;
}

/** Nur der Tab, der gerade spielt (oder der erste, wenn keiner spielt), meldet und führt aus */
const isOwner = () => audioOwner() === MY_TAB;

let last = "";
let lastAt = 0;
function report() {
  if (!isOwner()) return;
  const t = player.current;
  const outputs = [{ id: "local", name: L("Dieser PC") }, ...cachedDevices().map((d) => ({ id: d.id, name: d.name }))];
  const st = {
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
    output: { id: player.device?.id ?? "local", name: currentOutputLabel() },
    outputs,
    queue: player.queue.slice(0, 2000),
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
    if (!isOwner()) return;
    const c = e.payload;
    const num = (k: string) => (typeof c[k] === "number" ? (c[k] as number) : NaN);
    switch (c.cmd) {
      case "toggle": player.toggle(); break;
      case "play": if (!player.playing) player.toggle(); break;
      case "pause": if (player.playing) player.toggle(); break;
      case "next": player.next(); break;
      case "prev": player.prev(); break;
      case "seek": if (isFinite(num("pos")) && player.duration > 0) player.seekFrac(num("pos") / player.duration); break;
      case "volume": if (isFinite(num("vol"))) player.setVolume(Math.max(0, Math.min(1, num("vol")))); break;
      case "shuffle": player.setShuffle(!!c.on); break;
      case "output": await setOutput(String(c.id ?? "local")); break;
      case "queue": {
        // Wiedergabe vom Handy übernehmen: Warteschlange, Titel, Stelle
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
    report();
  });
  report();
}

async function setOutput(id: string) {
  if (id === "local") {
    if (player.device) await selectLocal(player.localSink);
    return;
  }
  const d = cachedDevices().find((x) => x.id === id);
  if (d && player.device?.id !== d.id) await selectCast(d);
}

/** Was die Handys gerade spielen (für das Ausgabe-Menü) */
export function phonesNow(): Promise<PhoneNow[]> {
  return invoke<PhoneNow[]>("remote_phones").catch(() => []);
}

/** Befehl an ein Handy */
export function phoneCmd(name: string, cmd: Record<string, unknown>) {
  return invoke("phone_command", { name, cmd }).catch(() => {});
}

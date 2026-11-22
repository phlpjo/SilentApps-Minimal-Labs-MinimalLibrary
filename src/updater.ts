// Auto-Update über GitHub-Releases (signiert). Prüft beim Start und alle 6 Stunden.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { player } from "./player";
import { store } from "./store";
import { toast } from "./ui";
import { L } from "./i18n";

export const updater = {
  available: null as Update | null,
  state: "idle" as "idle" | "checking" | "none" | "available" | "downloading" | "error",
  progress: 0,
  error: "",
  lastCheck: 0,
};

/** Beim Start automatisch installieren (nur wenn gerade nichts läuft) */
export const autoInstall = {
  get on() { return localStorage.getItem("ml.autoUpdate") !== "0"; },
  set on(v: boolean) { localStorage.setItem("ml.autoUpdate", v ? "1" : "0"); },
};

const emit = () => store.emit("update");

export async function checkUpdate(manual = false): Promise<void> {
  if (updater.state === "checking" || updater.state === "downloading") return;
  updater.state = "checking";
  emit();
  try {
    const u = await check();
    updater.lastCheck = Date.now();
    updater.available = u;
    updater.state = u ? "available" : "none";
    if (u && !manual) toast(L("Update {0} verfügbar – Einstellungen → Updates", u.version));
    if (manual && !u) toast(L("Minimal Library ist aktuell"));
  } catch (e) {
    updater.state = "error";
    updater.error = String(e);
    if (manual) toast(L("Update-Prüfung fehlgeschlagen: ") + updater.error);
  }
  emit();
}

export async function installUpdate(): Promise<void> {
  const u = updater.available;
  if (!u || updater.state === "downloading") return;
  updater.state = "downloading";
  updater.progress = 0;
  emit();
  let total = 0;
  let got = 0;
  try {
    await u.downloadAndInstall((ev) => {
      if (ev.event === "Started") total = ev.data.contentLength ?? 0;
      else if (ev.event === "Progress") {
        got += ev.data.chunkLength;
        updater.progress = total ? got / total : 0;
        emit();
      }
    });
    await relaunch();
  } catch (e) {
    updater.state = "error";
    updater.error = String(e);
    toast(L("Update fehlgeschlagen: ") + updater.error);
    emit();
  }
}

export function startUpdater() {
  setTimeout(async () => {
    await checkUpdate();
    // Beim Start: ohne Rückfrage installieren, solange keine Musik läuft
    if (updater.available && autoInstall.on && !player.playing) installUpdate();
  }, 4000);
  setInterval(() => checkUpdate(), 6 * 3600 * 1000);
}

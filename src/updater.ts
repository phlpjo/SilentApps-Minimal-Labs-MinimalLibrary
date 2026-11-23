// Auto-Update über GitHub-Releases (signiert). Prüft beim Start und alle 6 Stunden.
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { player } from "./player";
import { store } from "./store";
import { h, sheet, toast } from "./ui";
import { L } from "./i18n";
import { license } from "./license";

export const updater = {
  available: null as Update | null,
  state: "idle" as "idle" | "checking" | "none" | "available" | "downloading" | "error",
  /** schon im Hintergrund heruntergeladen – Installieren geht dann sofort */
  ready: false,
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

/** silent: nur prüfen (der Start entscheidet selbst, ob installiert oder gefragt wird) */
export async function checkUpdate(manual = false, silent = false): Promise<void> {
  if (updater.state === "checking" || updater.state === "downloading") return;
  updater.state = "checking";
  emit();
  try {
    // Mit aktiver Lizenz: deren Kennung mitschicken, damit die Website die Abfrage der Lizenz zuordnen kann
    const id = license.state === "licensed" ? license.id : "";
    const u = await check(id ? { headers: { "X-License-Id": id } } : undefined);
    updater.lastCheck = Date.now();
    updater.available = u;
    updater.state = u ? "available" : "none";
    updater.ready = false;
    if (u && !manual && !silent) prefetchAndAsk(u);
    if (manual && !u) toast(L("Minimal Library ist aktuell"));
  } catch (e) {
    updater.state = "error";
    updater.error = String(e);
    if (manual) toast(L("Update-Prüfung fehlgeschlagen: ") + updater.error);
  }
  emit();
}

/** Merker: beim nächsten Start installieren */
const NEXT = "ml.installNext";

/** Gefundenes Update gleich herunterladen, dann fragen: jetzt oder beim nächsten Start */
async function prefetchAndAsk(u: Update) {
  try {
    await u.download();
    updater.ready = true;
  } catch {
    /* Herunterladen klappt beim Installieren noch einmal */
  }
  if (document.hidden) return; // nur im sichtbaren Tab fragen
  try {
    if (localStorage.getItem("ml.updateAsked") === u.version) return;
    localStorage.setItem("ml.updateAsked", u.version);
  } catch {}
  const s = sheet(L("Update {0} verfügbar", u.version), h("div", {},
    h("div", { class: "note" }, updater.ready ? L("Das Update ist schon heruntergeladen. Jetzt installieren? Minimal Library startet dabei kurz neu.") : L("Jetzt installieren? Minimal Library startet dabei kurz neu.")),
    h("div", { class: "foot" },
      h("button", { class: "btn", onclick: () => {
        try { localStorage.setItem(NEXT, u.version); } catch {}
        s.close();
        toast(L("Wird beim nächsten Start installiert"));
      } }, L("Nein – beim nächsten Start")),
      h("button", { class: "btn primary", onclick: () => { s.close(); installUpdate(); } }, L("Jetzt installieren")),
    ),
  ));
}

export async function installUpdate(): Promise<void> {
  const u = updater.available;
  if (!u || updater.state === "downloading") return;
  try { localStorage.removeItem(NEXT); } catch {}
  if (updater.ready) {
    updater.state = "downloading";
    updater.progress = 1;
    emit();
    try {
      await u.install();
      await relaunch();
    } catch (e) {
      updater.state = "error";
      updater.error = String(e);
      toast(L("Update fehlgeschlagen: ") + updater.error);
      emit();
    }
    return;
  }
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
    let next = false;
    try { next = !!localStorage.getItem(NEXT); } catch {}
    await checkUpdate(false, true);
    if (!updater.available) try { localStorage.removeItem(NEXT); } catch {}
    // Beim Start: ohne Rückfrage installieren (eingestellt oder „beim nächsten Start“ gewählt), solange keine Musik läuft
    if (updater.available && (autoInstall.on || next) && !player.playing) installUpdate();
    else if (updater.available && !document.hidden) prefetchAndAsk(updater.available);
  }, 4000);
  setInterval(() => checkUpdate(), 6 * 3600 * 1000);
}

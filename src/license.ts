// Lizenz: 90 Tage Testphase, danach sind sechs Funktionen nur mit Schlüssel nutzbar.
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api, type LicenseStatus } from "./api";
import { LANG, L } from "./i18n";
import { store } from "./store";
import { h, sheet, toast, toastHooks } from "./ui";

/** Funktionen der Vollversion */
export type Feature = "connect" | "organize" | "smart" | "search" | "sync" | "ripper";

export const FEATURE_NAME: Record<Feature, () => string> = {
  connect: () => L("App verbinden (Minimal Player Connected)"),
  organize: () => L("Sortieren nach Muster"),
  smart: () => L("Smarte Playlisten"),
  search: () => L("Erweiterte Suche"),
  sync: () => L("Sync"),
  ripper: () => L("Ripper"),
};

export const BUY_URL = () => (LANG === "de" ? "https://silentapps-minimal-labs.com/de/app/minimal-library" : "https://silentapps-minimal-labs.com/app/minimal-library");

export let license: LicenseStatus = { state: "trial", daysLeft: 90, name: "", email: "", kind: "", trialStart: 0 };

/** Neuen Zustand übernehmen (nach Freischalten/Entfernen) */
export function setLicense(s: LicenseStatus) {
  license = s;
  store.emit("license");
}

export const unlocked = () => license.state === "trial" || license.state === "licensed";

export async function loadLicense() {
  license = await api.licenseStatus().catch(() => license);
  store.emit("license");
  // kurz vor Ende der Testphase einmal am Tag erinnern
  if (license.state === "trial" && license.daysLeft <= 7) {
    const today = new Date().toDateString();
    try {
      if (localStorage.getItem("ml.trialHint") !== today) {
        localStorage.setItem("ml.trialHint", today);
        toast(L("Testphase: noch {0} Tage. Einstellungen → Lizenz", license.daysLeft), 6000);
      }
    } catch {}
  }
}

listen<LicenseStatus>("license-changed", (e) => {
  const was = license.state;
  license = e.payload;
  store.emit("license");
  if (license.state === "licensed" && was !== "licensed") toast(L("Danke! Lizenziert für {0}", license.name || "–"), 6000);
});
listen<string>("license-error", (e) => toast(L("Freischalten fehlgeschlagen: {0}", e.payload)));

/** Vor einer Funktion der Vollversion aufrufen: false = gesperrt (Hinweis wird gezeigt) */
export function requireLicense(f: Feature): boolean {
  if (unlocked()) return true;
  licenseSheet(f);
  return false;
}

/** Hinweis „gehört zur Vollversion“ mit Schlüssel-Eingabe und Kauf-Link */
export function licenseSheet(f?: Feature) {
  const input = h("input", { type: "text", placeholder: "MLIB-…", style: { width: "100%", fontFamily: "var(--mono)", fontSize: "12px" } }) as HTMLInputElement;
  const msg = h("div", { class: "note" });
  const s = sheet(L("Vollversion"), h("div", {},
    h("div", { class: "note", style: { marginBottom: "12px" } },
      f ? L("„{0}“ gehört zur Vollversion. Die 90 Tage Testphase sind vorbei.", FEATURE_NAME[f]()) : L("Die 90 Tage Testphase sind vorbei."),
      " ",
      L("Alles andere bleibt kostenlos nutzbar."),
    ),
    input,
    msg,
    h("div", { class: "foot" },
      h("button", { class: "btn", onclick: () => openUrl(BUY_URL()).catch(() => {}) }, L("Lizenz kaufen …")),
      h("button", { class: "btn primary", onclick: async () => {
        try {
          license = await api.licenseActivate(input.value);
          store.emit("license");
          s.close();
        } catch (e) {
          msg.textContent = String(e);
        }
      } }, L("Freischalten")),
    ),
  ));
  setTimeout(() => input.focus(), 50);
}

/** Meldung vom Programmkern „LICENSE:<funktion>“ → Hinweis statt Fehlertext */
export function licenseFromError(msg: string): boolean {
  const m = /LICENSE:(\w+)/.exec(msg);
  if (!m) return false;
  licenseSheet(m[1] as Feature);
  return true;
}

toastHooks.license = licenseFromError;

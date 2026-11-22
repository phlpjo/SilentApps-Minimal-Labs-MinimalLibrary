// Sprache: Deutsch oder Englisch – Standard folgt dem System, in den Einstellungen umschaltbar.
// Texte stehen im Code auf Deutsch; L() liefert die englische Fassung aus EN (Platzhalter {0}, {1} …).
import { EN } from "./i18n-en";

export type Lang = "de" | "en";
export type LangMode = "system" | Lang;

export function langMode(): LangMode {
  try {
    return (localStorage.getItem("ml.lang") as LangMode) ?? "system";
  } catch {
    return "system";
  }
}

function detect(): Lang {
  const m = langMode();
  if (m === "de" || m === "en") return m;
  const l = (navigator.languages?.[0] ?? navigator.language ?? "en").toLowerCase();
  return l.startsWith("de") ? "de" : "en";
}

export const LANG: Lang = detect();
/** Für Zahlen und Datumsangaben */
export const LOCALE = LANG === "de" ? "de-DE" : "en-GB";
document.documentElement.lang = LANG;

export function setLangMode(m: LangMode) {
  try {
    localStorage.setItem("ml.lang", m);
  } catch {}
  location.reload();
}

const fill = (s: string, args: unknown[]) => s.replace(/\{(\d+)\}/g, (_, i) => String(args[Number(i)] ?? ""));

/** Übersetzt einen deutschen Text (mit {0}, {1} … als Platzhalter) */
export function L(de: string, ...args: unknown[]): string {
  const s = LANG === "en" ? (EN[de] ?? de) : de;
  return args.length ? fill(s, args) : s;
}

/** Einzahl/Mehrzahl: Ln(n, "{0} Titel", "{0} Titel") */
export function Ln(n: number, one: string, many: string, ...args: unknown[]): string {
  return L(n === 1 ? one : many, n, ...args);
}

// Meldungen vom Programmkern (Rust) kommen auf Deutsch – Muster mit {0} … werden hier erkannt
let patterns: [RegExp, string][] | null = null;
function compile() {
  patterns = [];
  for (const [de, en] of Object.entries(EN)) {
    if (!/\{\d+\}/.test(de)) continue;
    const re = "^" + de.replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\\?\{(\d+)\\?\}/g, "(?<a$1>[\\s\\S]*?)") + "$";
    try {
      patterns.push([new RegExp(re), en]);
    } catch {}
  }
}

/** Freien Text übersetzen (Fehlermeldungen, Fortschritt): exakt, Muster, sonst „Teil: Rest“ einzeln */
export function trMsg(msg: string): string {
  if (LANG === "de" || !msg) return msg;
  if (EN[msg]) return EN[msg];
  if (!patterns) compile();
  for (const [re, en] of patterns!) {
    const m = re.exec(msg);
    if (m) return en.replace(/\{(\d+)\}/g, (_, i) => trMsg(m.groups?.["a" + i] ?? ""));
  }
  const i = msg.indexOf(": ");
  if (i > 0) {
    const head = msg.slice(0, i);
    if (EN[head]) return EN[head] + ": " + trMsg(msg.slice(i + 2));
  }
  return msg;
}

/** Mehrzahl-Endung: Deutsch die angegebene („en“, „n“ …), Englisch „s“ */
export function pl(n: number, de: string): string {
  return n === 1 ? "" : LANG === "en" ? "s" : de;
}

/** Vom Programm angelegte Namen (Stern-Playlist, erster Tab) in der Sprache der Oberfläche zeigen */
export function shown(name: string): string {
  return name === "Stern" || name === "Bibliothek" ? L(name) : name;
}

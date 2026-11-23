import { L } from "./i18n";
// Plattform-Unterschiede Windows / macOS an einer Stelle
export const IS_MAC = /Mac/i.test(navigator.platform || navigator.userAgent);
/** Läuft im Browser vom NAS (Synology-Paket) */
export const IS_WEB = document.documentElement.classList.contains("web");
export const OS = IS_WEB ? L("System") : IS_MAC ? "macOS" : L("Windows");
export const FILE_MANAGER = IS_WEB ? L("NAS-Pfad") : IS_MAC ? L("Finder") : L("Explorer");
export const SHOW_IN = IS_WEB ? L("Pfad auf dem NAS kopieren") : L("Im {0} zeigen", FILE_MANAGER);
/** Wiedergabe auf diesem Gerät */
export const THIS_DEVICE = () => (IS_WEB ? L("Dieser Browser") : L("Dieser PC"));
export const TRASH = IS_MAC ? L("macOS-Papierkorb") : L("Windows-Papierkorb");
/** Strg (Windows) bzw. ⌘ (Mac) */
export const mod = (e: KeyboardEvent | MouseEvent) => (IS_MAC ? e.metaKey : e.ctrlKey);
/** Kopieren beim Ziehen: Strg (Windows) bzw. ⌥ (Mac) */
export const copyKey = (e: MouseEvent | PointerEvent) => (IS_MAC ? e.altKey : e.ctrlKey);
if (IS_MAC) document.documentElement.classList.add("mac");

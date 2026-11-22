import { L } from "./i18n";
// Plattform-Unterschiede Windows / macOS an einer Stelle
export const IS_MAC = /Mac/i.test(navigator.platform || navigator.userAgent);
export const OS = IS_MAC ? "macOS" : L("Windows");
export const FILE_MANAGER = IS_MAC ? L("Finder") : L("Explorer");
export const SHOW_IN = L("Im {0} zeigen", FILE_MANAGER);
export const TRASH = IS_MAC ? L("macOS-Papierkorb") : L("Windows-Papierkorb");
/** Strg (Windows) bzw. ⌘ (Mac) */
export const mod = (e: KeyboardEvent | MouseEvent) => (IS_MAC ? e.metaKey : e.ctrlKey);
/** Kopieren beim Ziehen: Strg (Windows) bzw. ⌥ (Mac) */
export const copyKey = (e: MouseEvent | PointerEvent) => (IS_MAC ? e.altKey : e.ctrlKey);
if (IS_MAC) document.documentElement.classList.add("mac");

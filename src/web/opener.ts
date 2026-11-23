// Browser-Ersatz für @tauri-apps/plugin-opener
import { toast } from "../ui";
export async function openUrl(url: string) {
  window.open(url, "_blank", "noopener");
}
export async function openPath(p: string) {
  toast(p, 6000);
}
/** Im Browser gibt es keinen Explorer – Pfad auf dem NAS zeigen (und kopieren) */
export async function revealItemInDir(p: string) {
  navigator.clipboard?.writeText(p).catch(() => {});
  toast(`NAS: ${p}`, 6000);
}

// Browser-Ersatz für @tauri-apps/api/window: ein Fenster („main“), Fensterknöpfe gibt es nicht.
const win = {
  label: "main",
  async minimize() {},
  async toggleMaximize() {},
  async close() {},
  async isMaximized() { return false; },
  async setTheme(_t?: string | null) {},
  async setTitle(t: string) { document.title = t; },
  async show() {},
  async setFocus() { window.focus(); },
  async onResized(cb: () => void) { window.addEventListener("resize", cb); return () => window.removeEventListener("resize", cb); },
  async onCloseRequested(_cb: unknown) { return () => {}; },
};
export function getCurrentWindow() {
  return win;
}

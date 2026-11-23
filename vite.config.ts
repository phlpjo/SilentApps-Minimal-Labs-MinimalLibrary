import { defineConfig } from "vite";
import { resolve } from "node:path";

// `vite build --mode web`: Browser-Version für das NAS-Paket (Synology). Tauri-Schnittstellen werden durch
// HTTP-Aufrufe an den NAS-Server ersetzt (src/web/*), Ausgabe nach dist-web.
export default defineConfig(({ mode }) => {
  const web = mode === "web";
  const shim = (f: string) => resolve(__dirname, "src/web", f);
  return {
    clearScreen: false,
    server: { port: 1420, strictPort: true },
    envPrefix: ["VITE_", "TAURI_"],
    resolve: web
      ? {
          alias: {
            "@tauri-apps/api/core": shim("core.ts"),
            "@tauri-apps/api/event": shim("event.ts"),
            "@tauri-apps/api/window": shim("window.ts"),
            "@tauri-apps/api/webview": shim("webview.ts"),
            "@tauri-apps/plugin-dialog": shim("dialog.ts"),
            "@tauri-apps/plugin-opener": shim("opener.ts"),
            "@tauri-apps/plugin-process": shim("process.ts"),
            "@tauri-apps/plugin-updater": shim("updater.ts"),
          },
        }
      : undefined,
    build: web
      ? { target: "es2022", outDir: "dist-web", emptyOutDir: true, rollupOptions: { input: resolve(__dirname, "web.html") } }
      : { target: "es2022", outDir: "dist", emptyOutDir: true },
  };
});

import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { store } from "../store";
import { clear, h, holdButton, toast } from "../ui";
import { analyzeIds, scanNow } from "../main";
import { applyTheme, themeMode, type ThemeMode } from "../theme";

export function settingsView(): { el: HTMLElement; destroy: () => void } {
  const body = h("div", { class: "view-scroll" });
  let version = "";
  api.version().then((v) => { version = v; render(); });

  const render = () => {
    const s = store.settings;
    clear(body);
    const roots = h("div", {},
      ...(s.roots.length
        ? s.roots.map((r) =>
            h("div", { class: "setting" }, h("span", { style: { fontFamily: "var(--mono)", fontSize: "12px" } }, r),
              h("button", { class: "btn", onclick: async () => { await store.saveSettings({ roots: s.roots.filter((x) => x !== r) }); render(); scanNow(); } }, "Entfernen"),
            ),
          )
        : [h("div", { class: "note" }, "Noch kein Musikordner. Füge den Ordner hinzu, in dem deine Musik liegt.")]),
    );
    const threads = h("select", {}, ...[1, 2, 3, 4, 6, 8, 12, 16].map((n) => h("option", { value: String(n), selected: n === s.threads }, String(n))));
    threads.addEventListener("change", () => store.saveSettings({ threads: Number(threads.value) }));
    const auto = h("input", { type: "checkbox", checked: s.autoAnalyze });
    auto.addEventListener("change", () => store.saveSettings({ autoAnalyze: auto.checked }));

    const theme = h("select", {}, ...([["system", "Wie Windows"], ["dark", "Dunkel"], ["light", "Hell"]] as const).map(([v, l]) => h("option", { value: v, selected: themeMode() === v }, l)));
    theme.addEventListener("change", () => applyTheme(theme.value as ThemeMode));
    body.append(
      h("div", { class: "card" }, h("h3", {}, "Darstellung"), h("div", { class: "setting" }, h("span", {}, "Farbschema"), theme),
        h("div", { class: "note" }, "Der Schalter oben links wechselt direkt zwischen Hell und Dunkel.")),
      h("div", { class: "card" }, h("h3", {}, "Musikordner"), roots,
        h("div", { class: "line" },
          h("button", { class: "btn primary", onclick: async () => {
            const d = await open({ directory: true, multiple: false });
            if (!d) return;
            await store.saveSettings({ roots: [...new Set([...s.roots, d as string])] });
            render();
            scanNow();
          } }, "Ordner hinzufügen …"),
          h("button", { class: "btn", onclick: () => scanNow() }, "Jetzt scannen"),
        ),
        h("div", { class: "note" }, "Beim Start wird automatisch nach neuen, geänderten und gelöschten Dateien gesucht."),
      ),
      h("div", { class: "card" }, h("h3", {}, "Playlist-Ordner für Minimal Player"),
        h("div", { class: "setting" }, h("span", {}, "Ordner"), h("span", { class: "v", style: { fontFamily: "var(--mono)", fontSize: "12px" } }, s.playlistDir || "aus")),
        h("div", { class: "line" },
          h("button", { class: "btn", onclick: async () => {
            const d = await open({ directory: true });
            if (!d) return;
            await store.saveSettings({ playlistDir: d as string });
            const n = await api.syncPlaylistFolder().catch(() => 0);
            toast(`${n} Playlisten geschrieben`);
            render();
          } }, "Wählen …"),
          s.playlistDir ? h("button", { class: "btn", onclick: async () => { await store.saveSettings({ playlistDir: null }); render(); } }, "Ausschalten") : null,
          s.playlistDir ? h("button", { class: "btn", onclick: async () => toast(`${await api.syncPlaylistFolder()} Playlisten geschrieben`) }, "Jetzt schreiben") : null,
        ),
        h("div", { class: "note" }, "Alle Playlisten werden hier automatisch als .m3u8 mit relativen Pfaden aktuell gehalten. Liegt der Ordner im Musikordner (z. B. Musik\\Playlists), kopierst du einfach den ganzen Musikordner aufs Handy – Minimal Player findet Lieder und Playlisten."),
      ),
      h("div", { class: "card" }, h("h3", {}, "Analyse"),
        h("div", { class: "setting" }, h("span", {}, "Gleichzeitige Analysen"), threads),
        h("div", { class: "setting" }, h("label", { class: "check" }, auto, "Neue Titel automatisch analysieren"), h("span", {})),
        h("div", { class: "line" },
          h("button", { class: "btn", onclick: () => analyzeIds(null) }, "Fehlende analysieren"),
          holdButton("Alles neu analysieren", () => analyzeIds([...store.tracks.keys()]), "halten · dauert"),
        ),
        h("div", { class: "note" }, "Die Analyse erstellt Wellenform, Audio-Fingerabdruck (für Duplikate), BPM und Tonart. Sie liest jede Datei einmal vollständig."),
      ),
      h("div", { class: "card" }, h("h3", {}, "Tastatur"),
        ...[
          ["Leertaste", "Abspielen / Pause"], ["← / →", "5 Sekunden zurück / vor"], ["Strg+F", "Suchen"], ["Strg+A", "Alle auswählen"],
          ["Strg+S", "Tags speichern"], ["Enter", "Auswahl abspielen"], ["Entf", "Aus Playlist entfernen"], ["Doppelklick", "Abspielen"], ["Klick in Wellenform", "ab dieser Stelle abspielen"],
        ].map(([k, v]) => h("div", { class: "setting" }, h("span", {}, v), h("span", { class: "v", style: { fontFamily: "var(--mono)", fontSize: "12px" } }, k))),
      ),
      h("div", { class: "card" }, h("h3", {}, "Über"),
        h("div", { class: "setting" }, h("span", {}, "Version"), h("span", { class: "v" }, version)),
        h("div", { class: "setting" }, h("span", {}, "Lizenzen"), h("span", { class: "v" }, "Space Grotesk (OFL 1.1) · Symphonia (MPL-2.0) · Lofty (MIT/Apache-2.0) · rusty-chromaprint (MIT) · Tauri (MIT/Apache-2.0)")),
        h("div", { class: "option", onclick: () => openUrl("https://silentapps-minimal-labs.com/de/") }, "Weitere Apps von SilentApps Minimal Labs", h("small", {}, "›")),
        h("div", { class: "option", onclick: () => openUrl(`mailto:apps@silentapps-minimal-labs.com?subject=${encodeURIComponent("Minimal Library – Feedback")}&body=${encodeURIComponent(`\n\n—\nMinimal Library ${version}\n${navigator.userAgent}`)}`) }, "Fehler melden & Vorschläge", h("small", {}, "›")),
      ),
    );
  };
  const unsub = store.on("settings", render);
  render();
  const el = h("div", { style: { display: "contents" } }, h("div", { class: "head" }, h("h1", {}, "Einstellungen")), body);
  return { el, destroy: unsub };
}

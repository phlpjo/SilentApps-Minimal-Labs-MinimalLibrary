import { open } from "@tauri-apps/plugin-dialog";
import { openUrl } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { store } from "../store";
import { clear, h, holdButton } from "../ui";
import { analyzeIds, scanNow } from "../main";
import { applyTheme, themeMode, type ThemeMode } from "../theme";
import { IS_MAC, FILE_MANAGER } from "../platform";
import { autoInstall, checkUpdate, installUpdate, updater } from "../updater";

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
    const acoustid = h("input", { type: "text", value: s.acoustidKey ?? "", placeholder: "z. B. AbCdEf1234", spellcheck: false, style: { width: "220px", fontFamily: "var(--mono)" } });
    acoustid.addEventListener("change", () => store.saveSettings({ acoustidKey: acoustid.value.trim() }));
    const auto = h("input", { type: "checkbox", checked: s.autoAnalyze });
    auto.addEventListener("change", () => store.saveSettings({ autoAnalyze: auto.checked }));

    const tray = h("input", { type: "checkbox", checked: s.closeToTray !== false });
    tray.addEventListener("change", () => store.saveSettings({ closeToTray: tray.checked }));
    const theme = h("select", {}, ...([["system", IS_MAC ? "Wie macOS" : "Wie Windows"], ["dark", "Dunkel"], ["light", "Hell"]] as const).map(([v, l]) => h("option", { value: v, selected: themeMode() === v }, l)));
    theme.addEventListener("change", () => applyTheme(theme.value as ThemeMode));
    body.append(
      h("div", { class: "card" }, h("h3", {}, "Darstellung"), h("div", { class: "setting" }, h("span", {}, "Farbschema"), theme),
        h("div", { class: "setting" }, h("label", { class: "check" }, tray, "Schließen (X) legt die App in den Infobereich – Musik läuft weiter"), h("button", { class: "btn", onclick: () => api.quit() }, "Jetzt beenden")),
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
        h("div", { class: "note" }, `Die Musikordner werden laufend überwacht: Was du im ${FILE_MANAGER} hinzufügst, verschiebst, umbenennst oder löschst, übernimmt die App sofort – Analyse und Playlisten bleiben beim Verschieben erhalten.`),
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
      h("div", { class: "card" }, h("h3", {}, "Online (nur auf Klick)"),
        h("div", { class: "setting" }, h("span", {}, "AcoustID-Schlüssel"), acoustid),
        h("div", { class: "note" },
          "Für „Am Klang erkennen“. Einmalig kostenlos anlegen: ",
          h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); openUrl("https://acoustid.org/new-application"); } }, "acoustid.org/new-application"),
          " (Name z. B. „Minimal Library“), den angezeigten API-Schlüssel hier einfügen. Übertragen werden nur Fingerabdruck und Dauer.",
        ),
      ),
      h("div", { class: "card" }, h("h3", {}, "Tastatur"),
        ...[
          ["Leertaste", "Abspielen / Pause"], ["← / →", "5 Sekunden zurück / vor"], ["Strg+F", "Suchen"], ["Strg+A", "Alle auswählen"],
          ["Strg+S", "Tags speichern"], ["Strg+Z", "Letzte Änderung rückgängig"], ["F2", "Datei umbenennen"], ["Enter", "Auswahl abspielen"], ["Entf", "Aus Playlist entfernen"], ["Doppelklick", "Abspielen"], ["Klick in Wellenform", "ab dieser Stelle abspielen"],
        ].map(([k, v]) => h("div", { class: "setting" }, h("span", {}, v), h("span", { class: "v", style: { fontFamily: "var(--mono)", fontSize: "12px" } }, k))),
      ),
      updateCard(version),
      h("div", { class: "card" }, h("h3", {}, "Über"),
        h("div", { class: "setting" }, h("span", {}, "Version"), h("span", { class: "v" }, version)),
        h("div", { class: "setting" }, h("span", {}, "Lizenzen"), h("span", { class: "v" }, "Space Grotesk (OFL 1.1) · Symphonia (MPL-2.0) · Lofty (MIT/Apache-2.0) · rusty-chromaprint (MIT) · Tauri (MIT/Apache-2.0)")),
        h("div", { class: "option", onclick: () => openUrl("https://silentapps-minimal-labs.com/de/") }, "Weitere Apps von SilentApps Minimal Labs", h("small", {}, "›")),
        h("div", { class: "option", onclick: () => openUrl(`mailto:apps@silentapps-minimal-labs.com?subject=${encodeURIComponent("Minimal Library – Feedback")}&body=${encodeURIComponent(`\n\n—\nMinimal Library ${version}\n${navigator.userAgent}`)}`) }, "Fehler melden & Vorschläge", h("small", {}, "›")),
      ),
    );
  };
  const unsubs = [store.on("settings", render), store.on("update", render)];
  render();
  const el = h("div", { style: { display: "contents" } }, h("div", { class: "head" }, h("h1", {}, "Einstellungen")), body);
  return { el, destroy: () => unsubs.forEach((u) => u()) };
}

function updateCard(version: string) {
  const u = updater;
  const status =
    u.state === "checking" ? "Suche nach Updates …"
    : u.state === "available" ? `Version ${u.available!.version} ist verfügbar`
    : u.state === "downloading" ? `Lädt … ${Math.round(u.progress * 100)} %`
    : u.state === "error" ? `Prüfung fehlgeschlagen: ${u.error}`
    : u.state === "none" ? `Aktuell (${version})`
    : `Installiert: ${version}`;
  const auto = h("input", { type: "checkbox", checked: autoInstall.on });
  auto.addEventListener("change", () => (autoInstall.on = auto.checked));
  return h("div", { class: "card" }, h("h3", {}, "Updates"),
    h("div", { class: "setting" }, h("span", {}, status),
      u.state === "available"
        ? h("button", { class: "btn primary", onclick: () => installUpdate() }, "Installieren & neu starten")
        : h("button", { class: "btn", disabled: u.state === "checking" || u.state === "downloading", onclick: () => checkUpdate(true) }, "Jetzt prüfen"),
    ),
    u.state === "available" && u.available!.body ? h("div", { class: "note" }, u.available!.body) : null,
    h("label", { class: "check", style: { marginTop: "8px" } }, auto, "Beim Start automatisch installieren (nur wenn gerade keine Musik läuft)"),
    h("div", { class: "note" }, "Updates kommen signiert aus den GitHub-Releases. Die App prüft beim Start und alle 6 Stunden."),
  );
}

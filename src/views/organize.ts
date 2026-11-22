import { open } from "@tauri-apps/plugin-dialog";
import { api, type PlannedMove } from "../api";
import { player } from "../player";
import { store } from "../store";
import { h, holdButton, reportErrors, toast } from "../ui";
import { historyList } from "./history";

const PLACEHOLDERS: [string, string][] = [
  ["{albumartist}", "Albumkünstler"], ["{artist}", "Künstler"], ["{album}", "Album"], ["{title}", "Titel"], ["{track}", "Track (01)"],
  ["{disc-}", "Disc „2-“ nur wenn > 1"], ["{disc}", "Disc"], ["{year}", "Jahr"], ["{genre}", "Genre"], ["{initial}", "Anfangsbuchstabe"],
  ["{bpm}", "BPM"], ["{key}", "Tonart"], ["/", "Unterordner"],
];
const PRESETS: [string, string][] = [
  ["{albumartist}/{album}/{disc-}{track} - {title}", "Künstler / Album / 01 - Titel"],
  ["{genre}/{artist} - {title}", "Genre / Künstler - Titel"],
  ["{initial}/{albumartist}/{year} - {album}/{track} - {title}", "A / Künstler / 1999 - Album / 01 - Titel"],
  ["{artist}/{artist} - {title}", "Künstler / Künstler - Titel"],
];

export function organizeView(preIds?: number[]): { el: HTMLElement; destroy: () => void } {
  const s = store.settings;
  const target = h("input", { type: "text", value: s.organizeTarget ?? s.roots[0] ?? "", placeholder: "Zielordner", style: { width: "100%" } });
  const pattern = h("input", { type: "text", value: s.pattern || PRESETS[0][0], style: { width: "100%", fontFamily: "var(--mono)" } });
  const scopeSel = h("select", {},
    preIds?.length ? h("option", { value: "sel" }, `Ausgewählte Titel (${preIds.length})`) : null,
    h("option", { value: "all" }, "Ganze Bibliothek"),
  );
  const modeSel = h("select", {}, h("option", { value: "move" }, "Verschieben"), h("option", { value: "copy" }, "Kopieren"));
  const example = h("div", { class: "note", style: { fontFamily: "var(--mono)" } });
  const previewBox = h("div", {});
  const hist = historyList(["move"]);
  const history = hist.el;
  let plan: PlannedMove[] = [];

  const updExample = async () => {
    const id = preIds?.[0] ?? store.tracks.keys().next().value;
    if (id === undefined || !target.value) return (example.textContent = "");
    const p = await api.organizePreview([id], target.value, pattern.value).catch(() => []);
    example.textContent = p[0] ? "Beispiel: " + p[0].to : "";
  };
  pattern.addEventListener("input", updExample);
  target.addEventListener("change", updExample);

  const preview = async () => {
    if (!target.value.trim()) return toast("Zielordner wählen");
    await store.saveSettings({ organizeTarget: target.value.trim(), pattern: pattern.value });
    const ids = scopeSel.value === "sel" ? preIds! : null;
    plan = await api.organizePreview(ids, target.value.trim(), pattern.value);
    const changes = plan.filter((m) => !m.unchanged);
    const renamed = changes.filter((m) => m.renamed).length;
    const rel = (p: string) => (p.toLowerCase().startsWith(target.value.trim().toLowerCase()) ? "…" + p.slice(target.value.trim().length) : p);
    previewBox.replaceChildren(
      h("div", { class: "line" },
        h("span", { class: "sub" }, `${changes.length} Dateien werden ${modeSel.value === "copy" ? "kopiert" : "verschoben"} · ${plan.length - changes.length} schon richtig${renamed ? ` · ${renamed} mit (2)… wegen gleichem Namen` : ""}`),
        h("span", { class: "spacer" }),
        changes.length ? holdButton(modeSel.value === "copy" ? "Kopieren" : "Verschieben", apply) : null,
      ),
      h("table", { class: "grid" },
        h("thead", {}, h("tr", {}, h("th", {}, "Von"), h("th", {}, "Nach"))),
        h("tbody", {}, ...changes.slice(0, 300).map((m) => h("tr", {}, h("td", { class: "mono" }, m.from), h("td", { class: "mono" }, rel(m.to) + (m.renamed ? "  (umbenannt)" : ""))))),
      ),
      changes.length > 300 ? h("div", { class: "note" }, `… und ${changes.length - 300} weitere`) : "",
    );
  };

  const apply = async () => {
    const changes = plan.filter((m) => !m.unchanged);
    if (modeSel.value === "move") player.release(changes.map((m) => m.trackId));
    const r = await api.organizeApply(plan, modeSel.value === "copy", `${modeSel.value === "copy" ? "Kopiert" : "Verschoben"}: ${changes.length} Dateien nach ${pattern.value}`);
    reportErrors(r.errors, `${r.moved} Dateien ${modeSel.value === "copy" ? "kopiert" : "verschoben"}`);
    // Zielordner als Musikordner aufnehmen, damit spätere Scans die Dateien finden
    const t = target.value.trim();
    if (!store.settings.roots.some((r) => t.toLowerCase().startsWith(r.toLowerCase()))) {
      await store.saveSettings({ roots: [...store.settings.roots, t] });
      toast("Zielordner wurde als Musikordner hinzugefügt");
    }
    await store.loadTracks();
    previewBox.replaceChildren();
    loadHistory();
  };

  const loadHistory = () => hist.reload();
  loadHistory();
  updExample();

  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Sortieren"), h("span", { class: "sub" }, "Dateien nach Tags in Windows-Ordner einsortieren")),
    h("div", { class: "view-scroll" },
      h("div", { class: "card" }, h("h3", {}, "Ziel"),
        h("div", { class: "line" }, h("div", { class: "grow" }, target), h("button", { class: "btn", onclick: async () => { const d = await open({ directory: true }); if (d) { target.value = d as string; updExample(); } } }, "Wählen …")),
      ),
      h("div", { class: "card" }, h("h3", {}, "Muster"),
        pattern,
        h("div", { class: "chips", style: { margin: "10px 0" } },
          ...PLACEHOLDERS.map(([p, l]) => h("span", { class: "chip", title: l, onclick: () => { insertAt(pattern, p); updExample(); } }, p)),
        ),
        h("div", { class: "chips", style: { marginBottom: "8px" } },
          ...PRESETS.map(([p, l]) => h("span", { class: "chip", onclick: () => { pattern.value = p; updExample(); } }, l)),
        ),
        example,
      ),
      h("div", { class: "card" }, h("h3", {}, "Ausführen"),
        h("div", { class: "line" }, scopeSel, modeSel, h("button", { class: "btn primary", onclick: preview }, "Vorschau")),
        h("div", { class: "note" }, "Nichts passiert ohne Vorschau. Leere Ordner werden aufgeräumt, Playlisten bleiben gültig, jeder Vorgang kann rückgängig gemacht werden."),
        previewBox,
      ),
      h("div", { class: "card" }, h("h3", {}, "Verlauf"), history),
    ),
  );
  return { el, destroy: () => {} };
}

function insertAt(inp: HTMLInputElement, text: string) {
  const s = inp.selectionStart ?? inp.value.length;
  const e = inp.selectionEnd ?? s;
  inp.value = inp.value.slice(0, s) + text + inp.value.slice(e);
  inp.focus();
  inp.setSelectionRange(s + text.length, s + text.length);
}

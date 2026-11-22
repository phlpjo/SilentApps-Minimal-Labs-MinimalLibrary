import { open } from "@tauri-apps/plugin-dialog";
import { api, type PlannedMove } from "../api";
import { store } from "../store";
import { h, holdButton, reportErrors, toast } from "../ui";
import { historyList } from "./history";
import { L } from "../i18n";

const PLACEHOLDERS: [string, string][] = [
  ["{albumartist}", L("Albumkünstler")], ["{artist}", L("Künstler")], ["{album}", "Album"], ["{title}", L("Titel")], ["{track}", L("Track (01)")],
  ["{disc-}", L("Disc „2-“ nur wenn > 1")], ["{disc}", "Disc"], ["{year}", L("Jahr")], ["{genre}", "Genre"], ["{initial}", L("Anfangsbuchstabe")],
  ["{bpm}", "BPM"], ["{key}", L("Tonart")], ["/", L("Unterordner")],
];
const PRESETS: [string, string][] = [
  [L("{albumartist}/{album}/{disc-}{track} - {title}"), L("Künstler / Album / 01 - Titel")],
  [L("{genre}/{artist} - {title}"), L("Genre / Künstler - Titel")],
  [L("{initial}/{albumartist}/{year} - {album}/{track} - {title}"), L("A / Künstler / 1999 - Album / 01 - Titel")],
  [L("{artist}/{artist} - {title}"), L("Künstler / Künstler - Titel")],
];

export function organizeView(preIds?: number[]): { el: HTMLElement; destroy: () => void } {
  const s = store.settings;
  const target = h("input", { type: "text", value: s.organizeTarget ?? s.roots[0] ?? "", placeholder: L("Zielordner"), style: { width: "100%" } });
  const pattern = h("input", { type: "text", value: s.pattern || PRESETS[0][0], style: { width: "100%", fontFamily: "var(--mono)" } });
  const scopeSel = h("select", {},
    preIds?.length ? h("option", { value: "sel" }, L("Ausgewählte Titel ({0})", preIds.length)) : null,
    h("option", { value: "all" }, L("Ganze Bibliothek")),
  );
  const modeSel = h("select", {}, h("option", { value: "move" }, L("Verschieben")), h("option", { value: "copy" }, L("Kopieren")));
  const example = h("div", { class: "note", style: { fontFamily: "var(--mono)" } });
  const previewBox = h("div", {});
  const hist = historyList(["move"]);
  const history = hist.el;
  let plan: PlannedMove[] = [];

  const updExample = async () => {
    const id = preIds?.[0] ?? store.tracks.keys().next().value;
    if (id === undefined || !target.value) return (example.textContent = "");
    const p = await api.organizePreview([id], target.value, pattern.value).catch(() => []);
    example.textContent = p[0] ? L("Beispiel: ") + p[0].to : "";
  };
  pattern.addEventListener("input", updExample);
  target.addEventListener("change", updExample);

  const preview = async () => {
    if (!target.value.trim()) return toast(L("Zielordner wählen"));
    await store.saveSettings({ organizeTarget: target.value.trim(), pattern: pattern.value });
    const ids = scopeSel.value === "sel" ? preIds! : null;
    plan = await api.organizePreview(ids, target.value.trim(), pattern.value);
    const changes = plan.filter((m) => !m.unchanged);
    const renamed = changes.filter((m) => m.renamed).length;
    const rel = (p: string) => (p.toLowerCase().startsWith(target.value.trim().toLowerCase()) ? "…" + p.slice(target.value.trim().length) : p);
    previewBox.replaceChildren(
      h("div", { class: "line" },
        h("span", { class: "sub" }, L("{0} Dateien werden {1} · {2} schon richtig{3}", changes.length, modeSel.value === "copy" ? L("kopiert") : L("verschoben"), plan.length - changes.length, renamed ? L(" · {0} mit (2)… wegen gleichem Namen", renamed) : "")),
        h("span", { class: "spacer" }),
        changes.length ? holdButton(modeSel.value === "copy" ? L("Kopieren") : L("Verschieben"), apply) : null,
      ),
      h("table", { class: "grid" },
        h("thead", {}, h("tr", {}, h("th", {}, L("Von")), h("th", {}, L("Nach")))),
        h("tbody", {}, ...changes.slice(0, 300).map((m) => h("tr", {}, h("td", { class: "mono" }, m.from), h("td", { class: "mono" }, rel(m.to) + (m.renamed ? "  (umbenannt)" : ""))))),
      ),
      changes.length > 300 ? h("div", { class: "note" }, L("… und {0} weitere", changes.length - 300)) : "",
    );
  };

  const apply = async () => {
    const changes = plan.filter((m) => !m.unchanged);
    const r = await api.organizeApply(plan, modeSel.value === "copy", L("{0}: {1} Dateien nach {2}", modeSel.value === "copy" ? L("Kopiert") : L("Verschoben"), changes.length, pattern.value));
    reportErrors(r.errors, L("{0} Dateien {1}", r.moved, modeSel.value === "copy" ? L("kopiert") : L("verschoben")));
    // Zielordner als Musikordner aufnehmen, damit spätere Scans die Dateien finden
    const t = target.value.trim();
    if (!store.settings.roots.some((r) => t.toLowerCase().startsWith(r.toLowerCase()))) {
      await store.saveSettings({ roots: [...store.settings.roots, t] });
      toast(L("Zielordner wurde als Musikordner hinzugefügt"));
    }
    await store.loadTracks();
    previewBox.replaceChildren();
    loadHistory();
  };

  const loadHistory = () => hist.reload();
  loadHistory();
  updExample();

  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, L("Sortieren")), h("span", { class: "sub" }, L("Dateien nach Tags in Ordner einsortieren"))),
    h("div", { class: "view-scroll" },
      h("div", { class: "card" }, h("h3", {}, L("Ziel")),
        h("div", { class: "line" }, h("div", { class: "grow" }, target), h("button", { class: "btn", onclick: async () => { const d = await open({ directory: true }); if (d) { target.value = d as string; updExample(); } } }, L("Wählen …"))),
      ),
      h("div", { class: "card" }, h("h3", {}, L("Muster")),
        pattern,
        h("div", { class: "chips", style: { margin: "10px 0" } },
          ...PLACEHOLDERS.map(([p, l]) => h("span", { class: "chip", title: l, onclick: () => { insertAt(pattern, p); updExample(); } }, p)),
        ),
        h("div", { class: "chips", style: { marginBottom: "8px" } },
          ...PRESETS.map(([p, l]) => h("span", { class: "chip", onclick: () => { pattern.value = p; updExample(); } }, l)),
        ),
        example,
      ),
      h("div", { class: "card" }, h("h3", {}, L("Ausführen")),
        h("div", { class: "line" }, scopeSel, modeSel, h("button", { class: "btn primary", onclick: preview }, L("Vorschau"))),
        h("div", { class: "note" }, L("Nichts passiert ohne Vorschau. Leere Ordner werden aufgeräumt, Playlisten bleiben gültig, jeder Vorgang kann rückgängig gemacht werden.")),
        previewBox,
      ),
      h("div", { class: "card" }, h("h3", {}, L("Verlauf")), history),
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

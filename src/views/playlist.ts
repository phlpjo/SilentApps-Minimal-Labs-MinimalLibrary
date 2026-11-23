import { open, save } from "@tauri-apps/plugin-dialog";
import { api, type Layout, type Playlist, type Rule, type SmartRules } from "../api";
import { player } from "../player";
import { store } from "../store";
import { TrackList } from "../tracklist";
import { clear, fmtDuration, fmtSize, h, holdButton, reportErrors, sheet, toast } from "../ui";
import { nav } from "../main";
import { trackMenu } from "./library";
import { pioneerExportDialog, pioneerOn } from "../pioneer";
import { requireLicense } from "../license";
import { L, pl, shown } from "../i18n";

export function playlistView(id: number): { el: HTMLElement; destroy: () => void } {
  const pl = () => store.playlists.find((p) => p.id === id);
  let items: number[] = [];
  const list = new TrackList({
    reorderable: true,
    defaultSort: "pos",
    emptyText: L("Titel aus der Bibliothek hierher oder auf den Playlist-Namen ziehen."),
    menu: (ids) =>
      pl()?.rules
        ? trackMenu(ids)
        : trackMenu(ids, [{ label: L("Aus Playlist entfernen"), hint: L("Entf"), action: () => removeItems(ids) }]),
    onReorder: async (ids) => {
      if (pl()?.rules) return;
      items = ids;
      await api.playlistSetItems(id, ids);
      store.loadPlaylists();
    },
    onDelete: (ids) => {
      if (!pl()?.rules) removeItems(ids);
    },
  });
  const title = h("h1", {});
  const sub = h("span", { class: "sub" });
  const actions = h("div", { style: { display: "flex", gap: "4px", alignItems: "center" } });

  const removeItems = async (ids: number[]) => {
    items = items.filter((i) => !ids.includes(i));
    await api.playlistSetItems(id, items);
    store.selection.clear();
    list.setIds(items);
    store.loadPlaylists();
  };

  const renderHead = () => {
    const p = pl();
    if (!p) return;
    title.textContent = shown(p.name);
    sub.textContent = L("{0}{1} Titel · {2}", p.rules ? L("Smarte Playlist · ") : "", p.count, fmtDuration(p.duration));
    clear(actions);
    actions.append(
      h("button", { class: "btn", onclick: () => player.play(items[0], 0, list.visibleIds), disabled: !items.length }, L("Abspielen")),
      p.rules
        ? h("button", { class: "btn", onclick: () => smartEditor(p) }, L("Regeln bearbeiten …"))
        : h("button", { class: "btn", onclick: () => renamePlaylist(p) }, L("Umbenennen …")),
      h("button", { class: "btn", onclick: () => saveM3u(p) }, L("Als M3U8 speichern …")),
      h("button", { class: "btn primary", onclick: () => exportDialog([p.id]) }, L("Exportieren …")),
      pioneerOn() ? h("button", { class: "btn", title: L("Export für Pioneer DJ / AlphaTheta-Player (OneLibrary)"), onclick: () => pioneerExportDialog([p.id]) }, L("Für Pioneer-Player …")) : "",
      holdButton(L("Löschen"), async () => {
        await api.playlistDelete(p.id);
        await store.loadPlaylists();
        nav("library");
      }, L("halten")),
    );
  };

  const load = async () => {
    items = await api.playlistItems(id);
    list.setIds(items);
    player.queue = list.visibleIds;
    renderHead();
  };
  const unsubs = [
    store.on("playlists", () => {
      renderHead();
      if (pl()?.rules) load();
    }),
    store.on("tracks", () => list.setIds(items)),
  ];
  load();
  const el = h("div", { style: { display: "contents" } }, h("div", { class: "head" }, title, sub, h("div", { class: "spacer" }), actions), list.el);
  return { el, destroy: () => { unsubs.forEach((u) => u()); list.destroy(); } };
}

export function renamePlaylist(p: Playlist) {
  const name = h("input", { type: "text", value: p.name, style: { width: "100%" } });
  const go = async () => {
    if (!name.value.trim()) return;
    await api.playlistUpdate(p.id, name.value.trim(), null);
    await store.loadPlaylists();
    s.close();
  };
  name.addEventListener("keydown", (e) => e.key === "Enter" && go());
  const s = sheet(L("Umbenennen"), h("div", {}, name, h("div", { class: "foot" }, h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")), h("button", { class: "btn primary", onclick: go }, L("Speichern")))));
  setTimeout(() => name.select());
}

async function saveM3u(p: Playlist) {
  const file = await save({ defaultPath: `${p.name}.m3u8`, filters: [{ name: "Playlist", extensions: ["m3u8"] }] });
  if (!file) return;
  try {
    await api.playlistSaveM3u8(p.id, file);
    toast(L("Gespeichert: ") + file);
  } catch (e) {
    toast(String(e));
  }
}

// ---------- Smarte Playlist ----------

const FIELDS: [string, string, "text" | "num" | "date"][] = [
  ["genre", "Genre", "text"], ["artist", L("Künstler"), "text"], ["albumartist", L("Albumkünstler"), "text"], ["album", "Album", "text"],
  ["title", L("Titel"), "text"], ["year", L("Jahr"), "num"], ["bpm", "BPM", "num"], ["key", L("Tonart"), "text"], ["duration", L("Dauer (Sek. oder m:ss)"), "num"],
  ["bitrate", L("Bitrate"), "num"], ["realkbps", L("Echte Qualität (kbps)"), "num"], ["quality", L("Qualität (z. B. hochgerechnet)"), "text"], ["comment", L("Kommentar"), "text"], ["path", L("Pfad"), "text"], ["added", L("Hinzugefügt"), "date"],
];
const OPS: Record<string, [string, string][]> = {
  text: [["contains", L("enthält")], ["notContains", L("enthält nicht")], ["is", L("ist")], ["isNot", L("ist nicht")], ["startsWith", L("beginnt mit")]],
  num: [["between", L("zwischen")], ["gt", L("größer als")], ["lt", L("kleiner als")]],
  date: [["inLastDays", L("in den letzten … Tagen")]],
};

/** Regelzeilen (Feld · Bedingung · Wert) – für smarte Playlisten und die erweiterte Suche */
export function renderRuleRows(box: HTMLElement, list: Rule[], onChange: () => void) {
  const render = () => {
    clear(box);
    list.forEach((r: Rule, i) => {
      const kind = FIELDS.find((f) => f[0] === r.field)?.[2] ?? "text";
      if (!OPS[kind].some(([o]) => o === r.op)) r.op = OPS[kind][0][0];
      const f = h("select", {}, ...FIELDS.map(([v, l]) => h("option", { value: v, selected: v === r.field }, l)));
      const o = h("select", {}, ...OPS[kind].map(([v, l]) => h("option", { value: v, selected: v === r.op }, l)));
      const v1 = h("input", { type: "text", value: r.value, placeholder: kind === "num" ? L("von") : L("Wert") });
      const v2 = h("input", { type: "text", value: r.value2, placeholder: L("bis"), style: { visibility: r.op === "between" ? "visible" : "hidden" } });
      f.addEventListener("change", () => { r.field = f.value; render(); onChange(); });
      o.addEventListener("change", () => { r.op = o.value; render(); onChange(); });
      v1.addEventListener("input", () => { r.value = v1.value; onChange(); });
      v2.addEventListener("input", () => { r.value2 = v2.value; onChange(); });
      box.appendChild(h("div", { class: "rule" }, f, o, v1, v2, h("button", { class: "btn", onclick: () => { list.splice(i, 1); render(); onChange(); } }, L("Entfernen"))));
    });
  };
  render();
}

export function smartEditor(p?: Playlist) {
  if (!requireLicense("smart")) return;
  const rules: SmartRules = p?.rules ? structuredClone(p.rules) : { all: true, rules: [{ field: "genre", op: "contains", value: "", value2: "" }], sort: "artist", desc: false, limit: null };
  const name = h("input", { type: "text", value: p?.name ?? "Neue smarte Playlist", style: { width: "100%" } });
  const allSel = h("select", {}, h("option", { value: "1", selected: rules.all }, L("alle Regeln")), h("option", { value: "0", selected: !rules.all }, L("mindestens eine Regel")));
  const box = h("div", {});
  const count = h("span", { class: "sub" });
  const sortSel = h("select", {}, ...[["artist", L("Künstler")], ["title", L("Titel")], ["album", "Album"], ["bpm", "BPM"], ["key", L("Tonart")], ["added", L("Hinzugefügt")], ["duration", L("Dauer")], ["random", L("Zufall")]].map(([v, l]) => h("option", { value: v, selected: rules.sort === v }, l)));
  const descBox = h("input", { type: "checkbox", checked: rules.desc });
  const limit = h("input", { type: "number", min: "0", placeholder: "∞", value: rules.limit ? String(rules.limit) : "", style: { width: "80px" } });
  let timer = 0;
  const preview = () => {
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      const ids = await api.smartPreview(collect());
      count.textContent = L("{0} Titel passen", ids.length);
    }, 200);
  };
  const collect = (): SmartRules => ({
    all: allSel.value === "1",
    rules: rules.rules,
    sort: sortSel.value,
    desc: descBox.checked,
    limit: limit.value ? Number(limit.value) : null,
  });
  const renderRules = () => renderRuleRows(box, rules.rules, preview);
  [allSel, sortSel, descBox, limit].forEach((e) => e.addEventListener("change", preview));
  renderRules();
  preview();
  const s = sheet(
    p ? L("Smarte Playlist bearbeiten") : L("Neue smarte Playlist"),
    h("div", {},
      h("div", { class: "field" }, h("label", {}, L("Name")), name),
      h("div", { class: "line" }, L("Titel, die"), allSel, L("erfüllen:")),
      box,
      h("button", { class: "btn", onclick: () => { rules.rules.push({ field: "genre", op: "contains", value: "", value2: "" }); renderRules(); } }, L("+ Regel")),
      h("div", { class: "line", style: { marginTop: "14px" } }, L("Sortieren nach"), sortSel, h("label", { class: "check" }, descBox, "absteigend"), h("span", { class: "spacer" }), L("höchstens"), limit, L("Titel")),
      h("div", { class: "foot" }, count, h("span", { class: "spacer" }),
        h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")),
        h("button", {
          class: "btn primary",
          onclick: async () => {
            const r = collect();
            if (p) await api.playlistUpdate(p.id, name.value.trim() || p.name, r);
            else {
              await api.playlistCreate(name.value.trim() || L("Smarte Playlist"), r, null);
              toast(L("Smarte Playlist „{0}“ angelegt", name.value.trim() || L("Smarte Playlist")));
            }
            await store.loadPlaylists();
            s.close();
          },
        }, L("Speichern")),
      ),
    ),
    { wide: true },
  );
}

// ---------- Export ----------

export async function exportDialog(preselect: number[]) {
  if (!requireLicense("export")) return;
  const checks = new Map<number, HTMLInputElement>();
  const size = h("span", { class: "sub" });
  const dest = h("input", { type: "text", value: localStorage.getItem("ml.exportDest") ?? "", placeholder: L("Zielordner (z. B. USB-Stick oder Handy)"), style: { width: "100%" } });
  let layout: Layout = (localStorage.getItem("ml.exportLayout") as Layout) ?? "numbered";
  const upd = async () => {
    const ids = [...checks].filter(([, c]) => c.checked).map(([i]) => i);
    size.textContent = ids.length ? L("{0} Playlist{1} · {2}", ids.length, pl(ids.length, "en"), fmtSize(await api.exportSize(ids))) : "";
  };
  const plBox = h("div", { style: { maxHeight: "220px", overflowY: "auto" } },
    ...store.playlists.map((p) => {
      const c = h("input", { type: "checkbox", checked: preselect.includes(p.id) });
      c.addEventListener("change", upd);
      checks.set(p.id, c);
      return h("div", {}, h("label", { class: "check", style: { padding: "4px 0" } }, c, shown(p.name), h("span", { class: "sub" }, L(" · {0} Titel", p.count))));
    }),
  );
  const radio = (v: Layout, label: string, note: string) => {
    const r = h("input", { type: "radio", name: "layout", checked: layout === v });
    r.addEventListener("change", () => (layout = v));
    return h("div", { style: { margin: "6px 0" } }, h("label", { class: "check" }, r, label), h("div", { class: "note", style: { marginLeft: "23px" } }, note));
  };
  upd();
  const s = sheet(
    L("Playlisten exportieren"),
    h("div", {},
      h("div", { class: "note" }, L("Kopiert die Original-Musikdateien und eine passende M3U8-Playlist in den Zielordner – fertig für Minimal Player auf dem Handy.")),
      h("div", { class: "card", style: { marginTop: "12px" } }, h("h3", {}, L("Playlisten")), plBox),
      h("div", { class: "card" }, h("h3", {}, L("Ziel")),
        h("div", { class: "line" }, h("div", { class: "grow" }, dest), h("button", { class: "btn", onclick: async () => { const d = await open({ directory: true }); if (d) dest.value = d as string; } }, L("Wählen …"))),
      ),
      h("div", { class: "card" }, h("h3", {}, L("Aufbau")),
        radio("numbered", L("Ein Ordner pro Playlist, nummeriert"), L("Ziel/Playlist/001 - Künstler - Titel.mp3 + Playlist.m3u8 – Reihenfolge bleibt auch ohne Playlist erhalten.")),
        radio("flat", L("Ein Ordner pro Playlist"), L("Ziel/Playlist/Künstler - Titel.mp3 + Playlist.m3u8")),
        radio("structured", L("Gemeinsamer Musikordner"), L("Ziel/Musik/Künstler/Album/… und alle .m3u8 in Ziel – jede Datei nur einmal, auch wenn sie in mehreren Playlisten ist.")),
      ),
      h("div", { class: "foot" }, size, h("span", { class: "spacer" }),
        h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")),
        h("button", {
          class: "btn primary",
          onclick: async () => {
            const ids = [...checks].filter(([, c]) => c.checked).map(([i]) => i);
            if (!ids.length || !dest.value.trim()) return toast(L("Playlist und Zielordner wählen"));
            localStorage.setItem("ml.exportDest", dest.value);
            localStorage.setItem("ml.exportLayout", layout);
            s.close();
            try {
              const r = await api.exportPlaylists(ids, dest.value.trim(), layout);
              reportErrors(r.errors, L("Export fertig: {0} kopiert, {1} schon vorhanden ({2})", r.copied, r.skipped, fmtSize(r.bytes)));
            } catch (e) {
              toast(String(e));
            }
          },
        }, L("Exportieren")),
      ),
    ),
    { wide: true },
  );
}

// Browser-Ersatz für @tauri-apps/plugin-dialog: Ordner und Dateien auf dem NAS auswählen.
import { invoke } from "./core";
import { h, sheet } from "../ui";
import { L } from "../i18n";

interface Entry { name: string; path: string; dir: boolean }
interface Listing { path: string; parent: string | null; entries: Entry[] }
interface OpenOpts { directory?: boolean; multiple?: boolean; defaultPath?: string; title?: string; filters?: { name: string; extensions: string[] }[] }
interface SaveOpts { defaultPath?: string; title?: string; filters?: { name: string; extensions: string[] }[] }

function pick(opts: { dirs: boolean; start?: string; title: string; exts?: string[]; saveName?: string }): Promise<string | null> {
  return new Promise((resolve) => {
    let done = false;
    let cur = "";
    const list = h("div", { class: "nas-list" });
    const where = h("div", { class: "note nas-where" });
    const name = h("input", { type: "text", value: opts.saveName ?? "", style: { width: "100%", marginTop: "8px", display: opts.saveName != null ? "" : "none" } }) as HTMLInputElement;
    const finish = (v: string | null) => {
      if (done) return;
      done = true;
      s.close();
      resolve(v);
    };
    const load = async (p: string) => {
      try {
        const r = await invoke<Listing>("server_browse", { path: p, files: !opts.dirs });
        cur = r.path;
        where.textContent = r.path || L("Freigaben auf dem NAS");
        const rows: HTMLElement[] = [];
        if (r.path) rows.push(h("div", { class: "nas-row up", onclick: () => load(r.parent ?? "") }, "‹ " + L("Zurück")));
        for (const e of r.entries) {
          if (!e.dir && opts.exts?.length && !opts.exts.some((x) => e.name.toLowerCase().endsWith("." + x.toLowerCase()))) continue;
          rows.push(h("div", { class: "nas-row" + (e.dir ? " dir" : ""), onclick: () => (e.dir ? load(e.path) : finish(e.path)) }, (e.dir ? "📁 " : "♪ ") + e.name));
        }
        if (!r.entries.length) rows.push(h("div", { class: "note" }, L("Leer")));
        list.replaceChildren(...rows);
      } catch (e) {
        list.replaceChildren(h("div", { class: "err" }, String(e)));
      }
    };
    const s = sheet(opts.title, h("div", {}, where, list, name,
      h("div", { class: "foot" },
        h("button", { class: "btn", onclick: () => finish(null) }, L("Abbrechen")),
        opts.dirs || opts.saveName != null
          ? h("button", { class: "btn primary", onclick: () => {
            if (!cur) return;
            if (opts.saveName != null) finish(name.value.trim() ? cur.replace(/\/+$/, "") + "/" + name.value.trim() : null);
            else finish(cur);
          } }, opts.saveName != null ? L("Speichern") : L("Diesen Ordner wählen"))
          : "")), { onClose: () => finish(null) });
    const start = opts.start && opts.start.startsWith("/") ? opts.start : "";
    load(start).then(() => { if (start && !cur) load(""); });
  });
}

export async function open(o: OpenOpts = {}): Promise<string | string[] | null> {
  const p = await pick({
    dirs: !!o.directory,
    start: o.defaultPath,
    title: o.title ?? (o.directory ? L("Ordner auf dem NAS wählen") : L("Datei auf dem NAS wählen")),
    exts: o.filters?.flatMap((f) => f.extensions),
  });
  if (p == null) return null;
  return o.multiple ? [p] : p;
}

export async function save(o: SaveOpts = {}): Promise<string | null> {
  const base = (o.defaultPath ?? "").split(/[\\/]/).pop() ?? "";
  return pick({ dirs: true, title: o.title ?? L("Speichern unter (NAS)"), saveName: base });
}

export async function message(text: string) {
  alert(text);
}
export async function ask(text: string) {
  return confirm(text);
}
export async function confirm2(text: string) {
  return confirm(text);
}

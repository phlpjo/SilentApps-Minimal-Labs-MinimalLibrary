import type { Track } from "./api";
import { player } from "./player";
import { store } from "./store";
import { drawWave, waveElement } from "./waveform";
import { camelot, fmtTime, h, keyLabel, keyOrder, ext, type MenuItem, contextMenu } from "./ui";
import { renameFileDialog } from "./fileops";
import { beginDrag, dropTarget } from "./dnd";

export type SortKey =
  | "pos" | "trackNo" | "title" | "artist" | "album" | "albumartist" | "genre" | "year" | "bpm" | "key" | "duration"
  | "bitrate" | "format" | "samplerate" | "size" | "added" | "comment" | "filename" | "path";
type ColKey = SortKey | "wave";

interface Col { key: ColKey; label: string; width: string; render: (t: Track, pos: number) => Node | string; num?: boolean; hideBelow?: number }

const fmtSizeShort = (b: number) => (b >= 1e9 ? (b / 1e9).toFixed(1) + " GB" : (b / 1e6).toFixed(1) + " MB").replace(".", ",");
const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/** Alle wählbaren Spalten (Reihenfolge = Reihenfolge im Menü) */
const COLUMNS: Col[] = [
  { key: "pos", label: "#", width: "34px", num: true, render: (_t, p) => String(p + 1) },
  { key: "trackNo", label: "Nr.", width: "44px", num: true, render: (t) => (t.trackNo ? (t.discNo && t.discNo > 1 ? `${t.discNo}-` : "") + t.trackNo : "") },
  {
    key: "title", label: "Titel", width: "minmax(150px, 2fr)",
    render: (t) => h("div", { style: { minWidth: "0" } }, h("div", { class: "t1" }, store.label(t)), h("div", { class: "t2" }, t.artist ?? "Unbekannter Künstler")),
  },
  { key: "artist", label: "Künstler", width: "minmax(100px, 1fr)", render: (t) => t.artist ?? "" },
  { key: "album", label: "Album", width: "minmax(100px, 1.2fr)", hideBelow: 700, render: (t) => t.album ?? "" },
  { key: "albumartist", label: "Albumkünstler", width: "minmax(100px, 1fr)", render: (t) => t.albumArtist ?? "" },
  { key: "genre", label: "Genre", width: "minmax(70px, 0.7fr)", hideBelow: 860, render: (t) => t.genre ?? "" },
  { key: "year", label: "Jahr", width: "52px", num: true, render: (t) => t.year?.slice(0, 4) ?? "" },
  { key: "bpm", label: "BPM", width: "52px", num: true, render: (t) => (t.bpm ? t.bpm.toFixed(t.bpm % 1 ? 1 : 0) : "") },
  { key: "key", label: "Tonart", width: "74px", render: (t) => h("span", { title: keyLabel(t.key) }, t.key ? `${camelot(t.key) || ""} ${t.key}`.trim() : "") },
  { key: "duration", label: "Zeit", width: "50px", num: true, render: (t) => fmtTime(t.duration) },
  { key: "bitrate", label: "Bitrate", width: "62px", num: true, render: (t) => (t.bitrate ? String(t.bitrate) : "") },
  { key: "format", label: "Format", width: "58px", render: (t) => ext(t.path) },
  { key: "samplerate", label: "kHz", width: "52px", num: true, render: (t) => (t.sampleRate ? (t.sampleRate / 1000).toLocaleString("de") : "") },
  { key: "size", label: "Größe", width: "70px", num: true, render: (t) => fmtSizeShort(t.size) },
  { key: "added", label: "Hinzugefügt", width: "92px", render: (t) => (t.added ? new Date(t.added * 1000).toLocaleDateString("de") : "") },
  { key: "comment", label: "Kommentar", width: "minmax(80px, 1fr)", render: (t) => t.comment ?? "" },
  { key: "filename", label: "Dateiname", width: "minmax(120px, 1.4fr)", render: (t) => fileName(t.path) },
  { key: "path", label: "Pfad", width: "minmax(160px, 2fr)", render: (t) => h("span", { style: { fontFamily: "var(--mono)", fontSize: "11px" } }, t.path) },
  { key: "wave", label: "Wellenform", width: "minmax(140px, 2.4fr)", render: () => "" },
];

const DEFAULTS: Record<string, ColKey[]> = {
  library: ["title", "album", "genre", "bpm", "key", "duration", "wave"],
  folders: ["trackNo", "title", "album", "bpm", "key", "duration", "wave"],
  playlist: ["pos", "title", "album", "genre", "bpm", "key", "duration", "wave"],
};

interface ColConfig { visible: ColKey[]; widths: Partial<Record<ColKey, number>> }

const ROW_H = 48;

export class TrackList {
  el: HTMLElement;
  private head: HTMLElement;
  private body: HTMLElement;
  private spacer: HTMLElement;
  private ids: number[] = [];
  private view: number[] = [];
  sort: SortKey;
  desc = false;
  private anchor = -1;
  private rows = new Map<number, { row: HTMLElement; canvas: HTMLCanvasElement; idx: number }>();
  private cols: Col[] = [];
  private config: ColConfig;
  private storageKey: string;
  private unsub: (() => void)[] = [];
  private dropIndex = -1;

  constructor(
    private opts: {
      reorderable?: boolean;
      defaultSort?: SortKey;
      menu?: (ids: number[]) => MenuItem[];
      onReorder?: (ids: number[]) => void;
      onDelete?: (ids: number[]) => void;
      onRename?: (id: number) => void;
      emptyText?: Node | string;
      /** Spalten-Einstellungen merken unter diesem Namen (library, folders, playlist) */
      columns?: string;
    } = {},
  ) {
    this.sort = opts.defaultSort ?? "pos";
    this.storageKey = opts.columns ?? (opts.reorderable ? "playlist" : "library");
    this.config = this.loadConfig();
    this.head = h("div", { class: "list-head" });
    this.head.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      this.columnMenu(e.clientX, e.clientY);
    });
    this.spacer = h("div", { style: { height: "0px" } });
    this.body = h("div", { class: "list-body", tabindex: "0" }, this.spacer);
    this.el = h("div", { class: "list" }, this.head, this.body);
    this.updateCols();
    this.body.addEventListener("scroll", () => this.renderRows());
    this.body.addEventListener("keydown", (e) => this.onKey(e));
    new ResizeObserver(() => {
      this.updateCols();
      this.renderRows(true);
    }).observe(this.body);
    this.unsub.push(store.on("peaks", () => this.redrawWaves()));
    this.unsub.push(store.on("player", () => this.renderRows(true)));
    this.unsub.push(player.onPos((id, f) => {
      const r = this.rows.get(id);
      if (r) drawWave(r.canvas, store.peaksFor(id), f);
    }));
    const theme = () => this.renderRows(true);
    document.addEventListener("themechange", theme);
    this.unsub.push(() => document.removeEventListener("themechange", theme));
    // Umsortieren in Playlisten (nur bei Sortierung nach "#")
    dropTarget(this.body, {
      accepts: (p) => p.kind === "ids" && !!this.opts.reorderable && this.sort === "pos" && !this.desc,
      over: (_p, e) => {
        const r = this.body.getBoundingClientRect();
        this.setDrop(Math.max(0, Math.min(this.view.length, Math.round((e.clientY - r.top + this.body.scrollTop) / ROW_H))));
      },
      leave: () => this.setDrop(-1),
      drop: (p) => p.kind === "ids" && this.dropAt(p.ids),
    });
  }

  private loadConfig(): ColConfig {
    try {
      const c = JSON.parse(localStorage.getItem("ml.cols." + this.storageKey) ?? "null") as ColConfig | null;
      if (c && Array.isArray(c.visible) && c.visible.length) return { visible: c.visible.filter((k) => COLUMNS.some((x) => x.key === k)), widths: c.widths ?? {} };
    } catch {
      /* Standard */
    }
    return { visible: [...(DEFAULTS[this.storageKey] ?? DEFAULTS.library)], widths: {} };
  }

  private saveConfig() {
    localStorage.setItem("ml.cols." + this.storageKey, JSON.stringify(this.config));
  }

  private widthOf(c: Col): string {
    if (c.key === "wave") return "minmax(120px, 1fr)";
    const w = this.config.widths[c.key];
    return w ? `${w}px` : c.width;
  }

  private template() {
    return this.cols.map((c) => this.widthOf(c)).join(" ");
  }

  /** Sichtbare Spalten bestimmen (schmale Ansicht: Album/Genre ohne feste Breite ausblenden) */
  private updateCols(force = false) {
    const w = this.body.clientWidth || 1200;
    const cols = this.config.visible
      .map((k) => COLUMNS.find((c) => c.key === k)!)
      .filter((c) => c && (c.key !== "pos" || this.opts.reorderable))
      .filter((c) => !c.hideBelow || this.config.widths[c.key] || w >= c.hideBelow);
    if (!force && cols.map((c) => c.key).join() === this.cols.map((c) => c.key).join()) return;
    this.cols = cols;
    this.head.style.gridTemplateColumns = this.template();
    this.renderHead();
    this.renderRows(true);
  }

  private columnMenu(x: number, y: number) {
    const items: MenuItem[] = COLUMNS.filter((c) => c.key !== "pos" || this.opts.reorderable).map((c) => {
      const on = this.config.visible.includes(c.key);
      return {
        label: (on ? "✓\u2002" : "\u2003\u2002") + c.label,
        disabled: c.key === "title",
        action: () => {
          if (on) this.config.visible = this.config.visible.filter((k) => k !== c.key);
          else {
            // an der Stelle aus der Standard-Reihenfolge einfügen, Wellenform bleibt hinten
            const order = COLUMNS.map((x) => x.key);
            const next = [...this.config.visible, c.key].sort((a, b) => (a === "wave" ? 1 : b === "wave" ? -1 : order.indexOf(a) - order.indexOf(b)));
            this.config.visible = next;
          }
          this.saveConfig();
          this.updateCols(true);
        },
      };
    });
    items.push({ label: "", sep: true }, {
      label: "Spaltenbreiten zurücksetzen",
      action: () => { this.config.widths = {}; this.saveConfig(); this.updateCols(true); },
    }, {
      label: "Standardspalten",
      action: () => { this.config = { visible: [...(DEFAULTS[this.storageKey] ?? DEFAULTS.library)], widths: {} }; this.saveConfig(); this.updateCols(true); },
    });
    contextMenu(x, y, items);
  }

  /** Breite ziehen am rechten Rand einer Kopfzelle */
  private startResize(e: PointerEvent, c: Col, cell: HTMLElement) {
    e.preventDefault();
    e.stopPropagation();
    const start = e.clientX;
    const w0 = cell.getBoundingClientRect().width;
    // Spalten ohne feste Breite links davon auf ihre aktuelle Breite festschreiben, damit nichts springt
    const move = (ev: PointerEvent) => {
      this.config.widths[c.key] = Math.max(36, Math.round(w0 + ev.clientX - start));
      const tpl = this.template();
      this.head.style.gridTemplateColumns = tpl;
      for (const [, r] of this.rows) r.row.style.gridTemplateColumns = tpl;
    };
    const up = () => {
      window.removeEventListener("pointermove", move, true);
      window.removeEventListener("pointerup", up, true);
      document.body.classList.remove("is-resizing");
      this.saveConfig();
    };
    document.body.classList.add("is-resizing");
    window.addEventListener("pointermove", move, true);
    window.addEventListener("pointerup", up, true);
  }

  destroy() {
    this.unsub.forEach((u) => u());
  }

  /** Anzuzeigende IDs (Reihenfolge = "#") */
  setIds(ids: number[]) {
    this.ids = ids;
    this.applySort();
  }

  get visibleIds() {
    return this.view;
  }

  private sortVal(t: Track, k: SortKey): string | number {
    const s = (v: string | null) => (v ?? "￿").toLowerCase();
    switch (k) {
      case "title": return s(store.label(t));
      case "artist": return s(t.artist) + "\u0000" + s(t.album) + "\u0000" + String(t.discNo ?? 0).padStart(3, "0") + String(t.trackNo ?? 0).padStart(4, "0");
      case "album": return s(t.album) + "\u0000" + String(t.discNo ?? 0).padStart(3, "0") + String(t.trackNo ?? 0).padStart(4, "0");
      case "genre": return s(t.genre);
      case "bpm": return t.bpm ?? 9999;
      case "key": return keyOrder(t.key);
      case "duration": return t.duration;
      case "added": return t.added;
      case "year": return s(t.year);
      case "trackNo": return (t.discNo ?? 1) * 10000 + (t.trackNo ?? 9999);
      case "albumartist": return s(t.albumArtist ?? t.artist);
      case "bitrate": return t.bitrate ?? 0;
      case "format": return ext(t.path);
      case "samplerate": return t.sampleRate ?? 0;
      case "size": return t.size;
      case "comment": return s(t.comment);
      case "filename": return fileName(t.path).toLowerCase();
      case "path": return t.path.toLowerCase();
      default: return 0;
    }
  }

  private applySort() {
    const ids = this.ids.filter((id) => store.tracks.has(id));
    if (this.sort === "pos") {
      this.view = this.desc ? ids.reverse() : ids;
    } else {
      const k = this.sort;
      const vals = new Map(ids.map((id) => [id, this.sortVal(store.tracks.get(id)!, k)]));
      ids.sort((a, b) => {
        const x = vals.get(a)!, y = vals.get(b)!;
        const r = typeof x === "number" && typeof y === "number" ? x - y : String(x).localeCompare(String(y), "de");
        return this.desc ? -r : r;
      });
      this.view = ids;
    }
    if (this.opts.emptyText && this.view.length === 0) {
      this.body.querySelector(".empty")?.remove();
      this.body.appendChild(h("div", { class: "empty" }, this.opts.emptyText));
    } else {
      this.body.querySelector(".empty")?.remove();
    }
    this.renderHead();
    this.renderRows(true);
  }

  private renderHead() {
    const artistCol = this.cols.some((c) => c.key === "artist");
    // Ohne eigene Künstler-Spalte schaltet die Titel-Spalte zwischen Titel und Künstler um
    const titleShowsArtist = !artistCol && this.sort === "artist";
    this.head.replaceChildren(
      ...this.cols.map((c) => {
        const sorted = this.sort === c.key || (c.key === "title" && titleShowsArtist);
        const cell = h(
          "div",
          {
            class: "hcell" + (sorted ? " sorted" : "") + (c.num ? " num" : ""),
            onclick: () => {
              if (c.key === "wave") return;
              let key = c.key as SortKey;
              if (c.key === "title" && !artistCol) {
                if (this.sort === "title" && !this.desc) key = "artist";
                else if (this.sort === "artist") key = "title";
              }
              if (this.sort === key && !(c.key === "title" && !artistCol && key === "artist")) this.desc = !this.desc;
              else this.desc = false;
              this.sort = key;
              this.applySort();
            },
          },
          h("span", { class: "hlabel" }, (c.key === "title" && titleShowsArtist ? "Künstler" : c.label) + (sorted ? (this.desc ? " ↓" : " ↑") : "")),
        );
        if (c.key !== "wave") {
          const rz = h("div", { class: "rz" });
          rz.addEventListener("pointerdown", (e) => this.startResize(e, c, cell));
          rz.addEventListener("click", (e) => e.stopPropagation());
          rz.addEventListener("dblclick", (e) => {
            e.stopPropagation();
            delete this.config.widths[c.key];
            this.saveConfig();
            this.updateCols(true);
          });
          cell.appendChild(rz);
        }
        return cell;
      }),
    );
  }

  private renderRows(force = false) {
    const top = this.body.scrollTop;
    const hgt = this.body.clientHeight || 600;
    const first = Math.max(0, Math.floor(top / ROW_H) - 4);
    const last = Math.min(this.view.length, Math.ceil((top + hgt) / ROW_H) + 4);
    this.spacer.style.height = this.view.length * ROW_H + "px";
    const want = new Set<number>();
    for (let i = first; i < last; i++) want.add(this.view[i]);
    for (const [id, r] of this.rows) {
      if (force || !want.has(id) || this.view[r.idx] !== id) {
        r.row.remove();
        this.rows.delete(id);
      }
    }
    const tpl = this.template();
    for (let i = first; i < last; i++) {
      const id = this.view[i];
      if (this.rows.has(id)) continue;
      const t = store.tracks.get(id)!;
      const isCur = player.current?.id === id;
      const { wrap, canvas } = waveElement({
        duration: () => t.duration,
        onSeek: (f) => player.play(id, f, this.view),
      });
      if (!t.analyzed) wrap.appendChild(h("div", { class: "pending" }, "WIRD ANALYSIERT …"));
      else if (t.analysisError) wrap.appendChild(h("div", { class: "pending", title: t.analysisError }, `KEINE WELLENFORM (${ext(t.path)})`));
      const cells = this.cols.map((c) =>
        c.key === "wave" ? wrap : h("div", { class: "cell" + (c.num ? " num" : "") }, c.render(t, this.opts.reorderable ? this.ids.indexOf(id) : i)),
      );
      const row = h(
        "div",
        {
          class: "row" + (store.selection.has(id) ? " sel" : "") + (isCur ? " playing" : ""),
          style: { top: i * ROW_H + "px", gridTemplateColumns: tpl },
          title: t.path,
        },
        ...cells,
      );
      row.addEventListener("pointerdown", (e) => this.onRowDown(e, i));
      row.addEventListener("dblclick", () => player.play(id, 0, this.view));
      row.addEventListener("contextmenu", (e) => {
        e.preventDefault();
        if (!store.selection.has(id)) this.selectOnly(i);
        if (this.opts.menu) contextMenu(e.clientX, e.clientY, this.opts.menu(this.selectedIds()));
      });
      this.body.appendChild(row);
      this.rows.set(id, { row, canvas, idx: i });
      requestAnimationFrame(() => drawWave(canvas, store.peaksFor(id), isCur ? player.frac() : -1));
    }
  }

  private redrawWaves() {
    for (const [id, r] of this.rows) {
      const p = store.peaks.get(id);
      if (p) drawWave(r.canvas, p, player.current?.id === id ? player.frac() : -1);
    }
  }

  selectedIds(): number[] {
    return this.view.filter((id) => store.selection.has(id));
  }

  private selectOnly(i: number) {
    store.selection.clear();
    store.selection.add(this.view[i]);
    this.anchor = i;
    this.updateSel();
  }

  private onRowDown(e: PointerEvent, i: number) {
    if (e.button === 2 && store.selection.has(this.view[i])) return;
    if (e.button === 0) e.preventDefault(); // keine Textauswahl beim Ziehen
    this.body.focus({ preventScroll: true });
    const id = this.view[i];
    let reduceOnClick = false;
    if (e.shiftKey && this.anchor >= 0) {
      if (!e.ctrlKey) store.selection.clear();
      const [a, b] = [Math.min(this.anchor, i), Math.max(this.anchor, i)];
      for (let k = a; k <= b; k++) store.selection.add(this.view[k]);
    } else if (e.ctrlKey || e.metaKey) {
      if (store.selection.has(id)) store.selection.delete(id);
      else store.selection.add(id);
      this.anchor = i;
    } else if (!store.selection.has(id) || store.selection.size === 1) {
      store.selection.clear();
      store.selection.add(id);
      this.anchor = i;
    } else {
      // Klick in eine Mehrfachauswahl: erst beim Loslassen ohne Ziehen auf diesen reduzieren
      reduceOnClick = true;
    }
    this.updateSel();
    if (e.shiftKey || e.ctrlKey || e.metaKey) return;
    beginDrag(
      e,
      () => {
        const ids = this.selectedIds();
        return ids.length ? { kind: "ids", ids } : null;
      },
      (dragged) => {
        if (!dragged && reduceOnClick) {
          store.selection.clear();
          store.selection.add(id);
          this.anchor = i;
          this.updateSel();
        }
      },
    );
  }

  updateSel() {
    for (const [id, r] of this.rows) r.row.classList.toggle("sel", store.selection.has(id));
    store.emit("selection");
  }

  selectAll() {
    this.view.forEach((id) => store.selection.add(id));
    this.updateSel();
  }

  private scrollTo(i: number) {
    const top = i * ROW_H;
    if (top < this.body.scrollTop) this.body.scrollTop = top;
    else if (top + ROW_H > this.body.scrollTop + this.body.clientHeight) this.body.scrollTop = top + ROW_H - this.body.clientHeight;
  }

  private onKey(e: KeyboardEvent) {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const cur = this.anchor >= 0 ? this.anchor : -1;
      const n = Math.max(0, Math.min(this.view.length - 1, cur + (e.key === "ArrowDown" ? 1 : -1)));
      if (e.shiftKey) {
        store.selection.add(this.view[n]);
        this.anchor = n;
        this.updateSel();
      } else this.selectOnly(n);
      this.scrollTo(n);
    } else if (e.key === "Enter") {
      const s = this.selectedIds();
      if (s.length) player.play(s[0], 0, this.view);
    } else if (e.key === "a" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      this.selectAll();
    } else if (e.key === "F2") {
      const s = this.selectedIds();
      if (s.length === 1) (this.opts.onRename ?? renameFileDialog)(s[0]);
    } else if (e.key === "Delete" && this.opts.onDelete) {
      const s = this.selectedIds();
      if (s.length) this.opts.onDelete(s);
    }
  }

  // ---------- Umsortieren per Drag & Drop (Playlist) ----------
  private setDrop(i: number) {
    if (this.dropIndex === i) return;
    this.dropIndex = i;
    for (const [, r] of this.rows) r.row.classList.toggle("drop-before", r.idx === i);
  }

  private dropAt(moved: number[]) {
    if (this.dropIndex < 0) return;
    const at = this.dropIndex;
    this.setDrop(-1);
    const before = this.view.slice(0, at).filter((id) => !moved.includes(id));
    const after = this.view.slice(at).filter((id) => !moved.includes(id));
    // Von außen (Bibliothek) kommende Titel werden eingefügt
    const next = [...before, ...moved, ...after];
    this.ids = next;
    this.applySort();
    this.opts.onReorder?.(next);
  }
}

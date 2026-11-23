import { lyricsPanel } from "./lyrics";
import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type Candidate, type TagUpdate, type Track } from "./api";
import { store } from "./store";
import { basename, clear, ext, fmtSize, fmtTime, h, holdButton, keyLabel, qualityText, reportErrors, sheet, toast } from "./ui";
import { SHOW_IN } from "./platform";
import { pioneerOn, pioneerPanel } from "./pioneer";
import { forgetThumb } from "./thumbs";
import { L, pl, LOCALE, trMsg } from "./i18n";

type FieldKey = keyof TagUpdate;
const FIELDS: { key: FieldKey; label: string; get: (t: Track) => string }[] = [
  { key: "title", label: L("Titel"), get: (t) => t.title ?? "" },
  { key: "artist", label: L("Künstler"), get: (t) => t.artist ?? "" },
  { key: "album", label: "Album", get: (t) => t.album ?? "" },
  { key: "albumArtist", label: L("Albumkünstler"), get: (t) => t.albumArtist ?? "" },
  { key: "genre", label: "Genre", get: (t) => t.genre ?? "" },
  { key: "year", label: L("Jahr"), get: (t) => t.year ?? "" },
  { key: "trackNo", label: "Track", get: (t) => (t.trackNo != null ? String(t.trackNo) : "") },
  { key: "discNo", label: "Disc", get: (t) => (t.discNo != null ? String(t.discNo) : "") },
  { key: "bpm", label: "BPM", get: (t) => (t.bpm != null ? String(Math.round(t.bpm)) : "") },
  { key: "key", label: L("Tonart"), get: (t) => t.key ?? "" },
  { key: "comment", label: L("Kommentar"), get: (t) => t.comment ?? "" },
];

export class Inspector {
  private inputs = new Map<FieldKey, HTMLInputElement>();
  private original = new Map<FieldKey, string | null>(); // null = verschieden
  private ids: number[] = [];
  private timer = 0;
  private pendingCoverRelease: string | null = null;

  constructor(private el: HTMLElement) {
    store.on("selection", () => this.schedule());
    store.on("tracks", () => this.schedule(true));
    document.addEventListener("keydown", (e) => {
      if (e.key === "s" && (e.ctrlKey || e.metaKey) && this.ids.length) {
        e.preventDefault();
        this.save();
      }
    });
  }

  private schedule(onlyIfChanged = false) {
    clearTimeout(this.timer);
    this.timer = window.setTimeout(() => {
      const ids = [...store.selection].filter((id) => store.tracks.has(id));
      // Ungespeicherte Eingaben nicht überschreiben, wenn nur Daten aktualisiert wurden
      if (onlyIfChanged && this.dirty() && ids.join() === this.ids.join()) return;
      this.render(ids);
    }, 60);
  }

  private dirty() {
    for (const [k, inp] of this.inputs) {
      const o = this.original.get(k);
      if (o === null ? inp.value !== "" : inp.value !== o) return true;
    }
    return false;
  }

  render(ids: number[]) {
    this.ids = ids;
    this.pendingCoverRelease = null;
    clear(this.el);
    this.inputs.clear();
    this.original.clear();
    const tracks = ids.map((id) => store.tracks.get(id)!).filter(Boolean);
    if (!tracks.length) {
      this.el.appendChild(h("div", { class: "empty", style: { padding: "40px 6px" } }, L("Titel auswählen, um Tags zu bearbeiten."), h("br"), h("br"), L("Mehrfachauswahl bearbeitet alle auf einmal.")));
      return;
    }
    const one = tracks.length === 1 ? tracks[0] : null;

    // Cover
    const cov = h("div", { class: "cover" }, tracks[0].hasCover ? "…" : L("Kein Cover"));
    if (tracks[0].hasCover) {
      api.getCover(tracks[0].id).then((url) => {
        if (this.ids !== ids) return;
        cov.replaceChildren(url ? h("img", { src: url }) : L("Kein Cover"));
      });
    }
    this.el.append(
      h("div", { class: "insp-title" }, one ? store.label(one) : L("{0} Titel", tracks.length)),
      cov,
      h("div", { class: "cover-acts" },
        h("button", { class: "btn", onclick: () => this.pickCover() }, L("Cover wählen …")),
        tracks.some((t) => t.hasCover) ? holdButton(L("Entfernen"), () => this.removeCover(), L("halten")) : null,
      ),
    );

    // Felder
    const mk = (f: (typeof FIELDS)[number]) => {
      const vals = new Set(tracks.map(f.get));
      const same = vals.size === 1;
      const v = same ? [...vals][0] : null;
      this.original.set(f.key, v);
      const inp = h("input", { type: "text", value: v ?? "", placeholder: same ? "" : L("‹verschieden›"), spellcheck: false });
      inp.addEventListener("input", () => {
        const o = this.original.get(f.key);
        inp.classList.toggle("changed", o === null ? inp.value !== "" : inp.value !== o);
      });
      inp.addEventListener("keydown", (e) => {
        if (e.key === "Enter") this.save();
        if (e.key === "Escape") {
          inp.value = this.original.get(f.key) ?? "";
          inp.classList.remove("changed");
        }
      });
      this.inputs.set(f.key, inp);
      return h("div", { class: "field" }, h("label", {}, f.label + (f.key === "key" && v ? `  ·  ${keyLabel(v)}` : "")), inp);
    };
    const byKey = (k: FieldKey) => FIELDS.find((f) => f.key === k)!;
    this.el.append(
      mk(byKey("title")), mk(byKey("artist")), mk(byKey("album")), mk(byKey("albumArtist")), mk(byKey("genre")),
      h("div", { class: "field-row" }, mk(byKey("year")), mk(byKey("trackNo"))),
      h("div", { class: "field-row" }, mk(byKey("discNo")), mk(byKey("bpm"))),
      mk(byKey("key")), mk(byKey("comment")),
      this.filenameBox(tracks),
      h("button", { class: "btn primary", style: { width: "100%", marginTop: "4px" }, onclick: () => this.save() }, L("Speichern  (Strg+S)")),
    );

    // Aktionen
    this.el.append(
      h("div", { class: "acts" },
        h("button", { class: "btn", onclick: () => this.onlineSearch(tracks[0]) }, L("Online suchen (MusicBrainz) …")),
        one ? h("button", { class: "btn", onclick: () => this.recognize(one), title: L("Erkennt den Titel am Audio-Fingerabdruck – auch ohne Tags") }, L("Am Klang erkennen (AcoustID) …")) : "",
        h("button", { class: "btn", onclick: () => this.fromFilename(tracks) }, L("Dateiname → Tags …")),
        tracks.some((t) => t.bpm || t.key) ? h("button", { class: "btn", onclick: () => this.analysisToTags() }, L("BPM + Tonart in Datei schreiben")) : null,
        one ? h("button", { class: "btn", onclick: () => revealItemInDir(one.path) }, SHOW_IN) : null,
      ),
    );

    // Songtext
    if (one) this.el.append(lyricsPanel(one.id));

    // Pioneer: Beatgrid & Cues (nur wenn eingeschaltet)
    if (one && pioneerOn()) this.el.append(pioneerPanel(one.id));

    // Infos
    if (one) {
      this.el.append(
        h("div", { class: "section", style: { padding: "18px 0 6px" } }, L("Datei")),
        h("div", { class: "path" }, one.path),
        kv("Format", `${ext(one.path)}${one.bitrate ? L(" · {0} kbit/s", one.bitrate) : ""}`),
        kv(L("Echte Qualität"), qualityText(one)),
        one.upscaled ? h("div", { class: "note" }, L("Die Höhen enden bei {0} kHz – typisch für {1} kbps. Manche Aufnahmen (alt, Vinyl, Lo-Fi) haben von Natur aus wenig Höhen.", ((one.cutoff ?? 0) / 1000).toLocaleString(LOCALE, { maximumFractionDigits: 1 }), one.realKbps)) : "",
        kv(L("Abtastrate"), one.sampleRate ? L("{0} kHz · {1}", (one.sampleRate / 1000).toLocaleString(LOCALE), one.channels === 1 ? L("Mono") : L("Stereo")) : "–"),
        kv(L("Dauer"), fmtTime(one.duration)),
        kv(L("Größe"), fmtSize(one.size)),
        kv(L("Analyse"), one.analyzed ? (one.analysisError ? L("Fehler") : L("fertig")) : L("ausstehend")),
        one.analysisError ? h("div", { class: "err" }, trMsg(one.analysisError)) : "",
      );
    } else {
      const total = tracks.reduce((s, t) => s + t.size, 0);
      const dur = tracks.reduce((s, t) => s + t.duration, 0);
      this.el.append(h("div", { class: "section", style: { padding: "18px 0 6px" } }, L("Auswahl")), kv(L("Dauer"), fmtTime(dur)), kv(L("Größe"), fmtSize(total)));
    }
  }

  // ---------- Dateiname nach Muster ----------
  private renameOn = localStorage.getItem("ml.renameOn") === "1";
  private namePattern = localStorage.getItem("ml.namePattern") ?? "{track} - {artist} - {title}";

  private filenameBox(tracks: Track[]): HTMLElement {
    const box = h("input", { type: "checkbox", checked: this.renameOn });
    const pat = h("input", { type: "text", value: this.namePattern, spellcheck: false, style: { fontFamily: "var(--mono)", fontSize: "12px" } });
    const prev = h("div", { class: "path", style: { margin: "6px 0 0" } });
    const chips = h("div", { class: "chips", style: { margin: "6px 0" } },
      ...["{artist}", "{title}", "{album}", "{albumartist}", "{track}", "{disc}", "{year}", "{genre}", "{bpm}", "{key}"].map((c) =>
        h("span", { class: "chip", onclick: () => { insertAtCursor(pat, c); update(); } }, c)),
    );
    const current = tracks.length === 1 ? basename(tracks[0].path) : L("{0} Dateien", tracks.length);
    const update = () => {
      this.namePattern = pat.value;
      localStorage.setItem("ml.namePattern", pat.value);
      const t = tracks[0];
      // Vorschau mit den (evtl. noch ungespeicherten) Feldwerten
      const val = (k: FieldKey, fallback: string | null) => {
        const inp = this.inputs.get(k);
        if (!inp) return fallback;
        if (inp.value === "" && this.original.get(k) === null) return fallback; // ‹verschieden›
        return inp.value || null;
      };
      const name = renderName(pat.value, {
        artist: val("artist", t.artist), title: val("title", t.title), album: val("album", t.album),
        albumartist: val("albumArtist", t.albumArtist), genre: val("genre", t.genre), year: val("year", t.year),
        track: val("trackNo", t.trackNo?.toString() ?? null), disc: val("discNo", t.discNo?.toString() ?? null),
        bpm: val("bpm", t.bpm ? String(Math.round(t.bpm)) : null), key: val("key", t.key),
      }, t.path);
      prev.textContent = box.checked ? `${current}  →  ${name}${tracks.length > 1 ? L("  (1. Datei, alle nach diesem Muster)") : ""}` : L("Aktuell: {0}", current);
      pat.style.opacity = box.checked ? "1" : "0.55";
    };
    box.addEventListener("change", () => {
      this.renameOn = box.checked;
      localStorage.setItem("ml.renameOn", box.checked ? "1" : "0");
      update();
    });
    pat.addEventListener("input", update);
    pat.addEventListener("keydown", (e) => e.key === "Enter" && this.save());
    for (const inp of this.inputs.values()) inp.addEventListener("input", update);
    setTimeout(update);
    return h("div", { class: "field", style: { marginTop: "6px" } },
      h("label", {}, L("Dateiname")),
      h("label", { class: "check", style: { fontSize: "13px", margin: "2px 0 6px" } }, box, L("Beim Speichern Datei umbenennen")),
      pat, chips, prev,
    );
  }

  private collect(): TagUpdate {
    const u: TagUpdate = {};
    for (const [k, inp] of this.inputs) {
      const o = this.original.get(k);
      const changed = o === null ? inp.value !== "" : inp.value !== o;
      if (changed) u[k] = inp.value;
    }
    return u;
  }

  async save() {
    const u = this.collect();
    const ids = this.ids;
    const release = this.pendingCoverRelease;
    const rename = this.renameOn && this.namePattern.trim() !== "";
    if (!Object.keys(u).length && !release && !rename) {
      toast(L("Keine Änderungen"));
      return;
    }
    if (Object.keys(u).length) {
      const errs = await api.writeTags(ids, u);
      reportErrors(errs, L("{0} Titel gespeichert", ids.length));
    }
    if (release) {
      const errs = await api.mbCover(ids, release).catch((e) => [String(e)]);
      reportErrors(errs, L("Cover übernommen"));
      forgetThumb(ids);
    }
    if (rename) {
      const r = await api.renameByPattern(ids, this.namePattern).catch((e) => ({ moved: 0, errors: [String(e)], batchId: 0 }));
      reportErrors(r.errors, r.moved ? L("{0} Datei{1} umbenannt", r.moved, pl(r.moved, "en")) : undefined);
      store.emit("dirs");
    }
    this.original.clear();
    await store.refreshTracks(ids);
    this.render(ids);
    store.loadPlaylists();
  }

  private async pickCover() {
    const file = await open({ multiple: false, filters: [{ name: L("Bilder"), extensions: ["jpg", "jpeg", "png"] }] });
    if (!file) return;
    reportErrors(await api.setCoverFile(this.ids, file as string), L("Cover gesetzt"));
    forgetThumb(this.ids);
    await store.refreshTracks(this.ids);
    this.render(this.ids);
  }

  private async removeCover() {
    reportErrors(await api.removeCover(this.ids), L("Cover entfernt"));
    forgetThumb(this.ids);
    await store.refreshTracks(this.ids);
    this.render(this.ids);
  }

  private async analysisToTags() {
    reportErrors(await api.writeAnalysisToTags(this.ids), L("BPM und Tonart in die Dateien geschrieben"));
    await store.refreshTracks(this.ids);
  }

  // ---------- Dateiname → Tags ----------
  private fromFilename(tracks: Track[]) {
    const pattern = h("input", { type: "text", value: localStorage.getItem("ml.fnpattern") ?? "{track} - {artist} - {title}", style: { width: "100%" } });
    const prev = h("tbody");
    const update = () => {
      clear(prev);
      for (const t of tracks.slice(0, 12)) {
        const r = parseFilename(basename(t.path).replace(/\.[^.]+$/, ""), pattern.value);
        prev.appendChild(h("tr", {}, h("td", { class: "mono" }, basename(t.path)), h("td", {}, r ? Object.entries(r).map(([k, v]) => `${k}: ${v}`).join(" · ") : L("passt nicht"))));
      }
    };
    pattern.addEventListener("input", update);
    update();
    const s = sheet(
      L("Dateiname → Tags"),
      h("div", {},
        h("div", { class: "note" }, L("Platzhalter: {artist} {title} {album} {track} {year} {genre} {*} (ignorieren). Alles andere ist Trenntext.")),
        h("div", { class: "line" }, pattern),
        h("table", { class: "grid" }, h("thead", {}, h("tr", {}, h("th", {}, L("Datei")), h("th", {}, L("Ergebnis")))), prev),
        h("div", { class: "foot" },
          h("button", { class: "btn", onclick: () => s.close() }, L("Abbrechen")),
          h("button", {
            class: "btn primary",
            onclick: async () => {
              localStorage.setItem("ml.fnpattern", pattern.value);
              const errs: string[] = [];
              let n = 0;
              for (const t of tracks) {
                const r = parseFilename(basename(t.path).replace(/\.[^.]+$/, ""), pattern.value);
                if (!r) continue;
                const u: TagUpdate = {};
                if (r.artist) u.artist = r.artist;
                if (r.title) u.title = r.title;
                if (r.album) u.album = r.album;
                if (r.track) u.trackNo = r.track;
                if (r.year) u.year = r.year;
                if (r.genre) u.genre = r.genre;
                errs.push(...(await api.writeTags([t.id], u)));
                n++;
              }
              s.close();
              reportErrors(errs, L("{0} Titel aktualisiert", n));
              await store.refreshTracks(tracks.map((t) => t.id));
            },
          }, L("Übernehmen")),
        ),
      ),
      { wide: true },
    );
  }

  // ---------- MusicBrainz ----------
  private async onlineSearch(t: Track) {
    const artist = h("input", { type: "text", value: t.artist ?? "", placeholder: L("Künstler") });
    const title = h("input", { type: "text", value: t.title ?? basename(t.path).replace(/\.[^.]+$/, ""), placeholder: L("Titel") });
    const withCover = h("input", { type: "checkbox", checked: true });
    const results = h("div", {}, h("div", { class: "note" }, L("Sucht in der freien MusicBrainz-Datenbank. Es werden nur Künstler und Titel übertragen.")));
    const run = async () => {
      results.replaceChildren(h("div", { class: "note" }, L("Suche …")));
      try {
        const list = await api.mbSearch(artist.value, title.value, null);
        results.replaceChildren(
          ...(list.length
            ? list.map((c) =>
                h("div", { class: "option", onclick: () => this.pickFields(c, withCover.checked, s, results, run) },
                  h("div", {}, h("div", { class: "t1" }, `${c.artist} – ${c.title}`), h("div", { class: "t2" }, [c.album, c.year, c.trackNo ? `Track ${c.trackNo}` : null, c.genre].filter(Boolean).join(" · "))),
                  h("small", {}, `${c.length ? fmtTime(c.length) + " · " : ""}${c.score} %`),
                ),
              )
            : [h("div", { class: "note" }, L("Nichts gefunden."))]),
        );
      } catch (e) {
        results.replaceChildren(h("div", { class: "err" }, trMsg(String(e))));
      }
    };
    [artist, title].forEach((i) => i.addEventListener("keydown", (e) => e.key === "Enter" && run()));
    const s = sheet(
      L("Online suchen"),
      h("div", {},
        h("div", { class: "line" }, h("div", { class: "grow" }, artist), h("div", { class: "grow" }, title), h("button", { class: "btn primary", onclick: run }, L("Suchen"))),
        h("div", { class: "line" }, h("label", { class: "check" }, withCover, L("Cover mit übernehmen (Cover Art Archive)"))),
        results,
        h("div", { class: "note", style: { marginTop: "12px" } }, L("Für {0} ausgewählte{1} Titel. Werte landen erst in den Feldern – Speichern schreibt sie in die Datei.", this.ids.length, this.ids.length === 1 ? "n" : "")),
      ),
      { wide: true },
    );
    run();
  }

  /** Titel am Klang erkennen (Fingerabdruck → AcoustID → MusicBrainz) */
  private async recognize(t: Track) {
    const results = h("div", {}, h("div", { class: "note" }, L("Erkenne …")));
    const withCover = true;
    const s = sheet(L("Am Klang erkennen"), h("div", {},
      h("div", { class: "note", style: { marginBottom: "10px" } }, L("{0} – der Audio-Fingerabdruck wird mit der freien AcoustID-Datenbank verglichen. Es werden nur Fingerabdruck und Dauer übertragen.", store.label(t))),
      results,
    ), { wide: true });
    const run = async () => {
      if (!t.analyzed || t.analysisError) {
        results.replaceChildren(h("div", { class: "err" }, L("Dieser Titel ist noch nicht analysiert – erst nach der Analyse gibt es einen Fingerabdruck.")));
        return;
      }
      if (!store.settings.acoustidKey) {
        results.replaceChildren(h("div", { class: "err" }, L("Kein AcoustID-Schlüssel eingetragen.")), h("div", { class: "note" }, L("Einstellungen → Online: kostenlosen Schlüssel eintragen (einmalig).")));
        return;
      }
      results.replaceChildren(h("div", { class: "note" }, L("Erkenne …")));
      try {
        const list = await api.acoustidLookup(t.id);
        if (!list.length) {
          results.replaceChildren(h("div", { class: "note" }, L("Nicht erkannt. Der Titel ist wahrscheinlich nicht in der AcoustID-Datenbank.")));
          return;
        }
        results.replaceChildren(
          ...list.map((c) =>
            h("div", { class: "option", onclick: () => this.pickFields(c, withCover, s, results, run) },
              h("div", {}, h("div", { class: "t1" }, `${c.artist} – ${c.title}`), h("div", { class: "t2" }, [c.album, c.year, c.trackNo ? `Track ${c.trackNo}` : null].filter(Boolean).join(" · "))),
              h("small", {}, `${c.length ? fmtTime(c.length) + " · " : ""}${c.score} %`),
            ),
          ),
        );
      } catch (e) {
        results.replaceChildren(h("div", { class: "err" }, trMsg(String(e))));
      }
    };
    run();
  }

  /** Treffer gewählt → pro Feld entscheiden, was übernommen wird */
  private pickFields(c: Candidate, withCover: boolean, s: { close: () => void }, results: HTMLElement, back: () => void) {
    const multi = this.ids.length > 1;
    const rows: { key: FieldKey; label: string; val: string; box: HTMLInputElement }[] = [];
    const add = (key: FieldKey, label: string, v: string | number | null | undefined) => {
      if (v === null || v === undefined || v === "") return;
      const inp = this.inputs.get(key);
      const cur = inp?.value ?? "";
      const val = String(v);
      const isVarious = this.original.get(key) === null && cur === "";
      // Vorauswahl: nur wo neu und anders; Titel/Track bei Mehrfachauswahl aus
      const pre = val !== cur && !(multi && (key === "title" || key === "trackNo"));
      rows.push({ key, label, val, box: h("input", { type: "checkbox", checked: pre }) });
      rows[rows.length - 1].box.dataset.cur = isVarious ? L("‹verschieden›") : cur;
    };
    add("title", L("Titel"), c.title);
    add("artist", L("Künstler"), c.artist);
    add("album", L("Album"), c.album);
    add("albumArtist", L("Albumkünstler"), c.albumArtist);
    add("year", L("Jahr"), c.year);
    add("trackNo", L("Track"), c.trackNo);
    add("discNo", L("Disc"), c.discNo);
    add("genre", L("Genre"), c.genre);
    const coverBox = h("input", { type: "checkbox", checked: withCover && !!c.releaseId });
    const all = h("input", { type: "checkbox", checked: false });
    all.addEventListener("change", () => rows.forEach((r) => (r.box.checked = all.checked)));
    results.replaceChildren(
      h("table", { class: "grid" },
        h("thead", {}, h("tr", {}, h("th", {}, all), h("th", {}, L("Feld")), h("th", {}, L("Aktuell")), h("th", {}, "MusicBrainz"))),
        h("tbody", {},
          ...rows.map((r) => {
            const tr = h("tr", { style: { cursor: "pointer" } },
              h("td", {}, r.box), h("td", {}, r.label),
              h("td", { style: { color: "var(--weak)" } }, r.box.dataset.cur || "–"),
              h("td", { style: { color: "var(--ink)" } }, r.val),
            );
            tr.addEventListener("click", (e) => { if (e.target !== r.box) r.box.checked = !r.box.checked; });
            return tr;
          }),
          c.releaseId ? h("tr", {}, h("td", {}, coverBox), h("td", {}, "Cover"), h("td", { style: { color: "var(--weak)" } }, "–"), h("td", {}, L("Front-Cover aus dem Cover Art Archive"))) : "",
        ),
      ),
      multi ? h("div", { class: "note", style: { marginTop: "8px" } }, L("Mehrfachauswahl: Titel und Track sind nicht vorausgewählt, damit nicht alle Dateien denselben Titel bekommen.")) : "",
      h("div", { class: "foot" },
        h("button", { class: "btn", onclick: back }, L("‹ Andere Treffer")),
        h("span", { class: "spacer" }),
        h("button", {
          class: "btn primary",
          onclick: () => {
            let n = 0;
            for (const r of rows) {
              if (!r.box.checked) continue;
              const inp = this.inputs.get(r.key);
              if (!inp) continue;
              inp.value = r.val;
              inp.dispatchEvent(new Event("input"));
              n++;
            }
            if (coverBox.checked && c.releaseId) this.pendingCoverRelease = c.releaseId;
            s.close();
            toast(n || coverBox.checked ? L("{0} Feld{1}{2} übernommen – mit Speichern in die Datei schreiben", n, pl(n, "er"), coverBox.checked ? L(" + Cover") : "") : L("Nichts übernommen"));
          },
        }, L("Auswahl übernehmen")),
      ),
    );
  }
}

function insertAtCursor(inp: HTMLInputElement, text: string) {
  const a = inp.selectionStart ?? inp.value.length;
  const b = inp.selectionEnd ?? a;
  inp.value = inp.value.slice(0, a) + text + inp.value.slice(b);
  inp.focus();
  inp.setSelectionRange(a + text.length, a + text.length);
}

const BAD = /[<>:"/\\|?*\u0000-\u001f]/g;
/** Vorschau – gleiche Regeln wie der Rust-Kern (organize::render) */
export function renderName(pattern: string, v: Record<string, string | null>, path: string): string {
  const stem = basename(path).replace(/\.[^.]+$/, "");
  const extn = (basename(path).match(/\.[^.]+$/) ?? [""])[0].toLowerCase();
  const pad = (n: string | null) => (n && /^\d+/.test(n) ? String(parseInt(n, 10)).padStart(2, "0") : "");
  const map: Record<string, string> = {
    artist: v.artist ?? L("Unbekannter Künstler"),
    albumartist: v.albumartist ?? v.artist ?? L("Unbekannter Künstler"),
    album: v.album ?? L("Unbekanntes Album"),
    title: v.title ?? stem,
    genre: v.genre ?? L("Unbekanntes Genre"),
    year: v.year ? v.year.slice(0, 4) : "0000",
    track: pad(v.track),
    disc: v.disc ?? "1",
    "disc-": v.disc && parseInt(v.disc, 10) > 1 ? `${parseInt(v.disc, 10)}-` : "",
    bpm: v.bpm ?? "",
    key: v.key ?? "",
  };
  let out = pattern.replace(/[\\/]/g, "-").replace(/\{([a-z-]+)\}/g, (m, k) => (k in map ? map[k].replace(/[\\/]/g, "_") : m));
  out = out.replace(BAD, "_").replace(/\s+/g, " ").trim().replace(/^[-\s]+/, "").replace(/[.\s]+$/, "");
  return (out || "_") + extn;
}

function kv(k: string, v: string) {
  return h("div", { class: "kv" }, h("span", {}, k), h("span", {}, v));
}

/** "{track} - {artist} - {title}" auf "01 - ABBA - Waterloo" anwenden */
export function parseFilename(name: string, pattern: string): Record<string, string> | null {
  const parts = pattern.split(/(\{[a-z*]+\})/).filter((p) => p !== "");
  let re = "^";
  const keys: string[] = [];
  for (const p of parts) {
    const m = p.match(/^\{([a-z*]+)\}$/);
    if (m) {
      keys.push(m[1]);
      re += m[1] === "track" || m[1] === "year" ? "(\\d+)" : "(.+?)";
    } else re += p.replace(/[.*+?^${}()|[\]\\]/g, "\\$&").replace(/\s+/g, "\\s*");
  }
  re += "$";
  const m = name.match(new RegExp(re));
  if (!m) return null;
  const out: Record<string, string> = {};
  keys.forEach((k, i) => {
    if (k !== "*") out[k] = m[i + 1].trim();
  });
  return out;
}

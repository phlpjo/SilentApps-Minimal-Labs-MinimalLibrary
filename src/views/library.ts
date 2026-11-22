import { editLyrics, fetchLyricsMany } from "../lyrics";
import { isStarred, toggleStar } from "../star";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { api, type Track } from "../api";
import { player } from "../player";
import { store } from "../store";
import { TrackList } from "../tracklist";
import { fmtDuration, searchNorm, h, holdButton, reportErrors, sheet, toast, type MenuItem } from "../ui";
import { analyzeIds, newPlaylistFromIds, nav } from "../main";
import { moveTracksPick, renameFileDialog } from "../fileops";
import { SHOW_IN, TRASH } from "../platform";

type Filter = "all" | "artist" | "title" | "album" | "genre" | "unanalyzed" | "notags" | "nocover" | "errors";

/** Gemeinsames Kontextmenü für Titel */
export function trackMenu(ids: number[], extra: MenuItem[] = []): MenuItem[] {
  const one = ids.length === 1 ? store.tracks.get(ids[0]) : undefined;
  const allStarred = ids.every((id) => isStarred(id));
  return [
    { label: "Abspielen", action: () => player.play(ids[0], 0, ids) },
    { label: allStarred ? "Stern entfernen" : "★ Stern setzen", action: () => toggleStar(ids) },
    {
      label: "Zu Playlist hinzufügen",
      sub: [
        { label: "Neue Playlist …", action: () => newPlaylistFromIds(ids) },
        ...(store.playlists.filter((p) => !p.rules).length ? [{ label: "", sep: true }] : []),
        ...store.playlists
          .filter((p) => !p.rules)
          .map((p) => ({
            label: p.name,
            action: async () => {
              await api.playlistAddItems(p.id, ids);
              await store.loadPlaylists();
              toast(`${ids.length} Titel → ${p.name}`);
            },
          })),
      ],
    },
    { label: "", sep: true },
    { label: "Neu analysieren", action: () => analyzeIds(ids) },
    ...(one ? [{ label: "Songtext …", action: () => editLyrics(one.id) }] : []),
    { label: "Songtexte holen (LRCLIB)", action: () => fetchLyricsMany(ids) },
    { label: "BPM + Tonart in Datei schreiben", action: async () => { reportErrors(await api.writeAnalysisToTags(ids), "Geschrieben"); store.refreshTracks(ids); } },
    { label: "In Ordner verschieben …", action: () => moveTracksPick(ids) },
    { label: "In Ordner kopieren …", action: () => moveTracksPick(ids, true) },
    ...(one ? [{ label: "Datei umbenennen …", hint: "F2", action: () => renameFileDialog(one.id) }] : []),
    { label: "Nach Muster sortieren …", action: () => nav("organize", ids) },
    ...(one ? [{ label: "Ordner in der Ordneransicht", action: () => nav("folders", undefined, one.path) }] : []),
    ...(one ? [{ label: SHOW_IN, action: () => revealItemInDir(one.path) }] : []),
    ...extra,
    { label: "", sep: true },
    {
      label: "Aus Bibliothek entfernen",
      hint: "Datei bleibt",
      action: async () => {
        await api.removeFromLibrary(ids);
        await store.loadTracks();
        store.loadPlaylists();
        toast(`${ids.length} Titel entfernt (Dateien unverändert)`);
      },
    },
    {
      label: "In den Papierkorb …",
      hint: "Datei",
      action: () => {
        const s = sheet(
          `${ids.length} Datei${ids.length === 1 ? "" : "en"} in den Papierkorb`,
          h("div", {},
            h("div", { class: "note" }, `Die Dateien landen im ${TRASH} und können von dort wiederhergestellt werden.`),
            h("div", { class: "foot" },
              h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
              holdButton("In den Papierkorb", async () => {
                s.close();
                player.release(ids);
                reportErrors(await api.trash(ids), `${ids.length} Datei${ids.length === 1 ? "" : "en"} im Papierkorb`);
                await store.loadTracks();
                store.loadPlaylists();
              }),
            ),
          ),
        );
      },
    },
  ];
}

/** Suchwörter gegen Tags, Tonart, BPM und Pfad (bzw. nur Dateiname) prüfen */
export function textMatches(t: Track, words: string[], fileNameOnly = false, field?: "artist" | "title" | "album" | "genre"): boolean {
  if (!words.length) return true;
  const where = fileNameOnly ? t.path.split(/[\\/]/).pop() : t.path;
  const parts =
    field === "artist" ? [t.artist, t.albumArtist]
    : field === "title" ? [t.title ?? store.label(t)]
    : field === "album" ? [t.album]
    : field === "genre" ? [t.genre]
    : [t.title, t.artist, t.album, t.albumArtist, t.genre, t.year, t.comment, where, t.key, t.bpm ? String(Math.round(t.bpm)) : ""];
  const hay = searchNorm(parts.join(" "));
  return words.every((w) => hay.includes(w));
}

/** Suchtext in Wörter zerlegen ("A&B" = "A & B") */
export const searchWords = (q: string) => searchNorm(q).split(/\s+/).filter(Boolean);

export function libraryView(): { el: HTMLElement; destroy: () => void } {
  let filter: Filter = (sessionStorage.getItem("ml.filter") as Filter) ?? "all";
  let query = sessionStorage.getItem("ml.query") ?? "";
  const list = new TrackList({ defaultSort: "artist", menu: (ids) => trackMenu(ids), emptyText: emptyText() });
  list.sort = (sessionStorage.getItem("ml.sort") as typeof list.sort) ?? "artist";
  const info = h("span", { class: "sub" });
  const search = h("input", { type: "search", placeholder: "Suchen …  (Strg+F)", value: query, style: { width: "260px" } });
  const sel = h("select", {},
    ...([["all", "Alle Titel"], ["artist", "Interpret"], ["title", "Titel"], ["album", "Album"], ["genre", "Genre"], ["unanalyzed", "Nicht analysiert"], ["notags", "Fehlende Tags"], ["nocover", "Ohne Cover"], ["errors", "Analysefehler"]] as const).map(
      ([v, l]) => h("option", { value: v, selected: v === filter }, l),
    ),
  );

  const matches = (t: Track, words: string[]) => {
    if (filter === "unanalyzed" && t.analyzed) return false;
    if (filter === "notags" && t.title && t.artist && t.album && t.genre) return false;
    if (filter === "nocover" && t.hasCover) return false;
    if (filter === "errors" && !t.analysisError) return false;
    const field = filter === "artist" || filter === "title" || filter === "album" || filter === "genre" ? filter : undefined;
    return textMatches(t, words, false, field);
  };
  const refresh = () => {
    const words = searchWords(query);
    const ids: number[] = [];
    let dur = 0;
    for (const t of store.tracks.values()) {
      if (matches(t, words)) {
        ids.push(t.id);
        dur += t.duration;
      }
    }
    list.setIds(ids);
    player.queue = list.visibleIds;
    const pending = [...store.tracks.values()].filter((t) => !t.analyzed).length;
    info.textContent = `${ids.length.toLocaleString("de")} Titel · ${fmtDuration(dur)}${pending ? ` · ${pending} nicht analysiert` : ""}`;
  };
  search.addEventListener("input", () => {
    query = search.value;
    sessionStorage.setItem("ml.query", query);
    refresh();
  });
  sel.addEventListener("change", () => {
    filter = sel.value as Filter;
    sessionStorage.setItem("ml.filter", filter);
    refresh();
  });
  const onKey = (e: KeyboardEvent) => {
    if (e.key === "f" && (e.ctrlKey || e.metaKey)) {
      e.preventDefault();
      search.focus();
      search.select();
    }
  };
  document.addEventListener("keydown", onKey);
  const unsub = store.on("tracks", refresh);
  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Bibliothek"), info, h("div", { class: "spacer" }), sel, search),
    list.el,
  );
  refresh();
  return {
    el,
    destroy: () => {
      sessionStorage.setItem("ml.sort", list.sort);
      unsub();
      list.destroy();
      document.removeEventListener("keydown", onKey);
    },
  };
}

function emptyText() {
  return h("div", {},
    h("b", {}, "Noch keine Musik"), h("br"),
    "Musikordner in den ", h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); nav("settings"); } }, "Einstellungen"),
    " hinzufügen – oder Dateien und Ordner einfach ins Fenster ziehen.",
  );
}

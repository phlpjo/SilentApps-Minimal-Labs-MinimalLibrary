import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { api, type RipEntry, type RipEvent } from "../api";
import { player } from "../player";
import { store } from "../store";
import { clear, fmtTime, h, reportErrors, toast } from "../ui";
import { analyzeIds, nav } from "../main";

// Bleibt beim Wechseln der Ansicht erhalten
let lastName = "";
let lastEntries: RipEntry[] = [];
const checked = new Set<number>();
const states = new Map<number, RipEvent>();
let running = false;
let urlText = "";

// Liste übersteht auch Neustarts der App
const KEY = "ml.rip";
function saveList() {
  try {
    const st = [...states.values()].filter((x) => x.stage === "done" || x.stage === "error");
    localStorage.setItem(KEY, JSON.stringify({ name: lastName, entries: lastEntries, checked: [...checked], states: st, url: urlText }));
  } catch { /* voll → egal */ }
}
try {
  const o = JSON.parse(localStorage.getItem(KEY) ?? "null");
  if (o) {
    lastName = o.name ?? "";
    lastEntries = o.entries ?? [];
    urlText = o.url ?? "";
    (o.checked ?? []).forEach((i: number) => checked.add(i));
    (o.states ?? []).forEach((x: RipEvent) => states.set(x.index, x));
  }
} catch { /* kaputt → leer */ }
/** Liste der offenen Ansicht neu zeichnen (null = Ripper gerade nicht geöffnet) */
let rerender: (() => void) | null = null;

// Fortschritt auch mitschreiben, während eine andere Ansicht offen ist
listen<RipEvent>("rip", (e) => {
  states.set(e.payload.index, e.payload);
  if (e.payload.stage === "done") checked.delete(e.payload.index);
  if (e.payload.stage === "done" || e.payload.stage === "error") saveList();
  rerender?.();
});

/** Läuft gerade ein Download? (für die Navigation) */
export const ripRunning = () => running;

export function ripperView(): { el: HTMLElement; destroy: () => void } {
  const url = h("textarea", { placeholder: "Link einfügen: YouTube, SoundCloud, Spotify – Titel, Album oder Playlist. Mehrere Links: einer pro Zeile.", rows: 2, spellcheck: false, style: { width: "100%", resize: "vertical" } }) as HTMLTextAreaElement;
  url.value = urlText;
  url.addEventListener("input", () => { urlText = url.value; saveList(); });
  const toolsBox = h("div", {});
  const listBox = h("div", {});
  const info = h("span", { class: "sub" });
  const s = store.settings;
  const defaultDir = s.ripDir || (s.roots[0] ? s.roots[0].replace(/[\\/]+$/, "") + (s.roots[0].includes("\\") ? "\\" : "/") + "Downloads" : "");
  const dir = h("input", { type: "text", value: defaultDir, placeholder: "Zielordner", style: { width: "100%" } });
  dir.addEventListener("change", () => store.saveSettings({ ripDir: dir.value.trim() || null }));
  const fmt = h("select", {}, h("option", { value: "mp3", selected: s.ripFormat !== "original" }, "MP3 320 kbit/s"), h("option", { value: "original", selected: s.ripFormat === "original" }, "Original (ohne Umwandlung)"));
  fmt.addEventListener("change", () => store.saveSettings({ ripFormat: fmt.value as "mp3" | "original" }));
  const subfolder = h("input", { type: "checkbox", checked: localStorage.getItem("ml.ripSub") !== "0" });
  subfolder.addEventListener("change", () => localStorage.setItem("ml.ripSub", subfolder.checked ? "1" : "0"));

  const renderTools = async () => {
    const t = await api.ripTools().catch(() => null);
    clear(toolsBox);
    if (t && t.ytdlp && t.ffmpeg) {
      // yt-dlp einmal am Tag still aktualisieren (YouTube ändert sich ständig)
      const last = Number(localStorage.getItem("ml.ytdlpUpdated") ?? "0");
      if (Date.now() - last > 86_400_000) {
        localStorage.setItem("ml.ytdlpUpdated", String(Date.now()));
        api.ripUpdate().catch(() => {});
      }
      toolsBox.append(
        h("div", { class: "line" },
          h("span", { class: "note" }, `yt-dlp ${t.ytdlp} · ffmpeg bereit`),
          h("button", { class: "btn", onclick: async () => { toast("Aktualisiere yt-dlp …"); toast(await api.ripUpdate().catch((e) => String(e))); renderTools(); } }, "yt-dlp aktualisieren"),
        ),
      );
    } else {
      toolsBox.append(
        h("div", { class: "note" }, "Für den Ripper braucht die App zwei freie Werkzeuge: yt-dlp (lädt die Audiospur) und ffmpeg (wandelt um). Einmalig ca. 90 MB."),
        h("div", { class: "line" }, h("button", {
          class: "btn primary",
          onclick: async () => {
            try {
              await api.ripInstall();
              toast("Werkzeuge installiert");
            } catch (e) {
              toast(String(e), 8000);
            }
            renderTools();
          },
        }, "Werkzeuge installieren")),
      );
    }
  };

  const renderList = () => {
    clear(listBox);
    if (!lastEntries.length) {
      listBox.append(h("div", { class: "empty" }, "Link einfügen und ", h("b", {}, "Laden"), " klicken. Titel, die schon in deiner Bibliothek sind, werden markiert und nicht angehakt."));
      info.textContent = "";
      return;
    }
    const known = lastEntries.filter((e) => e.inLibrary).length;
    info.textContent = `${lastName} · ${lastEntries.length} Titel${known ? ` · ${known} schon in der Bibliothek` : ""} · ${checked.size} ausgewählt`;
    const all = h("input", { type: "checkbox", checked: checked.size === lastEntries.length });
    all.addEventListener("change", () => {
      checked.clear();
      if (all.checked) lastEntries.forEach((e) => checked.add(e.index));
      saveList();
      renderList();
    });
    const rows = lastEntries.map((e) => {
      const box = h("input", { type: "checkbox", checked: checked.has(e.index), disabled: running });
      box.addEventListener("change", () => {
        if (box.checked) checked.add(e.index);
        else checked.delete(e.index);
        saveList();
        info.textContent = info.textContent!.replace(/\d+ ausgewählt$/, `${checked.size} ausgewählt`);
      });
      const st = states.get(e.index);
      const status = st
        ? st.stage === "done"
          ? h("span", { style: { color: "var(--ink)" } }, "✓ in der Bibliothek")
          : st.stage === "error"
            ? h("span", { class: "err", style: { margin: "0", display: "inline" }, title: st.message }, "Fehler: " + st.message.slice(0, 60))
            : h("span", {}, st.stage === "convert" ? "wandelt um …" : `${Math.round(st.percent)} %`)
        : e.inLibrary
          ? h("span", {
              class: "known", title: "Klick: in der Bibliothek anspielen",
              onclick: () => player.play(e.inLibrary![0], 0.3),
            }, "schon da: " + e.inLibrary[1])
          : "";
      return h("tr", { class: e.inLibrary && !st ? "is-known" : "" },
        h("td", {}, box),
        h("td", {}, h("div", { class: "t1" }, e.title), h("div", { class: "t2" }, [e.artist, e.album].filter(Boolean).join(" · "))),
        h("td", {}, e.site),
        h("td", {}, e.duration ? fmtTime(e.duration) : ""),
        h("td", {}, status),
      );
    });
    listBox.append(
      h("table", { class: "grid fixed rip" },
        h("colgroup", {}, h("col", { style: { width: "34px" } }), h("col", {}), h("col", { style: { width: "90px" } }), h("col", { style: { width: "56px" } }), h("col", { style: { width: "34%" } })),
        h("thead", {}, h("tr", {}, h("th", {}, all), h("th", {}, "Titel"), h("th", {}, "Quelle"), h("th", {}, "Zeit"), h("th", {}, "Status"))),
        h("tbody", {}, ...rows),
      ),
    );
  };

  const load = async () => {
    const links = url.value.split(/\s+/).map((x) => x.trim()).filter((x) => x.startsWith("http"));
    if (!links.length) return toast("Bitte einen Link einfügen");
    if (running) return toast("Erst den laufenden Download abwarten oder abbrechen");
    info.textContent = "lese Link …";
    const names: string[] = lastName ? [lastName] : [];
    const entries: RipEntry[] = [...lastEntries];
    let next = entries.reduce((m, e) => Math.max(m, e.index + 1), 0);
    const fresh: number[] = [];
    for (const l of links) {
      try {
        const r = await api.ripResolve(l);
        if (!names.includes(r.name)) names.push(r.name);
        // schon in der Liste (gleiche Quelle) → nicht doppelt
        for (const e of r.entries) {
          if (entries.some((x) => x.source === e.source)) continue;
          entries.push({ ...e, index: next });
          fresh.push(next++);
        }
        if (l.includes("spotify") && r.entries.length >= 100) toast("Spotify zeigt ohne Konto höchstens 100 Titel einer Playlist an.", 7000);
      } catch (e) {
        toast(`${l}\n${e}`, 8000);
      }
    }
    lastName = names.join(", ");
    lastEntries = entries;
    entries.filter((e) => fresh.includes(e.index) && !e.inLibrary).forEach((e) => checked.add(e.index));
    saveList();
    rerender?.();
  };

  const download = async () => {
    const list = lastEntries.filter((e) => checked.has(e.index));
    if (running) return toast("Download läuft schon");
    if (!list.length) return toast("Nichts ausgewählt");
    if (!dir.value.trim()) return toast("Zielordner wählen");
    let target = dir.value.trim();
    if (subfolder.checked && lastEntries.length > 1 && lastName) {
      const safe = lastName.replace(/[<>:"/\\|?*]/g, "_").slice(0, 80).trim();
      target += (target.includes("\\") ? "\\" : "/") + safe;
    }
    running = true;
    renderList();
    toast(`${list.length} Titel werden geladen – du kannst den Ripper verlassen, es läuft im Hintergrund weiter.`, 5000);
    try {
      const r = await api.ripDownload(list, target, fmt.value as "mp3" | "original");
      reportErrors(r.errors, `${r.ids.length} Titel heruntergeladen`);
      if (!store.settings.roots.some((root) => target.toLowerCase().startsWith(root.toLowerCase()))) {
        await store.saveSettings({ roots: [...store.settings.roots, dir.value.trim()] });
      }
      await store.loadTracks();
      if (r.ids.length) analyzeIds(r.ids);
    } catch (e) {
      toast(String(e));
    } finally {
      running = false;
      rerender?.();
    }
  };

  url.addEventListener("keydown", (e) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      load();
    }
  });
  // Link aus der Zwischenablage beim Einfügen direkt laden
  url.addEventListener("paste", () => setTimeout(() => url.value.trim().startsWith("http") && load(), 50));

  rerender = renderList;
  renderTools();
  renderList();
  setTimeout(() => url.focus());

  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Ripper"), info),
    h("div", { class: "view-scroll" },
      h("div", { class: "card" }, h("h3", {}, "Link"),
        url,
        h("div", { class: "line" },
          h("button", { class: "btn primary", onclick: load }, "Laden"),
          h("button", { class: "btn", onclick: () => {
            if (running) return toast("Download läuft noch");
            lastName = ""; lastEntries = []; checked.clear(); states.clear(); urlText = ""; url.value = "";
            saveList();
            renderList();
          } }, "Liste leeren"),
          h("span", { class: "spacer" }),
          h("span", { class: "note" }, "YouTube · YouTube Music · SoundCloud · Spotify · Bandcamp · Mixcloud u. v. m."),
        ),
      ),
      h("div", { class: "card" }, listBox),
      h("div", { class: "card" }, h("h3", {}, "Speichern"),
        h("div", { class: "line" }, h("div", { class: "grow" }, dir), h("button", { class: "btn", onclick: async () => { const d = await open({ directory: true }); if (d) { dir.value = d as string; store.saveSettings({ ripDir: dir.value }); } } }, "Wählen …")),
        h("div", { class: "line" }, fmt, h("label", { class: "check" }, subfolder, "Playlists in eigenen Unterordner"),
          h("span", { class: "spacer" }),
          h("button", { class: "btn", onclick: () => api.cancel() }, "Abbrechen"),
          h("button", { class: "btn primary", onclick: download }, "Ausgewählte herunterladen"),
        ),
        h("div", { class: "note" }, "Titel bekommen saubere Tags (Künstler, Titel, bei Spotify auch Album) und das Vorschaubild als Cover, werden in die Bibliothek aufgenommen und analysiert. Spotify-Titel werden über die YouTube-Suche geladen – bitte kurz reinhören, ob die richtige Version erwischt wurde. ",
          h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); nav("duplicates"); } }, "Duplikate prüfen"), "."),
      ),
      h("div", { class: "card" }, h("h3", {}, "Werkzeuge"), toolsBox,
        h("div", { class: "note", style: { marginTop: "8px" } }, "Lade nur Inhalte herunter, für die du die Rechte hast – z. B. eigene Uploads, freie Lizenzen oder Titel, die der Künstler zum Download anbietet."),
      ),
    ),
  );
  return { el, destroy: () => { if (rerender === renderList) rerender = null; } };
}

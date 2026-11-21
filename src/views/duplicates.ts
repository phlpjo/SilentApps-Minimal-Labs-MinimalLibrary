import { api, type DupGroup } from "../api";
import { player } from "../player";
import { store } from "../store";
import { drawWave, waveElement } from "../waveform";
import { clear, contextMenu, ext, fmtSize, fmtTime, h, holdButton, reportErrors, toast } from "../ui";
import { analyzeIds, nav } from "../main";
import { revealItemInDir } from "@tauri-apps/plugin-opener";

const REASON: Record<string, string> = { metadata: "Metadaten", filename: "Dateiname", audio: "Audio" };

// Ergebnis bleibt beim Wechseln der Ansicht erhalten
let lastGroups: DupGroup[] | null = null;
const keep = new Map<number, Set<number>>(); // Gruppenindex → behaltene IDs

export function duplicatesView(): { el: HTMLElement; destroy: () => void } {
  const modes = {
    metadata: h("input", { type: "checkbox", checked: localStorage.getItem("ml.dup.m") !== "0" }),
    filename: h("input", { type: "checkbox", checked: localStorage.getItem("ml.dup.f") !== "0" }),
    audio: h("input", { type: "checkbox", checked: localStorage.getItem("ml.dup.a") !== "0" }),
  };
  const out = h("div", {});
  const summary = h("span", { class: "sub" });
  const footer = h("div", { class: "line" });
  const hint = h("div", { class: "note" });
  const canvases: { id: number; c: HTMLCanvasElement }[] = [];

  const updateHint = () => {
    const pending = [...store.tracks.values()].filter((t) => !t.analyzed).length;
    clear(hint);
    if (modes.audio.checked && pending) {
      hint.append(`${pending} Titel sind noch nicht analysiert – für sie gibt es noch keinen Audio-Fingerabdruck. `,
        h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); analyzeIds(null); } }, "Jetzt analysieren"));
    }
  };

  const run = async () => {
    localStorage.setItem("ml.dup.m", modes.metadata.checked ? "1" : "0");
    localStorage.setItem("ml.dup.f", modes.filename.checked ? "1" : "0");
    localStorage.setItem("ml.dup.a", modes.audio.checked ? "1" : "0");
    out.replaceChildren(h("div", { class: "empty" }, "Suche Duplikate …"));
    try {
      lastGroups = await api.findDuplicates({ metadata: modes.metadata.checked, filename: modes.filename.checked, audio: modes.audio.checked });
      keep.clear();
      lastGroups.forEach((g, i) => keep.set(i, new Set([g.keep])));
      render();
    } catch (e) {
      out.replaceChildren(h("div", { class: "err" }, String(e)));
    }
  };

  const toRemove = () => {
    const ids: number[] = [];
    const replace: Record<string, number> = {};
    (lastGroups ?? []).forEach((g, i) => {
      const k = keep.get(i) ?? new Set();
      if (!k.size) return; // nie eine ganze Gruppe löschen
      const target = [...k][0];
      for (const id of g.trackIds) if (!k.has(id)) { ids.push(id); replace[String(id)] = target; }
    });
    return { ids, replace };
  };

  const renderFooter = () => {
    const { ids, replace } = toRemove();
    const bytes = ids.reduce((s, id) => s + (store.tracks.get(id)?.size ?? 0), 0);
    clear(footer);
    if (!lastGroups?.length) return;
    footer.append(
      h("span", { class: "sub" }, `${ids.length} Dateien markiert · ${fmtSize(bytes)}`),
      h("span", { class: "spacer" }),
      h("button", { class: "btn", disabled: !ids.length, onclick: async () => { await api.removeFromLibrary(ids); await store.loadTracks(); toast(`${ids.length} aus der Bibliothek entfernt`); run(); } }, "Nur aus Bibliothek entfernen"),
      ids.length
        ? holdButton("Markierte in den Papierkorb", async () => {
            player.release(ids);
            reportErrors(await api.trash(ids, replace), `${ids.length} Dateien im Papierkorb – Playlisten zeigen jetzt auf die behaltene Version`);
            await store.loadTracks();
            store.loadPlaylists();
            run();
          })
        : "",
    );
  };

  const render = () => {
    canvases.length = 0;
    const groups = (lastGroups ?? []).map((g, i) => [g, i] as const).filter(([g]) => g.trackIds.some((id) => store.tracks.has(id)));
    summary.textContent = lastGroups ? `${groups.length} Gruppen · ${groups.reduce((s, [g]) => s + g.trackIds.length - 1, 0)} mögliche Duplikate` : "";
    if (!lastGroups) {
      out.replaceChildren(h("div", { class: "empty" }, "Suchart wählen und ", h("b", {}, "Suchen"), " klicken."));
      return;
    }
    if (!groups.length) {
      out.replaceChildren(h("div", { class: "empty" }, "Keine Duplikate gefunden."));
      renderFooter();
      return;
    }
    out.replaceChildren(
      ...groups.slice(0, 400).map(([g, gi]) => {
        const k = keep.get(gi)!;
        const reasons = g.reasons.map((r) => (r === "audio" && g.audioSimilarity ? `Audio ${Math.round(g.audioSimilarity * 100)} %` : REASON[r] ?? r)).join(" · ");
        const rows = g.trackIds.filter((id) => store.tracks.has(id)).map((id) => {
          const t = store.tracks.get(id)!;
          const box = h("input", { type: "checkbox", checked: k.has(id), title: "Behalten" });
          const { wrap, canvas } = waveElement({ duration: () => t.duration, onSeek: (f) => player.play(id, f, g.trackIds) });
          canvases.push({ id, c: canvas });
          const tr = h("tr", { class: k.has(id) ? "keep" : "drop" },
            h("td", {}, box),
            h("td", {}, h("div", { class: "t1" }, `${t.artist ?? "?"} – ${store.label(t)}`), h("div", { class: "path", style: { margin: "3px 0 0" }, title: "Rechtsklick: Im Explorer zeigen" }, t.path)),
            h("td", {}, `${ext(t.path)}${t.bitrate ? " " + t.bitrate : ""}`),
            h("td", {}, fmtTime(t.duration)),
            h("td", {}, fmtSize(t.size)),
            h("td", {}, wrap),
          );
          box.addEventListener("change", () => {
            if (box.checked) k.add(id);
            else k.delete(id);
            tr.className = box.checked ? "keep" : "drop";
            renderFooter();
          });
          tr.addEventListener("dblclick", () => player.play(id, 0, g.trackIds));
          tr.addEventListener("contextmenu", (e) => {
            e.preventDefault();
            contextMenu(e.clientX, e.clientY, [
              { label: "Abspielen", action: () => player.play(id, 0, g.trackIds) },
              { label: "Im Explorer zeigen", action: () => revealItemInDir(t.path) },
              { label: "Ordner in der Ordneransicht", action: () => nav("folders", undefined, t.path) },
            ]);
          });
          return tr;
        });
        return h("div", { class: "group" },
          h("div", { class: "gh" }, h("span", {}, reasons), h("span", {}, "Haken = behalten")),
          h("table", { class: "grid fixed" },
            h("colgroup", {}, h("col", { style: { width: "34px" } }), h("col", {}), h("col", { style: { width: "90px" } }), h("col", { style: { width: "56px" } }), h("col", { style: { width: "76px" } }), h("col", { style: { width: "30%" } })),
            h("tbody", {}, ...rows)),
        );
      }),
      groups.length > 400 ? h("div", { class: "note" }, `… und ${groups.length - 400} weitere Gruppen. Erst diese bereinigen, dann erneut suchen.`) : "",
    );
    requestAnimationFrame(() => canvases.forEach(({ id, c }) => drawWave(c, store.peaksFor(id), player.current?.id === id ? player.frac() : -1)));
    renderFooter();
  };

  const unsubs = [
    store.on("peaks", () => canvases.forEach(({ id, c }) => store.peaks.has(id) && drawWave(c, store.peaks.get(id), player.current?.id === id ? player.frac() : -1))),
    player.onPos((id, f) => canvases.filter((x) => x.id === id).forEach(({ c }) => drawWave(c, store.peaksFor(id), f))),
    store.on("tracks", updateHint),
  ];
  Object.values(modes).forEach((m) => m.addEventListener("change", updateHint));
  updateHint();
  render();

  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Duplikate"), summary, h("div", { class: "spacer" }),
      h("label", { class: "check" }, modes.metadata, "Metadaten"),
      h("label", { class: "check" }, modes.filename, "Dateiname"),
      h("label", { class: "check" }, modes.audio, "Audio-Inhalt"),
      h("button", { class: "btn primary", onclick: run }, "Suchen"),
    ),
    h("div", { class: "view-scroll" },
      h("div", { class: "note", style: { marginBottom: "10px" } },
        "Metadaten: gleicher Künstler + Titel (ohne Zusätze wie „Remastered“). Dateiname: gleicher Name ohne Nummern und „(1)“/„Kopie“. ",
        "Audio-Inhalt: akustischer Fingerabdruck – findet dieselbe Aufnahme auch in anderem Format oder mit falschen Tags. Vorschlag zum Behalten: verlustfrei > höhere Bitrate > vollständigere Tags.",
      ),
      hint, out,
    ),
    h("div", { style: { padding: "8px 20px", borderTop: "1px solid var(--line-soft)" } }, footer),
  );
  return { el, destroy: () => unsubs.forEach((u) => u()) };
}

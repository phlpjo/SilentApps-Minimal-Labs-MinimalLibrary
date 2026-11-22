import { api, type Batch } from "../api";
import { store } from "../store";
import { clear, h, holdButton, reportErrors, toast } from "../ui";
import { analyzeIds } from "../main";
import { L, LOCALE } from "../i18n";

const KIND: Record<Batch["kind"], string> = { move: L("Dateien"), tags: L("Tags"), trash: L("Papierkorb"), dir: L("Ordner"), grid: L("Beatgrid / Cues") };

/** Einen Eintrag rückgängig machen (oder den letzten, wenn id = null). */
export async function undo(id: number | null) {
  try {
    const r = await api.undoBatch(id);
    reportErrors(r.errors, L("Rückgängig gemacht ({0})", r.restored));
    await store.loadTracks();
    await store.loadPlaylists();
    store.emit("dirs");
    store.emit("history");
    store.emit("pioneer");
    if (r.newIds.length) analyzeIds(r.newIds);
  } catch (e) {
    toast(String(e));
  }
}

/** Verlaufsliste – in der Ansicht „Verlauf“ und unter „Sortieren“ */
export function historyList(filter?: Batch["kind"][]): { el: HTMLElement; reload: () => void } {
  const el = h("div", {});
  const reload = async () => {
    const list = (await api.moveBatches()).filter((b) => !filter || filter.includes(b.kind));
    clear(el);
    if (!list.length) {
      el.append(h("div", { class: "note" }, L("Noch keine Änderungen.")));
      return;
    }
    for (const b of list) {
      el.append(
        h("div", { class: "setting" + (b.undone ? " undone" : "") },
          h("div", { style: { minWidth: "0" } },
            h("div", { class: "t1" }, b.description),
            h("div", { class: "note" }, `${KIND[b.kind] ?? b.kind} · ${new Date(b.created * 1000).toLocaleString(LOCALE)}${b.count > 1 ? ` · ${b.count}` : ""}`),
          ),
          b.undone ? h("span", { class: "sub" }, L("rückgängig gemacht")) : holdButton(L("Rückgängig"), () => undo(b.id), L("halten")),
        ),
      );
    }
  };
  reload();
  store.on("history", reload);
  return { el, reload };
}

export function historyView(): { el: HTMLElement; destroy: () => void } {
  const { el: list } = historyList();
  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, L("Verlauf")), h("span", { class: "sub" }, L("Strg+Z macht die letzte Änderung rückgängig"))),
    h("div", { class: "view-scroll" },
      h("div", { class: "note", style: { marginBottom: "12px" } },
        L("Jede Änderung an Dateien wird gemerkt: Tags und Cover, Verschieben, Umbenennen, Ordner, Papierkorb. Rückgängig stellt den vorherigen Stand wieder her – auch einzeln und nicht nur in Reihenfolge. Aus dem Papierkorb geholte Titel kommen zurück in ihre Playlisten."),
      ),
      h("div", { class: "card" }, list),
    ),
  );
  return { el, destroy: () => {} };
}

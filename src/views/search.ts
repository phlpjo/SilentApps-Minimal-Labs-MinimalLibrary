import { api, type Rule, type SmartRules } from "../api";
import { player } from "../player";
import { store } from "../store";
import { TrackList } from "../tracklist";
import { fmtDuration, h } from "../ui";
import { trackMenu } from "./library";
import { renderRuleRows } from "./playlist";
import { L, LOCALE } from "../i18n";

const KEY = "ml.adv";
const fresh = (): SmartRules => ({ all: true, rules: [{ field: "artist", op: "contains", value: "", value2: "" }], sort: "artist", desc: false, limit: null });

/** Erweiterte Suche: Regeln wie bei smarten Playlisten – nur anzeigen, nichts wird gespeichert. */
export function searchView(): { el: HTMLElement; destroy: () => void } {
  let rules: SmartRules;
  try {
    rules = JSON.parse(sessionStorage.getItem(KEY) ?? "") as SmartRules;
    if (!rules?.rules) rules = fresh();
  } catch {
    rules = fresh();
  }
  const list = new TrackList({ defaultSort: "artist", menu: (ids) => trackMenu(ids), columns: "library", emptyText: L("Keine Titel passen zu den Regeln.") });
  const info = h("span", { class: "sub" });
  const box = h("div", {});
  const allSel = h("select", {}, h("option", { value: "1", selected: rules.all }, L("alle Regeln")), h("option", { value: "0", selected: !rules.all }, L("mindestens eine Regel")));

  /** Leere Textregeln ignorieren, damit „enthält ''“ nicht alles bzw. nichts findet */
  const active = (): Rule[] => rules.rules.filter((r) => r.value.trim() !== "" || r.value2.trim() !== "");
  let timer = 0;
  let seq = 0;
  const refresh = () => {
    sessionStorage.setItem(KEY, JSON.stringify(rules));
    clearTimeout(timer);
    timer = window.setTimeout(async () => {
      const my = ++seq;
      const act = active();
      const ids = act.length ? await api.smartPreview({ ...rules, rules: act, limit: null }) : [...store.tracks.keys()];
      if (my !== seq) return;
      list.setIds(ids);
      player.queue = list.visibleIds;
      const dur = ids.reduce((a, id) => a + (store.tracks.get(id)?.duration ?? 0), 0);
      info.textContent = L("{0} Titel · {1}", ids.length.toLocaleString(LOCALE), fmtDuration(dur));
    }, 180);
  };
  allSel.addEventListener("change", () => { rules.all = allSel.value === "1"; refresh(); });
  const render = () => renderRuleRows(box, rules.rules, refresh);
  render();

  const unsub = store.on("tracks", refresh);
  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, L("Erweiterte Suche")), info, h("div", { class: "spacer" }),
      h("button", { class: "btn", onclick: () => { rules.rules.push({ field: "genre", op: "contains", value: "", value2: "" }); render(); } }, L("+ Regel")),
      h("button", { class: "btn", onclick: () => { rules = fresh(); allSel.value = "1"; render(); refresh(); } }, L("Zurücksetzen")),
    ),
    h("div", { class: "adv-rules" }, h("div", { class: "line", style: { marginTop: "0" } }, L("Titel, die"), allSel, L("erfüllen:")), box,
      h("div", { class: "note" }, L("Wie bei smarten Playlisten – aber nur zum Anzeigen. Es wird keine Playlist angelegt."))),
    list.el,
  );
  refresh();
  return { el, destroy: () => { clearTimeout(timer); unsub(); list.destroy(); } };
}

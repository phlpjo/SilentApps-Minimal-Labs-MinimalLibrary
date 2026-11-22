import { open } from "@tauri-apps/plugin-dialog";
import { api, type SyncPreview, type SyncTarget, type SyncTargetState } from "../api";
import { store } from "../store";
import { clear, fmtSize, h, holdButton, reportErrors, sheet, toast } from "../ui";

// ---------- Überwachung: Ziel angeschlossen? ----------

let targets: SyncTargetState[] = [];
const wasAvailable = new Map<number, boolean>();
const previews = new Map<number, SyncPreview | null>();
const running = new Set<number>();
let timer = 0;

export function syncReadyCount() {
  return targets.filter((t) => t.available).length;
}

async function refreshPreview(t: SyncTargetState) {
  if (!t.available) {
    previews.delete(t.id);
    return;
  }
  previews.set(t.id, await api.syncPreview(t.id).catch(() => null));
  store.emit("sync");
}

async function poll() {
  const list = await api.syncTargets().catch(() => targets);
  let changed = list.length !== targets.length;
  for (const t of list) {
    const before = wasAvailable.get(t.id);
    if (before !== t.available) changed = true;
    if (before === false && t.available) {
      // gerade angeschlossen
      if (t.auto) {
        toast(`„${t.name}“ angeschlossen – synchronisiere …`);
        runSync(t.id);
      } else {
        toast(`„${t.name}“ angeschlossen – Sync bereit`, 6000);
      }
    }
    if (before !== t.available) refreshPreview(t);
    wasAvailable.set(t.id, t.available);
  }
  targets = list;
  if (changed) store.emit("sync");
}

export function startSyncWatch() {
  clearInterval(timer);
  poll();
  timer = window.setInterval(poll, 3000);
  // Playlisten geändert → Vorschau neu
  store.on("playlists", () => targets.forEach((t) => t.available && refreshPreview(t)));
}

export async function runSync(id: number) {
  if (running.has(id)) return;
  running.add(id);
  store.emit("sync");
  try {
    const r = await api.syncRun(id);
    const parts = [`${r.copied} kopiert`, r.skipped ? `${r.skipped} schon aktuell` : "", r.removed ? `${r.removed} entfernt` : "", r.bytes ? fmtSize(r.bytes) : ""].filter(Boolean);
    reportErrors(r.errors, `Sync fertig: ${parts.join(", ")}`);
  } catch (e) {
    toast(String(e), 8000);
  } finally {
    running.delete(id);
    await poll();
    const t = targets.find((x) => x.id === id);
    if (t) refreshPreview(t);
    store.emit("sync");
  }
}

// ---------- Ansicht ----------

export function syncView(): { el: HTMLElement; destroy: () => void } {
  const box = h("div", {});
  const render = () => {
    clear(box);
    if (!targets.length) {
      box.append(h("div", { class: "empty" }, h("b", {}, "Noch kein Sync-Ziel"), h("br"),
        "Ein Ziel ist z. B. ein USB-Stick, eine SD-Karte oder eine Festplatte. Du wählst, welche Playlisten darauf sollen – die App kopiert Playlisten und Musikdateien und hält sie aktuell."));
    }
    for (const t of targets) {
      const pv = previews.get(t.id);
      const pls = t.playlists.map((id) => store.playlists.find((p) => p.id === id)?.name).filter(Boolean) as string[];
      const status = running.has(t.id)
        ? h("span", { class: "sync-state busy" }, "synchronisiert …")
        : t.available
          ? h("span", { class: "sync-state on" }, "● verbunden")
          : h("span", { class: "sync-state" }, "nicht verbunden");
      const info = t.available && pv
        ? pv.toCopy || pv.toRemove
          ? `${pv.toCopy} Titel zu kopieren (${fmtSize(pv.copyBytes)})${pv.toRemove ? `, ${pv.toRemove} zu entfernen` : ""}`
          : `aktuell · ${pv.upToDate} Titel`
        : "";
      box.append(
        h("div", { class: "card sync-card" + (t.available ? " ready" : "") },
          h("div", { class: "line", style: { margin: "0 0 6px" } },
            h("div", { class: "insp-title", style: { margin: "0" } }, t.name), status, h("span", { class: "spacer" }),
            t.available ? h("button", { class: "btn primary", disabled: running.has(t.id), onclick: () => runSync(t.id) }, "Synchronisieren") : "",
            t.available ? h("button", { class: "btn", disabled: running.has(t.id), onclick: async () => {
              try {
                await api.syncEject(t.path);
                toast(`„${t.name}“ kann entfernt werden`);
                poll();
              } catch (e) {
                toast(String(e), 7000);
              }
            } }, "Auswerfen") : "",
            h("button", { class: "btn", onclick: () => editTarget(t) }, "Bearbeiten"),
          ),
          h("div", { class: "kv" }, h("span", {}, "Ordner"), h("span", { style: { fontFamily: "var(--mono)" } }, t.path)),
          h("div", { class: "kv" }, h("span", {}, "Playlisten"), h("span", {}, pls.length ? pls.join(", ") : "– keine –")),
          h("div", { class: "kv" }, h("span", {}, "Modus"), h("span", {}, `${t.auto ? "automatisch beim Anschließen" : "manuell"}${t.mirror ? " · räumt auf" : ""}`)),
          t.available && t.free !== null ? h("div", { class: "kv" }, h("span", {}, "Frei"), h("span", {}, fmtSize(t.free))) : "",
          t.lastSync ? h("div", { class: "kv" }, h("span", {}, "Zuletzt"), h("span", {}, new Date(t.lastSync * 1000).toLocaleString("de"))) : "",
          info ? h("div", { class: "note", style: { marginTop: "8px", color: "var(--ink)" } }, info) : "",
        ),
      );
    }
  };
  const unsub = store.on("sync", render);
  render();
  poll();
  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Sync"), h("span", { class: "sub" }, "Playlisten + Musik auf USB-Stick, SD-Karte & Co."), h("span", { class: "spacer" }),
      h("button", { class: "btn primary", onclick: () => editTarget(null) }, "+ Ziel hinzufügen")),
    h("div", { class: "view-scroll" },
      box,
      h("div", { class: "note", style: { marginTop: "6px", lineHeight: "1.6" } },
        "Auf dem Ziel entstehen ein Ordner „Musik“ (Künstler/Album/Titel) und je Playlist eine .m3u8 – so findet Minimal Player Lieder und Playlisten. Kopiert wird nur, was fehlt. ",
        "„Aufräumen“ löscht ausschließlich Dateien, die die App selbst dorthin kopiert hat. ",
        "Hinweis: Android-Handys am Kabel erscheinen unter Windows nicht als Laufwerk (MTP) – dafür SD-Karte oder USB-Stick nutzen."),
    ),
  );
  return { el, destroy: unsub };
}

function editTarget(t: SyncTargetState | null) {
  const v: SyncTarget = t ? { ...t } : { id: 0, name: "", path: "", playlists: [], auto: false, mirror: true, lastSync: 0 };
  const name = h("input", { type: "text", value: v.name, placeholder: "z. B. USB-Stick Auto", style: { width: "100%" } });
  const path = h("input", { type: "text", value: v.path, placeholder: "Ordner auf dem Gerät, z. B. E:\\", style: { width: "100%" } });
  const auto = h("input", { type: "checkbox", checked: v.auto });
  const mirror = h("input", { type: "checkbox", checked: v.mirror });
  const chosen = new Set(v.playlists);
  const plBox = h("div", { style: { maxHeight: "240px", overflowY: "auto" } },
    ...(store.playlists.length
      ? store.playlists.map((p) => {
          const c = h("input", { type: "checkbox", checked: chosen.has(p.id) });
          c.addEventListener("change", () => (c.checked ? chosen.add(p.id) : chosen.delete(p.id)));
          return h("div", {}, h("label", { class: "check", style: { padding: "4px 0" } }, c, (p.rules ? "◇ " : "") + p.name, h("span", { class: "sub" }, ` · ${p.count} Titel`)));
        })
      : [h("div", { class: "note" }, "Noch keine Playlisten.")]),
  );
  const s = sheet(t ? `„${t.name}“ bearbeiten` : "Neues Sync-Ziel", h("div", {},
    h("div", { class: "field" }, h("label", {}, "Name"), name),
    h("div", { class: "field" }, h("label", {}, "Ordner auf dem Gerät"),
      h("div", { class: "line", style: { margin: "0" } }, h("div", { class: "grow" }, path),
        h("button", { class: "btn", onclick: async () => {
          const d = await open({ directory: true });
          if (d) {
            path.value = d as string;
            if (!name.value.trim()) name.value = (d as string).replace(/[\\/]+$/, "").split(/[\\/]/).pop() || (d as string);
          }
        } }, "Wählen …"))),
    h("div", { class: "card", style: { marginTop: "10px" } }, h("h3", {}, "Playlisten für dieses Ziel"), plBox),
    h("div", { class: "line" }, h("label", { class: "check" }, auto, "Automatisch synchronisieren, sobald das Gerät angeschlossen wird")),
    h("div", { class: "line" }, h("label", { class: "check" }, mirror, "Aufräumen: von der App kopierte Titel löschen, die in keiner gewählten Playlist mehr sind")),
    h("div", { class: "foot" },
      t ? holdButton("Ziel löschen", async () => { await api.syncDelete(t.id); s.close(); await poll(); store.emit("sync"); }, "halten") : "",
      h("span", { class: "spacer" }),
      h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
      h("button", {
        class: "btn primary",
        onclick: async () => {
          if (!path.value.trim()) return toast("Bitte einen Ordner wählen");
          const id = await api.syncSave({ ...v, name: name.value.trim() || path.value.trim(), path: path.value.trim(), playlists: [...chosen], auto: auto.checked, mirror: mirror.checked });
          s.close();
          await poll();
          const nt = targets.find((x) => x.id === id);
          if (nt) refreshPreview(nt);
          store.emit("sync");
        },
      }, "Speichern"),
    ),
  ), { wide: true });
}

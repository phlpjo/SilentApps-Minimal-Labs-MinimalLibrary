import { open } from "@tauri-apps/plugin-dialog";
import { api, type SyncLayout, type SyncPreview, type SyncTarget, type SyncTargetState } from "../api";
import { store } from "../store";
import { clear, fmtSize, h, holdButton, reportErrors, sheet, toast } from "../ui";
import { IS_MAC, OS } from "../platform";
import { rootOf } from "../fileops";

// ---------- Überwachung: Ziel angeschlossen? ----------

let targets: SyncTargetState[] = [];
const wasAvailable = new Map<number, boolean>();
const previews = new Map<number, SyncPreview | null>();
const running = new Set<number>();
let timer = 0;

export function syncReadyCount() {
  return targets.filter((t) => t.available).length;
}

/** Aufbau auf dem Gerät – dieselben drei Varianten wie beim Export */
export const LAYOUTS: [SyncLayout, string, string][] = [
  ["numbered", "Ein Ordner pro Playlist, nummeriert", "Ziel/Playlist/001 - Künstler - Titel.mp3 + Playlist.m3u8 – Reihenfolge bleibt auch ohne Playlist erhalten."],
  ["flat", "Ein Ordner pro Playlist", "Ziel/Playlist/Künstler - Titel.mp3 + Playlist.m3u8"],
  ["structured", "Gemeinsamer Musikordner", "Ziel/Musik/Künstler/Album/… und alle .m3u8 in Ziel – jede Datei nur einmal, auch wenn sie in mehreren Playlisten ist."],
];
/** Gespeicherten Wert lesen (alte Ziele: gemeinsamer Musikordner) */
const layoutOf = (t: { layout: string }): SyncLayout => (t.layout === "playlist" ? "numbered" : LAYOUTS.some((l) => l[0] === t.layout) ? (t.layout as SyncLayout) : "structured");

async function refreshPreview(t: SyncTargetState) {
  // auch ohne angeschlossenes Gerät: Größe der Playlisten anzeigen
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
    if (before !== t.available || !previews.has(t.id)) refreshPreview(t);
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
  store.on("playlists", () => targets.forEach((t) => refreshPreview(t)));
}

export async function runSync(id: number, force = false) {
  if (running.has(id)) return;
  running.add(id);
  store.emit("sync");
  try {
    const r = await api.syncRun(id, force);
    const parts = [`${r.copied} kopiert`, r.skipped ? `${r.skipped} schon aktuell` : "", r.removed ? `${r.removed} entfernt` : "", r.bytes ? fmtSize(r.bytes) : ""].filter(Boolean);
    reportErrors(r.errors, `Sync fertig: ${parts.join(", ")}`);
  } catch (e) {
    const msg = String(e);
    if (msg.includes("NOSPACE:")) noSpaceDialog(id, msg.slice(msg.indexOf("NOSPACE:") + 8));
    else toast(msg, 8000);
  } finally {
    running.delete(id);
    await poll();
    const t = targets.find((x) => x.id === id);
    if (t) refreshPreview(t);
    store.emit("sync");
  }
}

/** Zu wenig Platz gemeldet – bei Cloud-/Netzlaufwerken oft falsch (nur der lokale Zwischenspeicher) */
function noSpaceDialog(id: number, msg: string) {
  const s = sheet("Zu wenig Platz?", h("div", {},
    h("div", {}, msg),
    h("div", { class: "note", style: { marginTop: "8px" } },
      "Cloud- und Netzlaufwerke (z. B. pCloud, Google Drive, OneDrive) melden oft nur den Platz ihres lokalen Zwischenspeichers. Wenn das Ziel tatsächlich genug Platz hat, kannst du trotzdem synchronisieren – ist es wirklich voll, brechen die Kopien mit einer Meldung ab."),
    h("div", { class: "foot" },
      h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
      h("span", { class: "spacer" }),
      h("button", { class: "btn", onclick: async () => {
        s.close();
        const t = targets.find((x) => x.id === id);
        if (t) await api.syncSave({ ...t, ignoreSpace: true });
        await poll();
        runSync(id, true);
      } }, "Bei diesem Ziel nie mehr prüfen"),
      h("button", { class: "btn primary", onclick: () => { s.close(); runSync(id, true); } }, "Trotzdem synchronisieren"),
    )));
}

// ---------- Ansicht ----------

export function syncView(): { el: HTMLElement; destroy: () => void } {
  const box = h("div", {});
  const render = () => {
    clear(box);
    if (!targets.length) {
      box.append(h("div", { class: "empty" }, h("b", {}, "Noch kein Sync-Ziel"), h("br"),
        `Ein Ziel ist z. B. ein USB-Stick, eine SD-Karte, eine Festplatte${IS_MAC ? "" : " oder ein Handy am USB-Kabel"}. Du wählst, welche Playlisten darauf sollen – die App kopiert Playlisten und Musikdateien und hält sie aktuell.`));
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
            t.available && !t.device ? h("button", { class: "btn", disabled: running.has(t.id), onclick: async () => {
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
          t.device?.startsWith("wlan:")
            ? h("div", { class: "kv" }, h("span", {}, "Handy"), h("span", {}, h("b", {}, t.deviceName || "Handy"), " · im WLAN · Minimal Player Connected"))
            : t.device
            ? h("div", { class: "kv" }, h("span", {}, "Handy"), h("span", {}, h("b", {}, t.deviceName || "Handy"), " · ", h("span", { style: { fontFamily: "var(--mono)" } }, t.path)))
            : h("div", { class: "kv" }, h("span", {}, "Ordner"), h("span", { style: { fontFamily: "var(--mono)" } }, t.path)),
          h("div", { class: "kv" }, h("span", {}, "Playlisten"), h("span", {}, pls.length ? pls.join(", ") : "– keine –")),
          t.folders?.length ? h("div", { class: "kv" }, h("span", {}, "Ordner"), h("span", {}, t.folders.map((f) => f.split(/[\\/]/).filter(Boolean).pop()).join(", "))) : "",
          h("div", { class: "kv" }, h("span", {}, "Modus"), h("span", {}, `${t.auto ? "automatisch beim Anschließen" : "manuell"}${t.mirror ? " · räumt auf" : ""}`)),
          h("div", { class: "kv" }, h("span", {}, "Ablage"), h("span", {}, (LAYOUTS.find((l) => l[0] === layoutOf(t))?.[1] ?? "") + (t.noPlaylistFiles ? " · ohne Playlist-Dateien" : ""))),
          spaceBar(pv?.totalBytes ?? null, t.available ? t.free : null, pv && t.available ? pv.copyBytes : 0),
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
        "Auf dem Ziel landen die Titel in der gewählten Ablage und je Playlist eine .m3u8 – so findet Minimal Player Lieder und Playlisten. Kopiert wird nur, was fehlt. ",
        "„Aufräumen“ löscht ausschließlich Dateien, die die App selbst dorthin kopiert hat. ",
        "Handy im WLAN: in Minimal Player Connected unter Einstellungen „Von Minimal Library empfangen“ öffnen – das Handy erscheint dann hier als verbunden, Playlisten landen direkt in der App. ",
        IS_MAC
          ? `Hinweis: Android-Handys am Kabel erscheinen unter ${OS} nicht als Laufwerk (MTP) – dafür SD-Karte oder USB-Stick nutzen.`
          : "Handy am Kabel: per USB-Kabel anschließen, entsperren und in der Benachrichtigung „Dateiübertragung“ wählen. Über das Kabel (MTP) kopiert Windows deutlich langsamer als auf einen Stick – der erste Sync dauert entsprechend."),
    ),
  );
  return { el, destroy: unsub };
}

function editTarget(t: SyncTargetState | null) {
  const v: SyncTarget = t ? { ...t, folders: [...(t.folders ?? [])] } : { id: 0, name: "", path: "", playlists: [], auto: false, mirror: true, lastSync: 0, layout: "numbered", noPlaylistFiles: false, folders: [] };
  // Ganze Ordner (eine Richtung: Quellordner → Ziel, Struktur bleibt)
  const folderStats = (f: string) => {
    let n = 0;
    let bytes = 0;
    for (const tr of store.tracks.values()) {
      if (rootOf(tr.path, [f])) {
        n++;
        bytes += tr.size;
      }
    }
    return { n, bytes };
  };
  const fBox = h("div", {});
  const renderFolders = () => {
    fBox.replaceChildren(
      ...v.folders.map((f) => {
        const st = folderStats(f);
        return h("div", { class: "setting" },
          h("span", { style: { fontFamily: "var(--mono)", fontSize: "12px", wordBreak: "break-all" } }, f, h("span", { class: "sub" }, ` · ${st.n} Titel · ${fmtSize(st.bytes)}`)),
          h("button", { class: "btn", onclick: () => { v.folders = v.folders.filter((x) => x !== f); renderFolders(); upSize(); } }, "Entfernen"));
      }),
      h("button", { class: "btn", style: { marginTop: "6px" }, onclick: async () => {
        const d = await open({ directory: true, defaultPath: store.settings.roots[0] });
        if (d && !v.folders.includes(d as string)) {
          v.folders.push(d as string);
          renderFolders();
          upSize();
        }
      } }, "+ Ordner hinzufügen …"),
    );
  };
  let layout: SyncLayout = layoutOf(v);
  const radios = LAYOUTS.map(([k, label, note]) => {
    const r = h("input", { type: "radio", name: "synclayout", checked: layout === k });
    r.addEventListener("change", () => (layout = k));
    return h("div", { style: { margin: "6px 0" } }, h("label", { class: "check" }, r, label), h("div", { class: "note", style: { marginLeft: "23px" } }, note));
  });
  const m3u = h("input", { type: "checkbox", checked: !v.noPlaylistFiles });
  const sizeInfo = h("div", { class: "note", style: { color: "var(--ink)" } });
  const name = h("input", { type: "text", value: v.name, placeholder: "z. B. USB-Stick Auto", style: { width: "100%" } });
  // Handy im WLAN (Minimal Player Connected)
  let wlanId = v.device?.startsWith("wlan:") ? v.device.slice(5) : "";
  let wlanName = wlanId ? v.deviceName ?? "" : "";
  let token = wlanId ? v.token ?? "" : "";
  let phones: { id: string; name: string; addr: string; ready: boolean }[] = [];
  const wSel = h("select", { style: { width: "100%" } }) as HTMLSelectElement;
  const wNote = h("div", { class: "note" });
  const code = h("input", { type: "text", placeholder: "Code vom Handy, z. B. 482 913", inputmode: "numeric", style: { width: "100%" } }) as HTMLInputElement;
  const pairState = h("div", { class: "note", style: { color: "var(--ink)" } });
  const showPair = () => (pairState.textContent = token ? `✓ gekoppelt mit „${wlanName || "Handy"}“` : "noch nicht gekoppelt");
  const loadPhones = async () => {
    wSel.replaceChildren(h("option", { value: "" }, "suche im WLAN …"));
    phones = await api.syncPhones().catch(() => []);
    const opts = phones.map((p) => h("option", { value: p.id, selected: p.id === wlanId || (!wlanId && phones.length === 1) }, p.name + (p.ready ? "" : " (Empfangsordner fehlt)")));
    if (wlanId && !phones.some((p) => p.id === wlanId)) opts.unshift(h("option", { value: wlanId, selected: true }, `${wlanName || "Handy"} (gerade nicht sichtbar)`));
    wSel.replaceChildren(...(opts.length ? opts : [h("option", { value: "" }, "kein Handy gefunden")]));
    wNote.textContent = phones.length ? "" : "Auf dem Handy in Minimal Player Connected: Einstellungen → „Von Minimal Library empfangen“ öffnen. Handy und Computer im selben WLAN – dann „Neu suchen“.";
    pickPhone();
  };
  const pickPhone = () => {
    const id = wSel.value;
    if (!id) return;
    if (id !== wlanId) token = "";
    wlanId = id;
    wlanName = phones.find((p) => p.id === id)?.name ?? wlanName;
    if (!name.value.trim()) name.value = wlanName;
    showPair();
  };
  wSel.addEventListener("change", pickPhone);
  const doPair = async () => {
    const p = phones.find((x) => x.id === wlanId);
    if (!p) return toast("Handy gerade nicht sichtbar – „Von Minimal Library empfangen“ auf dem Handy öffnen und neu suchen");
    try {
      token = await api.syncPair(p.addr, code.value);
      code.value = "";
      showPair();
      toast("Gekoppelt");
    } catch (e) {
      toast(String(e), 6000);
    }
  };
  code.addEventListener("keydown", (e) => { if ((e as KeyboardEvent).key === "Enter") doPair(); });
  const wlanBox = h("div", {},
    h("div", { class: "field" }, h("label", {}, "Handy im WLAN"),
      h("div", { class: "line", style: { margin: "0" } }, h("div", { class: "grow" }, wSel), h("button", { class: "btn", onclick: loadPhones }, "Neu suchen")), wNote),
    h("div", { class: "field" }, h("label", {}, "Koppeln (einmalig)"),
      h("div", { class: "line", style: { margin: "0" } }, h("div", { class: "grow" }, code), h("button", { class: "btn", onclick: doPair }, "Koppeln")), pairState,
      h("div", { class: "note" }, "Den 6-stelligen Code zeigt das Handy unter „Von Minimal Library empfangen“. Die Musik landet im Empfangsordner, den du dort gewählt hast; Playlisten erscheinen direkt in der App. Während des Syncs den Bildschirm auf dem Handy offen lassen.")),
  );
  showPair();
  const path = h("input", { type: "text", value: v.device ? "" : v.path, placeholder: "Ordner auf dem Gerät, z. B. E:\\", style: { width: "100%" } });
  // Handy (MTP, nur Windows)
  let kind: "folder" | "phone" | "wlan" = v.device?.startsWith("wlan:") ? "wlan" : v.device ? "phone" : "folder";
  let device = v.device ?? "";
  let deviceName = v.deviceName ?? "";
  const phonePath = h("input", { type: "text", value: v.device ? v.path : "", placeholder: "z. B. Interner gemeinsamer Speicher/Music", style: { width: "100%" } }) as HTMLInputElement;
  const devSel = h("select", { style: { width: "100%" } }) as HTMLSelectElement;
  const devNote = h("div", { class: "note" });
  const loadDevices = async () => {
    devSel.replaceChildren(h("option", { value: "" }, "suche …"));
    const list = await api.syncDevices().catch(() => []);
    const opts = list.map((d) => h("option", { value: d.id, selected: d.id === device || (!device && list.length === 1) }, d.name));
    if (device && !list.some((d) => d.id === device)) opts.unshift(h("option", { value: device, selected: true }, `${deviceName || "Handy"} (nicht verbunden)`));
    devSel.replaceChildren(...(opts.length ? opts : [h("option", { value: "" }, "kein Handy gefunden")]));
    devNote.textContent = list.length ? "" : "Handy per USB anschließen, entsperren und „Dateiübertragung“ wählen – dann „Neu suchen“.";
    pickDevice();
  };
  const pickDevice = async () => {
    const id = devSel.value;
    if (!id) return;
    device = id;
    deviceName = devSel.selectedOptions[0]?.textContent?.replace(/ \(nicht verbunden\)$/, "") ?? "";
    if (!name.value.trim()) name.value = deviceName;
    // Vorschlag: Musikordner auf dem (einzigen) Speicher
    if (!phonePath.value.trim()) {
      const st = await api.syncDeviceFolders(id, "").catch(() => []);
      if (st.length) phonePath.value = `${st[0]}/Music`;
    }
  };
  devSel.addEventListener("change", pickDevice);
  const folderBox = h("div", { class: "field" }, h("label", {}, "Ordner auf dem Gerät"),
    h("div", { class: "line", style: { margin: "0" } }, h("div", { class: "grow" }, path),
      h("button", { class: "btn", onclick: async () => {
        const d = await open({ directory: true });
        if (d) {
          path.value = d as string;
          if (!name.value.trim()) name.value = (d as string).replace(/[\\/]+$/, "").split(/[\\/]/).pop() || (d as string);
        }
      } }, "Wählen …")));
  const phoneBox = h("div", {},
    h("div", { class: "field" }, h("label", {}, "Handy"),
      h("div", { class: "line", style: { margin: "0" } }, h("div", { class: "grow" }, devSel), h("button", { class: "btn", onclick: loadDevices }, "Neu suchen")), devNote),
    h("div", { class: "field" }, h("label", {}, "Ordner auf dem Handy"),
      h("div", { class: "line", style: { margin: "0" } }, h("div", { class: "grow" }, phonePath),
        h("button", { class: "btn", onclick: async () => {
          if (!device) return toast("Erst ein Handy wählen");
          const p = await browseDevice(device, deviceName, phonePath.value.trim());
          if (p) phonePath.value = p;
        } }, "Wählen …")),
      h("div", { class: "note" }, "Fehlende Ordner legt der Sync an. Minimal Player findet die Musik dort, wenn du den Ordner in der App hinzufügst.")),
  );
  const showKind = () => {
    folderBox.style.display = kind === "folder" ? "" : "none";
    phoneBox.style.display = kind === "phone" ? "" : "none";
    wlanBox.style.display = kind === "wlan" ? "" : "none";
    if (kind === "phone" && !devSel.options.length) loadDevices();
    if (kind === "wlan" && !wSel.options.length) loadPhones();
  };
  const kindRadio = (k: "folder" | "phone" | "wlan", label: string) => {
    const r = h("input", { type: "radio", name: "synckind", checked: kind === k });
    r.addEventListener("change", () => { kind = k; showKind(); });
    return h("label", { class: "check", style: { marginRight: "18px" } }, r, label);
  };
  const kindRow = h("div", { class: "line", style: { flexWrap: "wrap" } },
    kindRadio("folder", "Ordner / Laufwerk"),
    IS_MAC ? "" : kindRadio("phone", "Handy am USB-Kabel"),
    kindRadio("wlan", "Handy im WLAN (Minimal Player Connected)"));
  showKind();
  const auto = h("input", { type: "checkbox", checked: v.auto });
  const mirror = h("input", { type: "checkbox", checked: v.mirror });
  const ignoreSpace = h("input", { type: "checkbox", checked: !!v.ignoreSpace }) as HTMLInputElement;
  const chosen = new Set(v.playlists);
  const plBox = h("div", { style: { maxHeight: "240px", overflowY: "auto" } },
    ...(store.playlists.length
      ? store.playlists.map((p) => {
          const c = h("input", { type: "checkbox", checked: chosen.has(p.id) });
          c.addEventListener("change", () => { c.checked ? chosen.add(p.id) : chosen.delete(p.id); upSize(); });
          const sz = h("span", { class: "sub" }, ` · ${p.count} Titel`);
          api.exportSize([p.id]).then((b) => (sz.textContent = ` · ${p.count} Titel · ${fmtSize(b)}`)).catch(() => {});
          return h("div", {}, h("label", { class: "check", style: { padding: "4px 0" } }, c, (p.rules ? "◇ " : "") + p.name, sz));
        })
      : [h("div", { class: "note" }, "Noch keine Playlisten.")]),
  );
  let sizeSeq = 0;
  const upSize = async () => {
    const my = ++sizeSeq;
    const b = (chosen.size ? await api.exportSize([...chosen]).catch(() => 0) : 0) + v.folders.reduce((s2, f) => s2 + folderStats(f).bytes, 0);
    if (my !== sizeSeq) return;
    const free = t?.available ? t.free : null;
    sizeInfo.textContent = `Zusammen: ${fmtSize(b)}${free !== null && free !== undefined ? ` · frei auf dem Gerät: ${fmtSize(free)}` : ""}`;
  };
  renderFolders();
  upSize();
  const s = sheet(t ? `„${t.name}“ bearbeiten` : "Neues Sync-Ziel", h("div", {},
    h("div", { class: "field" }, h("label", {}, "Name"), name),
    kindRow,
    folderBox,
    phoneBox,
    wlanBox,
    h("div", { class: "card", style: { marginTop: "10px" } }, h("h3", {}, "Playlisten für dieses Ziel"), plBox),
    h("div", { class: "card" }, h("h3", {}, "Ganze Ordner"), fBox,
      h("div", { class: "note" }, "Nur in eine Richtung: Quellordner → Ziel. Auf dem Ziel als „Ordnername/…“ mit denselben Unterordnern.")),
    sizeInfo,
    h("div", { class: "card" }, h("h3", {}, "Aufbau der Playlisten"), ...radios),
    h("div", { class: "line" }, h("label", { class: "check" }, m3u, "Playlist-Dateien (.m3u8) mit ablegen")),
    h("div", { class: "line" }, h("label", { class: "check" }, auto, "Automatisch synchronisieren, sobald das Gerät angeschlossen wird")),
    h("div", { class: "line" }, h("label", { class: "check" }, mirror, "Aufräumen: von der App kopierte Titel löschen, die in keiner gewählten Playlist bzw. keinem gewählten Ordner mehr sind")),
    h("div", { class: "line" }, h("label", { class: "check" }, ignoreSpace, "Freien Platz nicht prüfen (Cloud- und Netzlaufwerke wie pCloud melden oft zu wenig)")),
    h("div", { class: "foot" },
      t ? holdButton("Ziel löschen", async () => { await api.syncDelete(t.id); s.close(); await poll(); store.emit("sync"); }, "halten") : "",
      h("span", { class: "spacer" }),
      h("button", { class: "btn", onclick: () => s.close() }, "Abbrechen"),
      h("button", {
        class: "btn primary",
        onclick: async () => {
          if (kind === "wlan") {
            if (!wlanId) return toast("Bitte ein Handy wählen");
            if (!token) return toast("Bitte erst mit dem Code vom Handy koppeln");
          }
          const wl = kind === "wlan";
          const phone = kind === "phone";
          const dest = (phone ? phonePath.value : path.value).trim().replace(/^[\\/]+|[\\/]+$/g, phone ? "" : "$&");
          if (phone && !device) return toast("Bitte ein Handy wählen");
          if (!dest && !wl) return toast("Bitte einen Ordner wählen");
          const id = await api.syncSave({ ...v, device: wl ? `wlan:${wlanId}` : phone ? device : "", deviceName: wl ? wlanName : phone ? deviceName : "", token: wl ? token : "", name: name.value.trim() || (wl ? wlanName : phone ? deviceName : dest), path: wl ? "" : dest, playlists: [...chosen], auto: auto.checked, mirror: mirror.checked, layout, noPlaylistFiles: !m3u.checked, folders: v.folders, ignoreSpace: ignoreSpace.checked });
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

/** Ordner auf dem Handy wählen (oberste Ebene = Speicher) */
function browseDevice(device: string, deviceName: string, start: string): Promise<string | null> {
  return new Promise((resolve) => {
    let cur = start.split(/[\\/]/).filter(Boolean);
    let done = false;
    const finish = (v: string | null) => { if (!done) { done = true; resolve(v); } s.close(); };
    const crumbs = h("div", { style: { fontFamily: "var(--mono)", fontSize: "12px", margin: "0 0 8px", wordBreak: "break-all" } });
    const list = h("div", { style: { maxHeight: "320px", overflowY: "auto", minHeight: "120px" } });
    const take = h("button", { class: "btn primary", onclick: () => finish(cur.join("/")) }, "Diesen Ordner nehmen") as HTMLButtonElement;
    const load = async () => {
      crumbs.textContent = `${deviceName || "Handy"} / ${cur.join(" / ")}`;
      take.disabled = cur.length === 0; // ein Speicher muss gewählt sein
      list.replaceChildren(h("div", { class: "note" }, "lese …"));
      try {
        const sub = await api.syncDeviceFolders(device, cur.join("/"));
        const row = (label: string, go: () => void) =>
          h("div", { class: "setting", style: { cursor: "pointer", padding: "6px 4px" }, onclick: go }, h("span", {}, label));
        list.replaceChildren(
          ...(cur.length ? [row("↑ eine Ebene höher", () => { cur = cur.slice(0, -1); load(); })] : []),
          ...sub.map((n) => row("▸ " + n, () => { cur = [...cur, n]; load(); })),
          ...(sub.length ? [] : [h("div", { class: "note" }, cur.length ? "Keine Unterordner." : "Kein Speicher sichtbar – ist das Handy entsperrt und auf „Dateiübertragung“ gestellt?")]),
        );
      } catch (e) {
        list.replaceChildren(h("div", { class: "note" }, String(e)));
      }
    };
    const s = sheet("Ordner auf dem Handy", h("div", {}, crumbs, list,
      h("div", { class: "foot" }, h("span", { class: "spacer" }),
        h("button", { class: "btn", onclick: () => finish(null) }, "Abbrechen"), take)),
      { onClose: () => { if (!done) { done = true; resolve(null); } } });
    load();
  });
}

/** Speicher: belegt durch die Playlisten · frei auf dem Gerät (mit Balken) */
function spaceBar(total: number | null, free: number | null, pending: number): HTMLElement | string {
  if (total === null && free === null) return "";
  const kv = (k: string, v: string) => h("div", { class: "kv" }, h("span", {}, k), h("span", {}, v));
  const rows: (HTMLElement | string)[] = [
    kv("Playlisten belegen", total !== null ? fmtSize(total) : "…"),
    free !== null ? kv("Frei auf dem Gerät", fmtSize(free) + (pending > free ? " – reicht nicht!" : "")) : kv("Frei auf dem Gerät", "nicht verbunden"),
  ];
  if (total !== null && free !== null) {
    // Anteil: schon belegter Platz der Playlisten vs. frei (nach dem Sync)
    const synced = Math.max(0, total - pending);
    const cap = synced + free;
    const pct = cap > 0 ? Math.min(100, (total / cap) * 100) : 0;
    rows.push(h("div", { class: "space-bar" + (pending > free ? " over" : "") }, h("div", { style: { width: pct.toFixed(1) + "%" } })));
  }
  return h("div", {}, ...rows);
}

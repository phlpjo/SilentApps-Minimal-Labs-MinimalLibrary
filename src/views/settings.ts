import { IS_WEB } from "../platform";
import { setStarMode, starMode, type StarMode } from "../stars";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import { api } from "../api";
import { store } from "../store";
import { clear, h, holdButton, toast } from "../ui";
import { analyzeIds, scanNow, scanning } from "../main";
import { applyTheme, themeMode, type ThemeMode } from "../theme";
import { IS_MAC, FILE_MANAGER } from "../platform";
import { autoInstall, checkUpdate, installUpdate, updater } from "../updater";
import { L, LOCALE, trMsg, langMode, setLangMode, type LangMode } from "../i18n";
import { BUY_URL, FEATURE_NAME, license, licenseSheet, setLicense, unlocked, type Feature } from "../license";

export function settingsView(): { el: HTMLElement; destroy: () => void } {
  const body = h("div", { class: "view-scroll" });
  // Musikordner sofort zeigen (hinzufügen/entfernen), auch bevor die Bibliothek fertig ist
  let pendingRoots: string[] | null = null;
  // Titel je Musikordner und ob gerade eingelesen wird
  const rootStatus = (r: string) => {
    const pre = r.replace(/[\\/]+$/, "").toLowerCase();
    let n = 0;
    for (const t of store.tracks.values()) {
      const p = t.path.toLowerCase();
      if (p.startsWith(pre + "\\") || p.startsWith(pre + "/")) n++;
    }
    const busy = scanning.all || scanning.roots.has(r);
    return (busy ? L("wird eingelesen … ") : "") + L("{0} Titel", n.toLocaleString(LOCALE));
  };
  let version = "";
  api.version().then((v) => { version = v; render(); });

  const render = () => {
    const s = store.settings;
    clear(body);
    const rootList = pendingRoots ?? s.roots;
    const roots = h("div", {},
      ...(rootList.length
        ? rootList.map((r) => {
            // Duplikatsuche: Titel dieses Ordners auch mit den anderen Musikordnern vergleichen?
            const cross = h("input", { type: "checkbox", checked: !(s.noCrossDup ?? []).includes(r) }) as HTMLInputElement;
            cross.addEventListener("change", () => {
              const set = new Set(s.noCrossDup ?? []);
              cross.checked ? set.delete(r) : set.add(r);
              store.saveSettings({ noCrossDup: [...set] });
            });
            // Cloud-Laufwerk (pCloud, OneDrive …): nichts automatisch komplett lesen, sonst lädt es alles herunter
            const od = h("input", { type: "checkbox", checked: store.onDemand.includes(r) }) as HTMLInputElement;
            od.addEventListener("change", () => {
              const on = new Set(s.onDemand ?? []);
              const off = new Set(s.onDemandOff ?? []);
              if (od.checked) on.add(r); else on.delete(r);
              off.delete(r);
              store.saveSettings({ onDemand: [...on], onDemandOff: [...off] });
            });
            return h("div", { class: "setting", style: { flexWrap: "wrap", gap: "6px 14px" } },
              h("span", { style: { fontFamily: "var(--mono)", fontSize: "12px", flex: "1", minWidth: "220px", wordBreak: "break-all" } }, r),
              s.roots.length > 1 ? h("label", { class: "check", title: L("Aus: Duplikate nur innerhalb dieses Ordners suchen") }, cross, L("Duplikate mit anderen Musikordnern finden")) : "",
              h("label", { class: "check", title: L("Spart Speicher bei Cloud-Laufwerken wie pCloud oder OneDrive: Titel und Tags werden gelesen, Wellenform, Tempo, Tonart, Qualität und Fingerabdruck aber erst beim Abspielen oder Export. Ohne Haken wird auch ein Cloud-Ordner komplett analysiert – Datei für Datei nacheinander.") }, od, L("Nur bei Bedarf laden (Cloud)")),
              h("span", { class: "note root-status", "data-root": r, style: { margin: "0" } }, rootStatus(r)),
              // Entfernen nur mit Gedrückthalten; Liste sofort aktualisieren, Bibliothek räumt im Hintergrund auf
              holdButton(L("Entfernen"), async () => {
                const keep = s.roots.filter((x) => x !== r);
                pendingRoots = keep;
                render();
                await store.saveSettings({ roots: keep, noCrossDup: (s.noCrossDup ?? []).filter((x) => x !== r), onDemand: (s.onDemand ?? []).filter((x) => x !== r), onDemandOff: (s.onDemandOff ?? []).filter((x) => x !== r) });
                pendingRoots = null;
                await store.loadTracks();
                render();
              }, L("halten · Titel verlassen die Bibliothek")),
            );
          })
        : [h("div", { class: "note" }, L("Noch kein Musikordner. Füge den Ordner hinzu, in dem deine Musik liegt."))]),
    );
    const threads = h("select", {}, ...[1, 2, 3, 4, 6, 8, 12, 16].map((n) => h("option", { value: String(n), selected: n === s.threads }, String(n))));
    threads.addEventListener("change", () => store.saveSettings({ threads: Number(threads.value) }));
    const acoustid = h("input", { type: "text", value: s.acoustidKey ?? "", placeholder: L("z. B. AbCdEf1234"), spellcheck: false, style: { width: "220px", fontFamily: "var(--mono)" } });
    acoustid.addEventListener("change", () => store.saveSettings({ acoustidKey: acoustid.value.trim() }));
    const auto = h("input", { type: "checkbox", checked: s.autoAnalyze });
    auto.addEventListener("change", () => store.saveSettings({ autoAnalyze: auto.checked }));

    const tray = h("input", { type: "checkbox", checked: s.closeToTray !== false });
    tray.addEventListener("change", () => store.saveSettings({ closeToTray: tray.checked }));
    const theme = h("select", {}, ...([["system", IS_WEB ? L("Wie System") : IS_MAC ? L("Wie macOS") : L("Wie Windows")], ["dark", L("Dunkel")], ["light", L("Hell")]] as const).map(([v, l]) => h("option", { value: v, selected: themeMode() === v }, l)));
    theme.addEventListener("change", () => applyTheme(theme.value as ThemeMode));
    const lang = h("select", {}, ...([["system", L("Wie System")], ["de", "Deutsch"], ["en", "English"]] as const).map(([v, l]) => h("option", { value: v, selected: langMode() === v }, l)));
    lang.addEventListener("change", () => setLangMode(lang.value as LangMode));
    body.append(
      licenseCard(),
      IS_WEB ? nasCard() : "",
      h("div", { class: "card" }, h("h3", {}, L("Darstellung")), h("div", { class: "setting" }, h("span", {}, L("Farbschema")), theme),
        h("div", { class: "setting" }, h("span", {}, L("Sprache")), lang),
        h("div", { class: "setting" }, h("span", {}, L("Sternenhimmel hinter den Listen")), (() => {
          const sel = h("select", {}, ...([["move", L("bewegt")], ["calm", L("ruhig")], ["off", L("aus")]] as const).map(([v, l]) => h("option", { value: v, selected: starMode() === v }, l))) as HTMLSelectElement;
          sel.addEventListener("change", () => setStarMode(sel.value as StarMode));
          return sel;
        })()),
        IS_WEB ? "" : h("div", { class: "setting" }, h("label", { class: "check" }, tray, L("Schließen (X) legt die App in den Infobereich – Musik läuft weiter")), h("button", { class: "btn", onclick: () => api.quit() }, L("Jetzt beenden"))),
        h("div", { class: "note" }, L("Der Schalter oben links wechselt direkt zwischen Hell und Dunkel."))),
      h("div", { class: "card" }, h("h3", {}, L("Musikordner")), roots,
        h("div", { class: "line" },
          h("button", { class: "btn primary", onclick: async () => {
            const d = await open({ directory: true, multiple: false });
            if (!d) return;
            const dir = d as string;
            if (s.roots.includes(dir)) return toast(L("Ist schon ein Musikordner"));
            // sofort zeigen, dann einlesen (samt allen Unterordnern) und gleich analysieren
            pendingRoots = [...s.roots, dir];
            scanning.roots.add(dir);
            render();
            await store.saveSettings({ roots: [...new Set([...s.roots, dir])] });
            pendingRoots = null;
            render();
            scanNow([dir]);
          } }, L("Ordner hinzufügen …")),
          h("button", { class: "btn", onclick: () => scanNow() }, L("Jetzt scannen")),
        ),
        h("div", { class: "note" }, L("Die Musikordner werden laufend überwacht: Was du im {0} hinzufügst, verschiebst, umbenennst oder löschst, übernimmt die App sofort – Analyse und Playlisten bleiben beim Verschieben erhalten.", FILE_MANAGER)),
      ),
      phoneCard(),
      pioneerCard(),
      h("div", { class: "card" }, h("h3", {}, L("Analyse")),
        h("div", { class: "setting" }, h("span", {}, L("Gleichzeitige Analysen")), threads),
        h("div", { class: "setting" }, h("label", { class: "check" }, auto, L("Neue Titel automatisch analysieren")), h("span", {})),
        h("div", { class: "line" },
          h("button", { class: "btn", onclick: () => analyzeIds(null) }, L("Fehlende analysieren")),
          holdButton(L("Alles neu analysieren"), () => analyzeIds([...store.tracks.keys()]), L("halten · dauert")),
        ),
        h("div", { class: "line" }, h("button", { class: "btn", onclick: async () => { const p = await api.diagPath(); if (p) revealItemInDir(p).catch(() => toast(L("Noch kein Protokoll vorhanden"))); } }, L("Protokoll zeigen"))),
        h("div", { class: "note" }, L("Die Analyse erstellt Wellenform, Audio-Fingerabdruck (für Duplikate), BPM und Tonart. Sie liest jede Datei einmal vollständig.")),
      ),
      h("div", { class: "card" }, h("h3", {}, L("Online (nur auf Klick)")),
        h("div", { class: "setting" }, h("span", {}, L("AcoustID-Schlüssel")), acoustid),
        h("div", { class: "note" },
          L("Für „Am Klang erkennen“. Einmalig kostenlos anlegen: "),
          h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); openUrl("https://acoustid.org/new-application"); } }, "acoustid.org/new-application"),
          L(" (Name z. B. „Minimal Library“), den angezeigten API-Schlüssel hier einfügen. Übertragen werden nur Fingerabdruck und Dauer."),
        ),
      ),
      h("div", { class: "card" }, h("h3", {}, L("Tastatur")),
        ...[
          [L("Leertaste"), L("Abspielen / Pause")], ["← / →", L("5 Sekunden zurück / vor")], [L("Strg+F"), L("Suchen")], [L("Strg+A"), L("Alle auswählen")],
          [L("Strg+S"), L("Tags speichern")], [L("Strg+Z"), L("Letzte Änderung rückgängig")], ["F2", L("Datei umbenennen")], ["Enter", L("Auswahl abspielen")], [L("Entf"), L("Aus Playlist entfernen")], [L("Doppelklick"), L("Abspielen")], [L("Klick in Wellenform"), L("ab dieser Stelle abspielen")],
        ].map(([k, v]) => h("div", { class: "setting" }, h("span", {}, v), h("span", { class: "v", style: { fontFamily: "var(--mono)", fontSize: "12px" } }, k))),
      ),
      updateCard(version),
      h("div", { class: "card" }, h("h3", {}, L("Über")),
        h("div", { class: "setting" }, h("span", {}, L("Version")), h("span", { class: "v" }, version)),
        h("div", { class: "note", style: { margin: "6px 0" } }, L("Pioneer DJ, rekordbox und OneLibrary sind Marken der AlphaTheta Corporation. Minimal Library steht in keiner Verbindung zu AlphaTheta.")),
        h("div", { class: "setting" }, h("span", {}, L("Lizenzen")), h("span", { class: "v" }, L("Space Grotesk (OFL 1.1) · Symphonia (MPL-2.0) · Lofty (MIT/Apache-2.0) · rusty-chromaprint (MIT) · Tauri (MIT/Apache-2.0)"))),
        h("div", { class: "option", onclick: () => openUrl("https://silentapps-minimal-labs.com/de/") }, L("Weitere Apps von SilentApps Minimal Labs"), h("small", {}, "›")),
        h("div", { class: "option", onclick: () => openUrl(`mailto:apps@silentapps-minimal-labs.com?subject=${encodeURIComponent(L("Minimal Library – Feedback"))}&body=${encodeURIComponent(L("\n\n—\nMinimal Library {0}\n{1}", version, navigator.userAgent))}`) }, L("Fehler melden & Vorschläge"), h("small", {}, "›")),
      ),
    );
  };
  // Titelzahl/„wird eingelesen“ je Musikordner laufend nachführen (ohne die Seite neu aufzubauen)
  let statT = 0;
  const updStatus = () => {
    if (statT) return;
    statT = window.setTimeout(() => {
      statT = 0;
      body.querySelectorAll<HTMLElement>(".root-status").forEach((e) => (e.textContent = rootStatus(e.dataset.root ?? "")));
    }, 400);
  };
  const unsubs = [store.on("settings", render), store.on("update", render), store.on("tracks", updStatus), store.on("scanning", updStatus)];
  render();
  const el = h("div", { style: { display: "contents" } }, h("div", { class: "head" }, h("h1", {}, L("Einstellungen"))), body);
  return { el, destroy: () => unsubs.forEach((u) => u()) };
}

function updateCard(version: string) {
  const u = updater;
  const status =
    u.state === "checking" ? L("Suche nach Updates …")
    : u.state === "available" ? L("Version {0} ist verfügbar", u.available!.version)
    : u.state === "downloading" ? L("Lädt … {0} %", Math.round(u.progress * 100))
    : u.state === "error" ? L("Prüfung fehlgeschlagen: {0}", trMsg(String(u.error)))
    : u.state === "none" ? L("Aktuell ({0})", version)
    : L("Installiert: {0}", version);
  const auto = h("input", { type: "checkbox", checked: autoInstall.on });
  auto.addEventListener("change", () => (autoInstall.on = auto.checked));
  return h("div", { class: "card" }, h("h3", {}, L("Updates")),
    h("div", { class: "setting" }, h("span", {}, status),
      u.state === "available"
        ? h("button", { class: "btn primary", onclick: () => installUpdate() }, L("Installieren & neu starten"))
        : h("button", { class: "btn", disabled: u.state === "checking" || u.state === "downloading", onclick: () => checkUpdate(true) }, L("Jetzt prüfen")),
    ),
    u.state === "available" && u.available!.body ? h("div", { class: "note" }, u.available!.body) : null,
    h("label", { class: "check", style: { marginTop: "8px" } }, auto, L("Beim Start automatisch installieren (nur wenn gerade keine Musik läuft)")),
    h("div", { class: "note" }, L("Updates kommen signiert aus den GitHub-Releases. Die App prüft beim Start und alle 6 Stunden.")),
  );
}

/** Minimal Player Connected: Bibliothek im WLAN freigeben (wie Spotify aufs Handy) */
/** Lizenz: Testphase / lizenziert / abgelaufen, Schlüssel eintragen, Kauf-Link */
function licenseCard() {
  const box = h("div", {});
  const fill = () => {
    const l = license;
    const status =
      l.state === "licensed" ? L("Lizenziert für {0}", l.name || "–") + (l.kind === "free" ? L(" (geschenkt)") : "")
      : l.state === "trial" ? L("Testphase: noch {0} Tage – alle Funktionen frei", l.daysLeft)
      : l.state === "revoked" ? L("Dieser Schlüssel wurde gesperrt.")
      : L("Testphase vorbei – die Funktionen der Vollversion sind gesperrt.");
    const input = h("input", { type: "text", placeholder: "MLIB-…", style: { flex: "1", minWidth: "220px", fontFamily: "var(--mono)", fontSize: "12px" } }) as HTMLInputElement;
    const msg = h("div", { class: "note" });
    box.replaceChildren(
      h("div", { class: "setting" }, h("span", {}, h("b", {}, status), l.email ? h("div", { class: "note" }, l.email) : ""),
        l.state === "licensed" ? holdButton(L("Lizenz entfernen"), async () => { await api.licenseRemove().then((s) => { setLicense(s); }).catch((e) => toast(String(e))); }, L("halten")) : ""),
      l.state !== "licensed"
        ? h("div", { class: "setting", style: { gap: "8px", flexWrap: "wrap" } }, input,
            h("button", { class: "btn primary", onclick: async () => {
              try { setLicense(await api.licenseActivate(input.value)); } catch (e) { msg.textContent = trMsg(String(e)); }
            } }, L("Freischalten")))
        : "",
      msg,
      h("div", { class: "setting" },
        h("span", { class: "note" }, L("Vollversion: {0}. Alles andere bleibt kostenlos.", (["connect", "organize", "smart", "search", "sync", "ripper"] as Feature[]).map((f) => FEATURE_NAME[f]()).join(", "))),
        l.state !== "licensed" ? h("button", { class: "btn", onclick: () => openUrl(BUY_URL()).catch(() => {}) }, L("Lizenz kaufen …")) : ""),
    );
  };
  fill();
  const off = store.on("license", fill);
  const card = h("div", { class: "card" }, h("h3", {}, L("Lizenz")), box);
  // beim Verlassen der Einstellungen abmelden
  const t = window.setInterval(() => { if (!document.body.contains(card)) { off(); clearInterval(t); } }, 5000);
  return card;
}

function phoneCard() {
  const box = h("input", { type: "checkbox", checked: !store.settings.remoteOff }) as HTMLInputElement;
  const body = h("div", {});
  const fill = async () => {
    if (!unlocked()) {
      box.disabled = true;
      return body.replaceChildren(h("div", { class: "setting" }, h("span", { class: "note" }, L("Gehört zur Vollversion – die Testphase ist vorbei.")), h("button", { class: "btn", onclick: () => licenseSheet("connect") }, L("Freischalten …"))));
    }
    box.disabled = false;
    if (!box.checked) return body.replaceChildren();
    const i = await api.remoteInfo().catch(() => null);
    if (!i) return;
    body.replaceChildren(
      h("div", { class: "setting" }, h("span", {}, L("Kopplungscode")), h("b", { style: { fontSize: "20px", letterSpacing: "3px" } }, i.code.replace(/(\d{3})(\d{3})/, "$1 $2"))),
      ...i.paired.map((n) => {
        // Ordner, in den neue Musik von diesem Handy kommt (Standard: Ablage › Gerätename)
        const own = store.settings.deviceDirs?.[n];
        const setDir = async (d: string | null) => {
          const m = { ...(store.settings.deviceDirs ?? {}) };
          if (d) m[n] = d; else delete m[n];
          await store.saveSettings({ deviceDirs: m });
          fill();
        };
        return h("div", { class: "setting", style: { flexWrap: "wrap", gap: "6px 12px" } },
          h("span", {}, "📱 " + n, h("div", { class: "note" }, own ? L("Ordner: {0}", own) : L("Ordner: {0} (Standard)", (store.settings.phoneDir || L("Musik › Minimal Library")) + " › " + n))),
          h("span", { style: { display: "flex", gap: "8px" } },
            h("button", { class: "btn", title: L("Ordner für neue Musik von diesem Handy"), onclick: async () => {
              const d = await open({ directory: true, defaultPath: own });
              if (d) setDir(d as string);
            } }, L("Ordner ändern …")),
            own ? h("button", { class: "btn", onclick: () => setDir(null) }, L("Standard")) : "",
            (() => {
              // Geteilter Haushalt: Handy darf die Wiedergaben hier steuern oder nur sehen, was läuft
              const c = h("input", { type: "checkbox", checked: !(i.viewOnly ?? []).includes(n) }) as HTMLInputElement;
              c.addEventListener("change", () => api.remoteAllowControl(n, c.checked).catch((e) => toast(String(e))));
              return h("label", { class: "check", title: L("Aus: das Handy sieht nur, was hier läuft") }, c, L("darf steuern"));
            })(),
            h("button", { class: "btn", onclick: async () => { await api.remoteUnpair(n); fill(); } }, L("Entkoppeln"))));
      }),
      i.port ? "" : h("div", { class: "err" }, L("Freigabe konnte nicht starten (Netzwerk blockiert?).")),
      h("div", { class: "setting" },
        h("span", {}, L("Ablage für Musik vom Handy"), h("div", { class: "note" }, (store.settings.phoneDir || L("Musik › Minimal Library")) + L(" › <Gerätename> – wird als eigene Musikquelle angelegt (je Handy änderbar)"))),
        h("button", { class: "btn", onclick: async () => {
          const d = await open({ directory: true });
          if (d) { await store.saveSettings({ phoneDir: d as string }); fill(); }
        } }, L("Ändern …"))),
    );
  };
  box.addEventListener("change", async () => { await store.saveSettings({ remoteOff: !box.checked }); fill(); });
  fill();
  // Name dieses PCs – so heißt er am Handy und im Ausgabe-Menü
  const nameIn = h("input", { type: "text", placeholder: "", style: { width: "220px" } }) as HTMLInputElement;
  api.remoteInfo().then((i) => { nameIn.value = i.ownName ?? ""; nameIn.placeholder = i.name; }).catch(() => {});
  let nameT = 0;
  nameIn.addEventListener("input", () => { clearTimeout(nameT); nameT = window.setTimeout(() => api.pcNameSet(nameIn.value).catch(() => {}), 500); });
  // Code wechselt nach dem Koppeln → beim Offenhalten der Einstellungen nachziehen
  const t = window.setInterval(() => { if (!document.body.contains(body)) clearInterval(t); else if (box.checked) fill(); }, 5000);
  return h("div", { class: "card" }, h("h3", {}, L("Minimal Player Connected (Handy im WLAN)")),
    h("div", { class: "setting" }, h("label", { class: "check" }, box, L("Bibliothek fürs Handy freigeben, solange Minimal Library läuft")), h("span", {})),
    h("div", { class: "setting" }, h("span", {}, L("Name dieses Computers"), h("div", { class: "note" }, L("So heißt er am Handy und unter „Läuft gerade im Haus“"))), nameIn),
    body,
    h("div", { class: "note" }, L("In Minimal Player Connected unter „Bibliothek“ diesen Computer wählen und einmal den Code eingeben. Danach siehst du am Handy alle Ordner und Playlisten, spielst direkt ab, bearbeitest Playlisten und machst Ordner oder Playlisten offline verfügbar. Neue Musik aus dem Hauptordner am Handy kommt automatisch hierher (je Handy eine eigene Musikquelle); sortierst du sie hier, zieht sie am Handy mit. Playlisten und Stern werden in beide Richtungen zusammengeführt. Nur im eigenen WLAN; Windows fragt beim ersten Mal evtl. nach der Firewall-Freigabe – bitte „Private Netzwerke“ erlauben.")),
  );
}

/** Pioneer DJ / AlphaTheta-Export: erst nach Einschalten gibt es Beatgrid-Analyse, Cues und Export */
function pioneerCard() {
  const on = store.settings.pioneer;
  const box = h("input", { type: "checkbox", checked: on }) as HTMLInputElement;
  box.addEventListener("change", async () => {
    await store.saveSettings({ pioneer: box.checked });
    if (box.checked) {
      toast(L("Pioneer-Export eingeschaltet – Beatgrids und Wellenformen werden im Hintergrund berechnet."));
      analyzeIds(null);
    }
  });
  return h("div", { class: "card" }, h("h3", {}, L("Export für Pioneer DJ / AlphaTheta-Player (OneLibrary)"), h("span", { class: "chip-exp" }, L("experimentell"))),
    h("div", { class: "setting" }, h("label", { class: "check" }, box, L("Einschalten")), h("span", {})),
    h("div", { class: "note" },
      L("Für CDJ-3000, CDJ-3000X, XDJ-AZ, OPUS-QUAD, OMNIS-DUO und weitere OneLibrary-Geräte. Eingeschaltet berechnet die Analyse zusätzlich Beatgrid und die Wellenformen der Player; "),
      L("im Inspektor lassen sich Grid und Cues korrigieren, Playlisten kommen per Rechtsklick „Für Pioneer-Player exportieren …“ auf den Stick. "),
      L("Dazu gibt es einen rekordbox-XML-Export. Alles andere in der App bleibt unverändert.")),
    on ? h("div", { class: "line" }, h("button", { class: "btn", onclick: () => analyzeIds(null) }, L("Fehlende Pioneer-Analysen nachholen"))) : "",
  );
}

/** NAS (Synology): Anmeldung im Browser – Passwort ändern, abmelden */
function nasCard() {
  const old = h("input", { type: "password", placeholder: L("Altes Passwort"), autocomplete: "current-password" }) as HTMLInputElement;
  const neu = h("input", { type: "password", placeholder: L("Neues Passwort"), autocomplete: "new-password" }) as HTMLInputElement;
  const change = async () => {
    const r = await fetch("/api/password", { method: "POST", credentials: "same-origin", headers: { "Content-Type": "application/json" }, body: JSON.stringify({ old: old.value, new: neu.value }) });
    const b = await r.json().catch(() => ({}));
    toast(r.ok ? L("Passwort geändert") : b.error ?? L("Ging nicht"));
    if (r.ok) { old.value = ""; neu.value = ""; }
  };
  return h("div", { class: "card" }, h("h3", {}, L("Synology NAS")),
    h("div", { class: "note" }, L("Minimal Library läuft auf deinem NAS. Musikordner wählst du aus den Freigaben – das Paket braucht dafür Lese-/Schreibrechte (Systemsteuerung → Gemeinsamer Ordner → Berechtigungen → Systeminterner Benutzer „MinimalLibrary“).")),
    h("div", { class: "setting" }, h("span", {}, L("Passwort ändern")), h("span", { style: { display: "flex", gap: "8px" } }, old, neu, h("button", { class: "btn", onclick: change }, L("Ändern")))),
    h("div", { class: "setting" }, h("span", {}, L("Abmelden (dieser Browser)")), h("button", { class: "btn", onclick: async () => { await fetch("/api/logout", { credentials: "same-origin" }); location.reload(); } }, L("Abmelden"))),
  );
}

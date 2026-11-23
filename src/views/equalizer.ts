import { api, type CastSound } from "../api";
import { BAND_LABELS, PRESETS, eq, type EqSettings } from "../eq";
import { cachedDevices, discoverIfStale, isFound, isSearching, labelsHidden, localOutputs, selectCast, selectLocal, unlockLabels } from "../outputs";
import type { CastDevice } from "../api";
import { player } from "../player";
import { store } from "../store";
import { clear, h, toast } from "../ui";
import { IS_MAC, OS } from "../platform";
import { L } from "../i18n";

export function equalizerView(): { el: HTMLElement; destroy: () => void } {
  const outBox = h("div", {});
  const eqBox = h("div", {});
  // Welches Gerät wird eingestellt? (Nur Auswahl zum Einstellen – die Wiedergabe wechselt man unten am Lautstärkeregler)
  let editing: { cast: CastDevice | null; sink: string } = { cast: player.device, sink: player.localSink };
  const pick = (e: typeof editing) => {
    editing = e;
    renderOutputs();
    renderEq();
  };

  const renderOutputs = async () => {
    const locals = await localOutputs();
    clear(outBox);
    const isPlaying = (local: boolean, id: string) => (local ? !player.device && player.localSink === id : player.device?.id === id);
    // Zeile antippen = Klang einstellen; rechts „Hier abspielen“ schaltet die Wiedergabe um
    const item = (label: string, sub: string, on: boolean, click: () => void, dim: boolean, local: boolean, play: () => void, id: string) =>
      h("div", { class: "out-item" + (on ? " on" : "") + (dim ? " dim" : ""), onclick: click },
        h("span", { class: "out-name" }, label, h("small", {}, sub)),
        isPlaying(local, id)
          ? h("span", { class: "out-live" }, L("● spielt hier"))
          : h("button", { class: "btn small", onclick: async (e: Event) => { e.stopPropagation(); await play(); click(); } }, L("Hier abspielen")));
    outBox.append(
      h("div", { class: "out-group" }, h("h3", {}, L("Dieser PC")),
        ...locals.map((o) => item(o.id === "default" ? L("Standard-Ausgabe") : o.label, (o.id === "default" ? L("wie in {0} eingestellt", OS) : L("Soundkarte")), !editing.cast && editing.sink === o.id, () => pick({ cast: null, sink: o.id }), false, true, () => selectLocal(o.id), o.id)),
        locals.length < 2 && IS_MAC ? h("div", { class: "note", style: { marginTop: "6px" } }, L("Weitere Ausgänge (Kopfhörer, USB, AirPlay) wählst du in den macOS-Einstellungen unter Ton.")) : "",
        labelsHidden() ? h("div", { class: "note", style: { marginTop: "6px" } }, L("Gerätenamen verborgen – "),
          h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); unlockLabels(); } }, L("Namen anzeigen")),
          L(" ({0} fragt einmalig nach dem Mikrofon, es wird nichts aufgenommen).", OS)) : "",
      ),
      h("div", { class: "out-group" }, h("h3", {}, L("Heimnetz"), isSearching() ? h("span", { class: "note" }, L("  · sucht …")) : ""),
        ...cachedDevices().map((d) => item(d.name, `${d.kind === "sonos" ? "Sonos" : "DLNA"}`, editing.cast?.id === d.id, () => pick({ cast: d, sink: editing.sink }), !isFound(d.id), false, () => selectCast(d), d.id)),
        cachedDevices().length ? "" : h("div", { class: "note" }, isSearching() ? L("Suche Geräte …") : L("Keine Geräte gefunden.")),
        h("button", { class: "btn", style: { marginTop: "6px" }, onclick: () => discoverIfStale(true) }, L("Neu suchen")),
      ),
    );
  };

  let soundTimer = 0;
  const renderEq = async () => {
    clear(eqBox);
    const dev = editing.cast;
    if (dev) {
      eqBox.append(h("h3", {}, L("Klang · {0}", dev.name)), h("div", { class: "note" }, L("Lädt …")));
      const sound: CastSound | null = await api.castSound(dev.id).catch(() => null);
      if (editing.cast?.id !== dev.id) return;
      clear(eqBox);
      eqBox.append(h("h3", {}, L("Klang · {0}", dev.name)));
      if (!sound || !sound.supported) {
        eqBox.append(h("div", { class: "note" }, L("Dieses Gerät bietet keine Klangregelung über das Netzwerk.")));
        return;
      }
      const send = () => {
        clearTimeout(soundTimer);
        soundTimer = window.setTimeout(() => api.castSetSound(dev.id, sound).catch((e) => toast(String(e))), 150);
      };
      const slider = (label: string, key: "bass" | "treble") => {
        const val = h("span", { class: "eq-val" }, String(sound[key]));
        const r = h("input", { type: "range", min: "-10", max: "10", step: "1", value: String(sound[key]) });
        r.addEventListener("input", () => {
          sound[key] = Number(r.value);
          val.textContent = (sound[key] > 0 ? "+" : "") + sound[key];
          send();
        });
        return h("div", { class: "setting" }, h("span", {}, label), h("div", { style: { display: "flex", gap: "10px", alignItems: "center", width: "60%" } }, r, val));
      };
      const loud = h("input", { type: "checkbox", checked: sound.loudness });
      loud.addEventListener("change", () => {
        sound.loudness = loud.checked;
        send();
      });
      eqBox.append(
        slider(L("Bass"), "bass"),
        slider(L("Höhen"), "treble"),
        h("div", { class: "setting" }, h("label", { class: "check" }, loud, L("Loudness (mehr Bass und Höhen bei leiser Wiedergabe)")), h("span", {})),
        h("div", { class: "note" }, dev.kind === "sonos" ? L("Sonos speichert die Einstellung selbst – sie gilt für alle Lautsprecher dieser Gruppe, auch in der Sonos-App.") : ""),
      );
      return;
    }

    // Lokal: 10-Band-Equalizer
    const sink = editing.sink;
    const key = "local:" + sink;
    const s: EqSettings = eq.load(key);
    const label = sink === "default" ? L("Standard-Ausgabe") : (await localOutputs()).find((o) => o.id === sink)?.label ?? L("Soundkarte");
    if (!eq.enabled) {
      const on = h("input", { type: "checkbox", checked: false });
      on.addEventListener("change", () => {
        localStorage.setItem("ml.audioGraph", on.checked ? "1" : "0");
        toast(L("Wird nach einem Neustart der App aktiv."));
      });
      eqBox.append(h("h3", {}, L("Equalizer · {0}", label)), h("div", { class: "line" }, h("label", { class: "check" }, on, L("Klangverarbeitung einschalten (Neustart nötig)"))));
      return;
    }
    const save = () => eq.save(key, s);
    const onBox = h("input", { type: "checkbox", checked: s.on });
    const sliders: HTMLInputElement[] = [];
    const vals: HTMLElement[] = [];
    const fmt = (v: number) => (v > 0 ? "+" : "") + v;
    const bands = h("div", { class: "eq-bands" },
      ...s.gains.map((g, i) => {
        const r = h("input", { type: "range", min: "-12", max: "12", step: "0.5", value: String(g), class: "vslider" });
        const v = h("div", { class: "eq-val" }, fmt(g));
        r.addEventListener("input", () => {
          s.gains[i] = Number(r.value);
          v.textContent = fmt(s.gains[i]);
          if (!s.on) {
            s.on = true;
            onBox.checked = true;
          }
          save();
        });
        r.addEventListener("dblclick", () => {
          r.value = "0";
          r.dispatchEvent(new Event("input"));
        });
        sliders.push(r);
        vals.push(v);
        return h("div", { class: "eq-band" }, v, h("div", { class: "vwrap" }, r), h("div", { class: "eq-freq" }, BAND_LABELS[i]));
      }),
    );
    const pre = h("input", { type: "range", min: "-12", max: "6", step: "0.5", value: String(s.preamp) });
    const preVal = h("span", { class: "eq-val", style: { minWidth: "64px", textAlign: "right", whiteSpace: "nowrap", flex: "none", fontVariantNumeric: "tabular-nums" } }, fmt(s.preamp) + " dB");
    pre.addEventListener("input", () => {
      s.preamp = Number(pre.value);
      preVal.textContent = fmt(s.preamp) + " dB";
      save();
    });
    onBox.addEventListener("change", () => {
      s.on = onBox.checked;
      save();
    });
    const presets = h("div", { class: "chips", style: { margin: "10px 0" } },
      ...Object.entries(PRESETS).map(([name, g]) =>
        h("span", { class: "chip", onclick: () => {
          s.gains = [...g];
          s.on = name !== "Flach";
          onBox.checked = s.on;
          // Vorverstärkung absenken, damit nichts übersteuert
          s.preamp = -Math.max(0, ...g) * 0.5;
          pre.value = String(s.preamp);
          preVal.textContent = fmt(s.preamp) + " dB";
          sliders.forEach((r, i) => {
            r.value = String(s.gains[i]);
            vals[i].textContent = fmt(s.gains[i]);
          });
          save();
        } }, L(name))),
    );
    eqBox.append(
      h("h3", {}, L("Equalizer · {0}", label)),
      h("div", { class: "line" }, h("label", { class: "check" }, onBox, L("Equalizer an")), h("span", { class: "spacer" }), h("span", { class: "note" }, L("Doppelklick auf einen Regler = 0"))),
      presets,
      bands,
      h("div", { class: "setting" }, h("span", {}, L("Vorverstärkung")), h("div", { style: { display: "flex", gap: "10px", alignItems: "center", width: "60%", flexWrap: "nowrap" } }, pre, preVal)),
      h("div", { class: "note" }, L("Jedes Ausgabegerät hat seine eigene Einstellung – z. B. mehr Bass für kleine Laptop-Lautsprecher, flach für die Anlage.")),
    );
  };

  const all = () => {
    renderOutputs();
    renderEq();
  };
  // Ausgabe unten gewechselt → das neue Gerät zum Einstellen vorwählen
  let lastDev = player.device?.id ?? "local:" + player.localSink;
  const unsubs = [store.on("outputs", renderOutputs), store.on("player", () => {
    const now = player.device?.id ?? "local:" + player.localSink;
    if (now !== lastDev) {
      lastDev = now;
      pick({ cast: player.device, sink: player.localSink });
    } else renderOutputs();
  })];
  all();
  discoverIfStale();

  const el = h("div", { style: { display: "contents" } },
    h("div", { class: "head" }, h("h1", {}, "Equalizer"), h("span", { class: "sub" }, L("Alle Ausgänge mit eigenem Klang – antippen zum Einstellen, „Hier abspielen“ zum Umschalten"))),
    h("div", { class: "view-scroll" },
      h("div", { class: "eq-layout" },
        h("div", { class: "card" }, h("h3", {}, L("Ausgänge")), outBox),
        h("div", { class: "card" }, eqBox),
      ),
    ),
  );
  return { el, destroy: () => unsubs.forEach((u) => u()) };
}

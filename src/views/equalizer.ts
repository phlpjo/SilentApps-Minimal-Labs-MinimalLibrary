import { api, type CastSound } from "../api";
import { BAND_LABELS, PRESETS, eq, type EqSettings } from "../eq";
import { cachedDevices, discoverIfStale, isFound, isSearching, labelsHidden, localOutputs, unlockLabels } from "../outputs";
import type { CastDevice } from "../api";
import { player } from "../player";
import { store } from "../store";
import { clear, h, toast } from "../ui";
import { OS } from "../platform";

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
    const item = (label: string, sub: string, on: boolean, click: () => void, dim = false) =>
      h("div", { class: "out-item" + (on ? " on" : "") + (dim ? " dim" : ""), onclick: click }, h("span", {}, label), h("small", {}, sub));
    const playing = (local: boolean, id: string) => (local ? !player.device && player.localSink === id : player.device?.id === id) ? " · spielt hier" : "";
    outBox.append(
      h("div", { class: "out-group" }, h("h3", {}, "Dieser PC"),
        ...locals.map((o) => item(o.id === "default" ? "Standard-Ausgabe" : o.label, (o.id === "default" ? `wie in ${OS} eingestellt` : "Soundkarte") + playing(true, o.id), !editing.cast && editing.sink === o.id, () => pick({ cast: null, sink: o.id }))),
        labelsHidden() ? h("div", { class: "note", style: { marginTop: "6px" } }, "Gerätenamen verborgen – ",
          h("a", { href: "#", style: { color: "inherit" }, onclick: (e: Event) => { e.preventDefault(); unlockLabels(); } }, "Namen anzeigen"),
          ` (${OS} fragt einmalig nach dem Mikrofon, es wird nichts aufgenommen).`) : "",
      ),
      h("div", { class: "out-group" }, h("h3", {}, "Heimnetz", isSearching() ? h("span", { class: "note" }, "  · sucht …") : ""),
        ...cachedDevices().map((d) => item(d.name, `${d.kind === "sonos" ? "Sonos" : "DLNA"}${playing(false, d.id)}`, editing.cast?.id === d.id, () => pick({ cast: d, sink: editing.sink }), !isFound(d.id))),
        cachedDevices().length ? "" : h("div", { class: "note" }, isSearching() ? "Suche Geräte …" : "Keine Geräte gefunden."),
        h("button", { class: "btn", style: { marginTop: "6px" }, onclick: () => discoverIfStale(true) }, "Neu suchen"),
      ),
    );
  };

  let soundTimer = 0;
  const renderEq = async () => {
    clear(eqBox);
    const dev = editing.cast;
    if (dev) {
      eqBox.append(h("h3", {}, `Klang · ${dev.name}`), h("div", { class: "note" }, "Lädt …"));
      const sound: CastSound | null = await api.castSound(dev.id).catch(() => null);
      if (editing.cast?.id !== dev.id) return;
      clear(eqBox);
      eqBox.append(h("h3", {}, `Klang · ${dev.name}`));
      if (!sound || !sound.supported) {
        eqBox.append(h("div", { class: "note" }, "Dieses Gerät bietet keine Klangregelung über das Netzwerk."));
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
        slider("Bass", "bass"),
        slider("Höhen", "treble"),
        h("div", { class: "setting" }, h("label", { class: "check" }, loud, "Loudness (mehr Bass und Höhen bei leiser Wiedergabe)"), h("span", {})),
        h("div", { class: "note" }, dev.kind === "sonos" ? "Sonos speichert die Einstellung selbst – sie gilt für alle Lautsprecher dieser Gruppe, auch in der Sonos-App." : ""),
      );
      return;
    }

    // Lokal: 10-Band-Equalizer
    const sink = editing.sink;
    const key = "local:" + sink;
    const s: EqSettings = eq.load(key);
    const label = sink === "default" ? "Standard-Ausgabe" : (await localOutputs()).find((o) => o.id === sink)?.label ?? "Soundkarte";
    if (!eq.enabled) {
      const on = h("input", { type: "checkbox", checked: false });
      on.addEventListener("change", () => {
        localStorage.setItem("ml.audioGraph", on.checked ? "1" : "0");
        toast("Wird nach einem Neustart der App aktiv.");
      });
      eqBox.append(h("h3", {}, `Equalizer · ${label}`), h("div", { class: "line" }, h("label", { class: "check" }, on, "Klangverarbeitung einschalten (Neustart nötig)")));
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
        } }, name)),
    );
    eqBox.append(
      h("h3", {}, `Equalizer · ${label}`),
      h("div", { class: "line" }, h("label", { class: "check" }, onBox, "Equalizer an"), h("span", { class: "spacer" }), h("span", { class: "note" }, "Doppelklick auf einen Regler = 0")),
      presets,
      bands,
      h("div", { class: "setting" }, h("span", {}, "Vorverstärkung"), h("div", { style: { display: "flex", gap: "10px", alignItems: "center", width: "60%", flexWrap: "nowrap" } }, pre, preVal)),
      h("div", { class: "note" }, "Jedes Ausgabegerät hat seine eigene Einstellung – z. B. mehr Bass für kleine Laptop-Lautsprecher, flach für die Anlage."),
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
    h("div", { class: "head" }, h("h1", {}, "Equalizer"), h("span", { class: "sub" }, "Klang je Ausgabegerät – wo du hörst, wählst du unten am Lautstärkeregler")),
    h("div", { class: "view-scroll" },
      h("div", { class: "eq-layout" },
        h("div", { class: "card" }, h("h3", {}, "Gerät zum Einstellen"), outBox),
        h("div", { class: "card" }, eqBox),
      ),
    ),
  );
  return { el, destroy: () => unsubs.forEach((u) => u()) };
}

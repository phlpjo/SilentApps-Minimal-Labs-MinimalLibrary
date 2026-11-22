// Equalizer für die Wiedergabe auf diesem PC (Web Audio), eigene Einstellung pro Ausgabegerät.

export const BANDS = [32, 64, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
export const BAND_LABELS = ["32", "64", "125", "250", "500", "1k", "2k", "4k", "8k", "16k"];

export interface EqSettings { on: boolean; preamp: number; gains: number[] }

export const PRESETS: Record<string, number[]> = {
  Flach: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
  "Bass +": [6, 5, 4, 2, 0, 0, 0, 0, 0, 0],
  "Höhen +": [0, 0, 0, 0, 0, 1, 2, 4, 5, 6],
  Loudness: [5, 4, 2, 0, -1, -1, 0, 2, 4, 5],
  Elektronisch: [5, 4, 1, 0, -2, 1, 0, 2, 4, 5],
  Akustik: [3, 2, 1, 1, 2, 2, 3, 3, 2, 1],
  Stimme: [-3, -2, -1, 1, 3, 4, 4, 3, 1, -1],
  "Kleine Lautsprecher": [7, 6, 4, 2, 0, 0, 0, 1, 2, 2],
};

const FLAT: EqSettings = { on: false, preamp: 0, gains: [0, 0, 0, 0, 0, 0, 0, 0, 0, 0] };

class Eq {
  private ctx: AudioContext | null = null;
  private pre!: GainNode;
  private filters: BiquadFilterNode[] = [];
  private sources = new WeakMap<HTMLMediaElement, MediaElementAudioSourceNode>();
  private pendingSink: string | null = null;
  private volGain!: GainNode;
  private vol = 1;
  /** Schlüssel des aktuellen Ausgabegeräts, z. B. "local:default" */
  key = "local:default";
  enabled = localStorage.getItem("ml.audioGraph") !== "0";

  load(key: string): EqSettings {
    try {
      const s = JSON.parse(localStorage.getItem("ml.eq." + key) ?? "null") as EqSettings | null;
      if (s && Array.isArray(s.gains) && s.gains.length === BANDS.length) return s;
    } catch {
      /* Standard */
    }
    return structuredClone(FLAT);
  }

  save(key: string, s: EqSettings) {
    localStorage.setItem("ml.eq." + key, JSON.stringify(s));
    if (key === this.key) this.apply(s);
  }

  private ensure() {
    if (this.ctx || !this.enabled) return;
    this.ctx = new AudioContext({ latencyHint: "playback" });
    this.pre = this.ctx.createGain();
    let node: AudioNode = this.pre;
    this.filters = BANDS.map((f, i) => {
      const b = this.ctx!.createBiquadFilter();
      b.type = i === 0 ? "lowshelf" : i === BANDS.length - 1 ? "highshelf" : "peaking";
      b.frequency.value = f;
      b.Q.value = 1.1;
      node.connect(b);
      node = b;
      return b;
    });
    // Lautstärke im Klangweg: WebKit (Mac) ignoriert audio.volume, sobald Web Audio dran hängt
    this.volGain = this.ctx.createGain();
    this.volGain.gain.value = this.vol;
    node.connect(this.volGain);
    this.volGain.connect(this.ctx.destination);
    this.apply(this.load(this.key));
    if (this.pendingSink !== null) this.setSink(this.pendingSink);
  }

  /** Audio-Element durch den Equalizer leiten (einmal pro Element) */
  attach(a: HTMLMediaElement) {
    if (!this.enabled) return;
    a.crossOrigin = "anonymous";
    a.addEventListener("play", () => {
      this.ensure();
      if (!this.ctx) return;
      if (!this.sources.has(a)) {
        try {
          const src = this.ctx.createMediaElementSource(a);
          src.connect(this.pre);
          this.sources.set(a, src);
        } catch {
          /* schon verbunden */
        }
      }
      if (this.ctx.state === "suspended") this.ctx.resume().catch(() => {});
    });
  }

  setVolume(v: number) {
    this.vol = v;
    if (this.ctx) this.volGain.gain.setTargetAtTime(v, this.ctx.currentTime, 0.015);
  }

  apply(s: EqSettings) {
    if (!this.ctx) return;
    const t = this.ctx.currentTime;
    this.pre.gain.setTargetAtTime(s.on ? Math.pow(10, s.preamp / 20) : 1, t, 0.02);
    this.filters.forEach((f, i) => f.gain.setTargetAtTime(s.on ? s.gains[i] : 0, t, 0.02));
  }

  /** Gerätewechsel: Einstellungen dieses Geräts laden */
  use(key: string) {
    this.key = key;
    this.apply(this.load(key));
  }

  /** Lokale Soundkarte wählen ("" = Windows-Standard) */
  async setSink(id: string, audio?: HTMLMediaElement) {
    const sink = id === "default" ? "" : id;
    if (!this.ctx) {
      this.pendingSink = id;
      // ohne Equalizer-Klangweg direkt am Element umschalten
      if (!this.enabled && audio && "setSinkId" in audio) await (audio as HTMLMediaElement & { setSinkId(id: string): Promise<void> }).setSinkId(sink).catch(() => {});
      return;
    }
    const c = this.ctx as AudioContext & { setSinkId?: (id: string) => Promise<void> };
    if (c.setSinkId) await c.setSinkId(sink).catch(() => {});
  }
}

export const eq = new Eq();

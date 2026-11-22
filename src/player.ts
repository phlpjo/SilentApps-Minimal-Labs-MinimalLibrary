import { convertFileSrc } from "@tauri-apps/api/core";
import { api, type CastDevice, type Track } from "./api";
import { store } from "./store";
import { toast } from "./ui";

type PosListener = (id: number, frac: number) => void;

/**
 * Wiedergabe: lokal über ein <audio>-Element oder im Heimnetz (Sonos/DLNA).
 * Wird die laufende Datei verschoben/umbenannt, läuft sie nahtlos weiter.
 */
class Player {
  audio: HTMLAudioElement;
  current: Track | null = null;
  queue: number[] = [];
  shuffle = localStorage.getItem("ml.shuffle") === "1";
  /** Gewähltes Netzwerkgerät (null = dieser PC) */
  device: CastDevice | null = null;
  private posListeners = new Set<PosListener>();
  private raf = 0;
  private token = 0;
  private shuffleBag: number[] = [];
  private history: number[] = [];
  private remote = { playing: false, pos: 0, at: 0, dur: 0, poll: 0, startedAt: 0, seekUntil: 0 };
  private volume: number;
  private volTimer = 0;

  constructor() {
    const vol = Number(localStorage.getItem("ml.volume") ?? "0.9");
    this.volume = isFinite(vol) ? vol : 0.9;
    this.audio = this.makeAudio();
    // Nach Verschieben/Umbenennen: neuen Pfad übernehmen, ohne anzuhalten
    store.on("tracks", () => this.follow());
  }

  private makeAudio() {
    const a = new Audio();
    a.preload = "auto";
    a.volume = this.volume;
    a.addEventListener("ended", () => a === this.audio && this.next(true));
    a.addEventListener("play", () => {
      if (a !== this.audio) return;
      this.loop();
      store.emit("player");
    });
    a.addEventListener("pause", () => a === this.audio && store.emit("player"));
    a.addEventListener("error", () => {
      if (a !== this.audio || !this.current || !a.getAttribute("src")) return;
      const ext = this.current.path.split(".").pop()?.toUpperCase();
      toast(`${ext}-Datei kann nicht abgespielt werden${ext === "AIFF" || ext === "AIF" ? " (AIFF unterstützt die Windows-WebView nicht)" : ""}.`);
      store.emit("player");
    });
    return a;
  }

  onPos(fn: PosListener) {
    this.posListeners.add(fn);
    return () => this.posListeners.delete(fn);
  }

  get isRemote() {
    return !!this.device;
  }

  get playing() {
    return this.isRemote ? this.remote.playing : !this.audio.paused;
  }

  get duration() {
    if (this.isRemote) return this.remote.dur || this.current?.duration || 0;
    return isFinite(this.audio.duration) && this.audio.duration > 0 ? this.audio.duration : this.current?.duration ?? 0;
  }

  get currentTime(): number {
    if (!this.isRemote) return this.audio.currentTime;
    const r = this.remote;
    return r.playing ? r.pos + (performance.now() - r.at) / 1000 : r.pos;
  }

  get volumeLevel() {
    return this.volume;
  }

  frac(): number {
    const d = this.duration;
    return d > 0 ? Math.min(1, this.currentTime / d) : 0;
  }

  private emitPos() {
    if (this.current) {
      const f = this.frac();
      this.posListeners.forEach((l) => l(this.current!.id, f));
    }
  }

  private loop() {
    cancelAnimationFrame(this.raf);
    const step = () => {
      this.emitPos();
      if (this.playing) this.raf = requestAnimationFrame(step);
    };
    this.raf = requestAnimationFrame(step);
  }

  /** Titel abspielen, optional ab Anteil (0..1) – Klick in die Wellenform. */
  async play(id: number, frac = 0, queue?: number[]) {
    const t = store.tracks.get(id);
    if (!t) return;
    if (queue) {
      this.queue = queue;
      this.shuffleBag = [];
    }
    if (this.current && this.current.id !== id) this.history.push(this.current.id);
    const my = ++this.token;

    if (this.isRemote) {
      // Gleicher Titel läuft schon auf dem Gerät → nur springen (kein Neustart)
      if (this.current?.id === id && this.remote.dur > 0) {
        this.seekFrac(frac);
        if (!this.remote.playing) this.toggle();
        return;
      }
      this.current = t;
      store.emit("player");
      try {
        const start = frac * (t.duration || 0);
        await api.castPlay(this.device!.id, id, start);
        if (my !== this.token) return;
        this.remote = { ...this.remote, playing: true, pos: start, at: performance.now(), dur: t.duration, startedAt: performance.now(), seekUntil: 0 };
        this.startPolling();
        this.loop();
      } catch (e) {
        toast(String(e));
      }
      store.emit("player");
      return;
    }

    if (this.current?.id === id && this.audio.getAttribute("src")) {
      this.seekFrac(frac);
      if (this.audio.paused) await this.audio.play().catch(() => {});
      return;
    }
    await api.allowPlay(t.path);
    if (my !== this.token) return;
    this.current = t;
    this.audio.src = convertFileSrc(t.path);
    if (frac > 0) {
      const onMeta = () => {
        this.audio.currentTime = frac * this.duration;
        this.audio.removeEventListener("loadedmetadata", onMeta);
      };
      this.audio.addEventListener("loadedmetadata", onMeta);
    }
    store.emit("player");
    await this.audio.play().catch(() => {});
  }

  /** Laufende Datei hat einen neuen Pfad → nahtlos auf die neue Datei umschalten */
  private async follow() {
    const cur = this.current;
    if (!cur) return;
    const t = store.tracks.get(cur.id);
    if (!t) return; // aus der Bibliothek entfernt: weiterspielen, solange es geht
    this.current = t;
    if (t.path === cur.path || this.isRemote) return;
    await api.allowPlay(t.path);
    const old = this.audio;
    const wasPlaying = !old.paused;
    const next = this.makeAudio();
    next.src = convertFileSrc(t.path);
    next.addEventListener(
      "loadedmetadata",
      () => {
        next.currentTime = old.currentTime; // inkl. der Zeit, die beim Laden vergangen ist
        const swap = () => {
          this.audio = next;
          old.pause();
          old.removeAttribute("src");
          old.load();
          store.emit("player");
          if (wasPlaying) this.loop();
        };
        if (wasPlaying) next.play().then(swap).catch(swap);
        else swap();
      },
      { once: true },
    );
  }

  seekFrac(frac: number) {
    if (!this.current) return;
    const t = Math.max(0, Math.min(0.999, frac)) * this.duration;
    if (this.isRemote) {
      api.castSeek(this.device!.id, t).catch((e) => toast(String(e)));
      this.remote.pos = t;
      this.remote.at = performance.now();
      this.remote.seekUntil = performance.now() + 2500; // Abfragen kurz ignorieren, bis das Gerät gesprungen ist
    } else this.audio.currentTime = t;
    this.emitPos();
  }

  seekBy(sec: number) {
    if (!this.current) return;
    this.seekFrac((this.currentTime + sec) / (this.duration || 1));
  }

  toggle() {
    if (!this.current) {
      const first = this.shuffle ? this.pickRandom() : this.queue[0];
      if (first !== undefined) this.play(first);
      return;
    }
    if (this.isRemote) {
      const dev = this.device!.id;
      if (this.remote.playing) {
        this.remote.pos = this.currentTime;
        this.remote.playing = false;
        api.castPause(dev).catch((e) => toast(String(e)));
      } else {
        this.remote.playing = true;
        this.remote.at = performance.now();
        api.castResume(dev).catch((e) => toast(String(e)));
        this.loop();
      }
      store.emit("player");
      return;
    }
    if (this.audio.paused) this.audio.play().catch(() => {});
    else this.audio.pause();
  }

  private pickRandom(): number | undefined {
    if (!this.shuffleBag.length) {
      this.shuffleBag = this.queue.filter((id) => id !== this.current?.id);
      for (let i = this.shuffleBag.length - 1; i > 0; i--) {
        const j = Math.floor(Math.random() * (i + 1));
        [this.shuffleBag[i], this.shuffleBag[j]] = [this.shuffleBag[j], this.shuffleBag[i]];
      }
    }
    return this.shuffleBag.pop();
  }

  next(auto = false) {
    if (!this.current) return;
    const n = this.shuffle ? this.pickRandom() : this.queue[this.queue.indexOf(this.current.id) + 1];
    if (n !== undefined) this.play(n);
    else {
      if (auto) this.remote.playing = false;
      store.emit("player");
    }
  }

  prev() {
    if (!this.current) return;
    if (this.currentTime > 3) {
      this.seekFrac(0);
      return;
    }
    const p = this.shuffle ? this.history.pop() : this.queue[this.queue.indexOf(this.current.id) - 1];
    if (p === undefined) return;
    const pushedBefore = this.history.length;
    this.play(p).then(() => {
      // Zurück soll die Verlaufsliste nicht wieder auffüllen
      if (this.shuffle && this.history.length > pushedBefore) this.history.pop();
    });
  }

  setShuffle(on: boolean) {
    this.shuffle = on;
    this.shuffleBag = [];
    localStorage.setItem("ml.shuffle", on ? "1" : "0");
    store.emit("player");
  }

  setVolume(v: number) {
    this.volume = v;
    this.audio.volume = v;
    localStorage.setItem("ml.volume", String(v));
    if (this.isRemote) {
      clearTimeout(this.volTimer);
      const dev = this.device!.id;
      this.volTimer = window.setTimeout(() => api.castVolume(dev, Math.round(v * 100)).catch(() => {}), 120);
    }
  }

  // ---------- Heimnetz ----------

  /** Ausgabegerät wechseln (null = dieser PC). Der laufende Titel wechselt an der gleichen Stelle mit. */
  async setDevice(dev: CastDevice | null) {
    const wasPlaying = this.playing;
    const id = this.current?.id;
    const frac = this.frac();
    if (this.isRemote) {
      clearInterval(this.remote.poll);
      api.castStop(this.device!.id).catch(() => {});
      this.remote.playing = false;
    } else {
      this.audio.pause();
    }
    this.device = dev;
    if (dev) {
      const st = await api.castStatus(dev.id).catch(() => null);
      if (st && st.volume >= 0) this.volume = st.volume / 100;
    } else {
      this.volume = Number(localStorage.getItem("ml.volume") ?? "0.9");
      this.audio.volume = this.volume;
    }
    store.emit("player");
    if (id !== undefined && wasPlaying) {
      if (!dev) this.audio.removeAttribute("src");
      this.current = null; // erzwingt Neustart auf dem neuen Gerät
      await this.play(id, frac);
    }
  }

  private startPolling() {
    clearInterval(this.remote.poll);
    this.remote.poll = window.setInterval(async () => {
      if (!this.device) return clearInterval(this.remote.poll);
      const st = await api.castStatus(this.device.id).catch(() => null);
      if (!st) return;
      const r = this.remote;
      // Kurz nach dem Start melden Geräte oft noch STOPPED/TRANSITIONING
      const settling = performance.now() - r.startedAt < 4000;
      if (performance.now() < r.seekUntil) return;
      if (st.state === "PLAYING") {
        if (!r.playing) {
          r.playing = true;
          this.loop();
          store.emit("player");
        }
        r.pos = st.position;
        r.at = performance.now();
        if (st.duration > 0) r.dur = st.duration;
      } else if (st.state === "PAUSED_PLAYBACK") {
        if (r.playing) {
          r.playing = false;
          store.emit("player");
        }
        r.pos = st.position;
      } else if (st.state === "STOPPED" && r.playing && !settling) {
        r.playing = false;
        this.next(true); // Titel zu Ende
      }
    }, 1000);
  }

  /** Datei wird gelöscht → Wiedergabe lösen */
  release(ids: number[]) {
    if (this.current && ids.includes(this.current.id)) {
      if (this.isRemote) api.castStop(this.device!.id).catch(() => {});
      this.audio.pause();
      this.audio.removeAttribute("src");
      this.audio.load();
      this.current = null;
      this.remote.playing = false;
      store.emit("player");
    }
  }
}

export const player = new Player();

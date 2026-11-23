import { convertFileSrc } from "@tauri-apps/api/core";
import { api, type CastDevice, type Track } from "./api";
import { store } from "./store";
import { toast } from "./ui";
import { eq } from "./eq";
import { claimAudio, onOtherTabPlays } from "./tabs";
import { L } from "./i18n";

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
  /** Soundkarte dieses PCs ("default" = Windows-Standard) */
  localSink = localStorage.getItem("ml.localSink") ?? "default";
  private posListeners = new Set<PosListener>();
  private raf = 0;
  private token = 0;
  private onDemandTimer = 0;
  private shuffleBag: number[] = [];
  private history: number[] = [];
  /**
   * Netzwerk-Wiedergabe: started = Gerät hat bestätigt, dass es unseren Titel an der gewünschten Stelle spielt.
   * Bis dahin steht die Zeit still (sonst läuft Leiste/Songtext dem Lautsprecher davon).
   */
  private remote = { playing: false, started: false, pos: 0, at: 0, dur: 0, poll: 0, startedAt: 0, seekUntil: 0, target: 0, key: "", loading: false, pollTick: 0 };
  private volume: number;
  private volTimer = 0;
  private endGuard = 0;

  constructor() {
    const vol = Number(localStorage.getItem("ml.volume") ?? "0.9");
    this.volume = isFinite(vol) ? vol : 0.9;
    this.audio = this.makeAudio();
    // Anderer Tab spielt jetzt → hier aufhören
    onOtherTabPlays((device) => this.yieldToOtherTab(device));
    // Nach Verschieben/Umbenennen: neuen Pfad übernehmen, ohne anzuhalten
    store.on("tracks", () => this.follow());
  }

  private makeAudio() {
    const a = new Audio();
    a.preload = "auto";
    eq.attach(a);
    this.applyVolume(a); // Equalizer (Web Audio) – setzt crossOrigin vor dem ersten src
    a.addEventListener("ended", () => a === this.audio && this.next(true));
    a.addEventListener("play", () => {
      if (a !== this.audio) return;
      claimAudio("local:" + this.localSink);
      this.loop();
      store.emit("player");
    });
    a.addEventListener("pause", () => a === this.audio && store.emit("player"));
    a.addEventListener("loadedmetadata", () => a.crossOrigin && (corsOk = true));
    a.addEventListener("error", () => {
      if (a !== this.audio || !this.current || !a.getAttribute("src")) return;
      // WebView lässt Web Audio (crossOrigin) nicht zu → ohne Equalizer weiter abspielen
      if (a.crossOrigin && !corsOk && eq.enabled) {
        // Nur wenn es ohne crossOrigin klappt, liegt es an Web Audio – sonst ist die Datei das Problem
        eq.enabled = false;
        this.applyVolume(a);
        const src = a.getAttribute("src")!;
        a.removeAttribute("crossorigin");
        a.addEventListener("loadedmetadata", () => {
          try { localStorage.setItem("ml.audioGraph", "0"); } catch {}
          toast(L("Equalizer ist auf diesem System nicht verfügbar – Wiedergabe läuft ohne."));
        }, { once: true });
        a.addEventListener("error", () => { eq.enabled = true; this.applyVolume(a); }, { once: true });
        a.src = src;
        a.play().catch(() => {});
        return;
      }
      const ext = this.current.path.split(".").pop()?.toUpperCase();
      toast(L("{0}-Datei kann nicht abgespielt werden{1}.", ext, ext === "AIFF" || ext === "AIF" ? L(" (AIFF unterstützt die Windows-WebView nicht)") : ""));
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
    return r.playing && r.started ? r.pos + (performance.now() - r.at) / 1000 : r.pos;
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
      // Während das Gerät umschaltet, meldet es STOPPED – das darf nicht als „Titel zu Ende“ zählen
      this.remote.loading = true;
      this.remote.startedAt = performance.now();
      store.emit("player");
      try {
        const start = frac * (t.duration || 0);
        this.remote.pos = start;
        this.remote.started = false;
        const key = await api.castPlay(this.device!.id, id, start);
        if (my !== this.token) return;
        claimAudio(this.device!.id);
        this.remote = { ...this.remote, playing: true, started: false, pos: start, target: start, key, at: performance.now(), dur: t.duration, startedAt: performance.now(), seekUntil: 0, loading: false };
        this.startPolling();
        this.loop();
      } catch (e) {
        if (my === this.token) {
          this.remote.loading = false;
          this.remote.playing = false;
          toast(String(e));
        }
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
    // Cloud-Ordner: Wellenform & Co. erst holen, wenn der Titel wirklich gehört wird (nicht beim Durchskippen)
    clearTimeout(this.onDemandTimer);
    if (!t.analyzed && store.isOnDemand(t.path)) {
      this.onDemandTimer = window.setTimeout(() => {
        if (this.current?.id === t.id && !store.tracks.get(t.id)?.analyzed) api.analyzeQuiet([t.id]).catch(() => {});
      }, 12000);
    }
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
      // bis das Gerät an der neuen Stelle spielt: Zeit anhalten, alte Positionsmeldungen ignorieren
      this.remote.started = false;
      this.remote.target = t;
      this.remote.seekUntil = performance.now() + 1200;
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
        this.remote.started = true;
        api.castPause(dev).catch((e) => toast(String(e)));
      } else {
        this.remote.playing = true;
        this.remote.at = performance.now();
        api.castResume(dev).catch((e) => toast(String(e)));
        claimAudio(dev);
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

  /** Wiederholen: aus, einzelner Titel, ganze Liste */
  repeat: "off" | "one" | "all" = (localStorage.getItem("ml.repeat") as "off" | "one" | "all") ?? "off";
  cycleRepeat() {
    this.repeat = this.repeat === "off" ? "all" : this.repeat === "all" ? "one" : "off";
    localStorage.setItem("ml.repeat", this.repeat);
    store.emit("player");
  }

  next(auto = false) {
    if (!this.current) return;
    if (auto && this.repeat === "one") {
      const id = this.current.id;
      if (this.isRemote) this.current = null; // Gerät neu starten
      this.play(id, 0);
      return;
    }
    let n = this.shuffle ? this.pickRandom() : this.queue[this.queue.indexOf(this.current.id) + 1];
    if (n === undefined && this.repeat === "all" && this.queue.length) n = this.queue[0];
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

  /** Mit Equalizer regelt der Klangweg die Lautstärke, sonst das Audio-Element */
  private applyVolume(a: HTMLMediaElement) {
    if (eq.enabled) {
      a.volume = 1;
      eq.setVolume(this.volume);
    } else {
      a.volume = this.volume;
      eq.setVolume(1);
    }
  }

  setVolume(v: number) {
    this.volume = v;
    this.applyVolume(this.audio);
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
    eq.use(dev ? "cast:" + dev.id : "local:" + this.localSink);
    if (dev) {
      const st = await api.castStatus(dev.id).catch(() => null);
      if (st && st.volume >= 0) this.volume = st.volume / 100;
    } else {
      this.volume = Number(localStorage.getItem("ml.volume") ?? "0.9");
      this.applyVolume(this.audio);
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
    clearInterval(this.endGuard);
    // Sicherheitsnetz: Meldet das Gerät das Ende nicht, nach Ablauf der Spielzeit selbst weiterschalten
    this.endGuard = window.setInterval(() => {
      const r = this.remote;
      const dur = r.dur || this.current?.duration || 0;
      if (this.device && r.playing && r.started && dur > 0 && this.currentTime > dur + 6) {
        r.playing = false;
        this.next(true);
      }
    }, 1000);
    this.remote.poll = window.setInterval(async () => {
      if (!this.device) return clearInterval(this.remote.poll);
      if (this.remote.loading) return; // Gerät schaltet gerade um – nicht zusätzlich abfragen
      // kurz vor Titelende öfter nachsehen → schneller zum nächsten Titel
      const left = (this.remote.dur || this.current?.duration || 0) - this.currentTime;
      if (left > 3 && (this.remote.pollTick = (this.remote.pollTick + 1) % 2) === 1) return;
      const st = await api.castStatus(this.device.id).catch(() => null);
      if (!st || !this.device) return;
      const r = this.remote;
      const now = performance.now();
      if (r.loading || now < r.seekUntil) return;
      // Spielt das Gerät (noch) nicht unsere Datei? Kurz nach dem Start normal – später hat jemand anderes übernommen
      const ours = !r.key || !st.track || st.track === r.key;
      if (!ours) {
        if (now - r.startedAt > 10000 && st.state === "PLAYING" && r.playing) {
          r.playing = false;
          store.emit("player");
        }
        return;
      }
      const dur = r.dur || this.current?.duration || 0;
      if (st.duration > 0) r.dur = st.duration;
      if (st.state === "PLAYING") {
        if (!r.started) {
          // erst übernehmen, wenn das Gerät an der gewünschten Stelle angekommen ist
          if (st.position + 1 < r.target || st.position > r.target + 8) return;
          r.started = true;
          r.pos = Math.max(r.target, st.position);
          r.at = now;
          if (!r.playing) r.playing = true;
          this.loop();
          store.emit("player");
          return;
        }
        if (!r.playing) {
          r.playing = true;
          r.pos = st.position;
          r.at = now;
          this.loop();
          store.emit("player");
          return;
        }
        // Geräte melden ganze Sekunden: nur bei echter Abweichung nachziehen → Leiste und Songtext laufen ruhig
        const est = this.currentTime;
        if (est < st.position - 0.3 || est > st.position + 1.6) {
          r.pos = st.position + 0.5;
          r.at = now;
        }
        return;
      }
      if (!r.playing || !r.started) return;
      const est = this.currentTime;
      const nearEnd = dur > 0 && (est >= dur - 8 || st.position >= dur - 3);
      if (st.state === "STOPPED" || st.state === "NO_MEDIA_PRESENT" || (st.state === "PAUSED_PLAYBACK" && nearEnd)) {
        if (nearEnd || dur <= 0) {
          r.playing = false;
          this.next(true);
        } else if (st.state === "STOPPED") {
          // am Gerät oder in der Sonos-App gestoppt
          r.pos = est;
          r.playing = false;
          store.emit("player");
        }
      } else if (st.state === "PAUSED_PLAYBACK") {
        r.pos = st.position;
        r.playing = false;
        store.emit("player");
      }
    }, 400);
  }

  /** Ausgabe auf eine Soundkarte dieses PCs legen (beendet ggf. das Streamen ins Heimnetz) */
  async setLocalSink(id: string) {
    this.localSink = id;
    localStorage.setItem("ml.localSink", id);
    if (this.isRemote) await this.setDevice(null);
    await eq.setSink(id, this.audio);
    eq.use("local:" + id);
    store.emit("player");
  }

  /** Ein anderer Tab hat die Wiedergabe übernommen */
  /** Ein anderer Tab spielt jetzt – nur weichen, wenn er denselben Ausgang nimmt (verschiedene Ausgänge laufen parallel) */
  private yieldToOtherTab(device: string) {
    const mine = this.device ? this.device.id : "local:" + this.localSink;
    const same = (a: string, b: string) => a === b || (a.startsWith("local:") && b === "local") || (b.startsWith("local:") && a === "local");
    if (!same(device, mine)) return;
    if (this.isRemote) {
      clearInterval(this.remote.poll);
      // Das Gerät spielt jetzt den Titel des anderen Tabs – nicht stoppen, nur loslassen
      this.remote.playing = false;
      this.remote.loading = false;
    } else if (!this.audio.paused) {
      this.audio.pause();
    }
    store.emit("player");
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

/** Hat schon eine Datei mit crossOrigin geladen? (dann ist ein Fehler ein echter Dateifehler) */
let corsOk = false;

export const player = new Player();

import { convertFileSrc } from "@tauri-apps/api/core";
import { api, type Track } from "./api";
import { store } from "./store";
import { toast } from "./ui";

type PosListener = (id: number, frac: number) => void;

class Player {
  audio = new Audio();
  current: Track | null = null;
  queue: number[] = [];
  private posListeners = new Set<PosListener>();
  private raf = 0;
  private token = 0;

  constructor() {
    this.audio.preload = "auto";
    const vol = Number(localStorage.getItem("ml.volume") ?? "0.9");
    this.audio.volume = isFinite(vol) ? vol : 0.9;
    this.audio.addEventListener("ended", () => this.next());
    this.audio.addEventListener("play", () => {
      this.loop();
      store.emit("player");
    });
    this.audio.addEventListener("pause", () => store.emit("player"));
    this.audio.addEventListener("error", () => {
      if (!this.current) return;
      const ext = this.current.path.split(".").pop()?.toUpperCase();
      toast(`${ext}-Datei kann nicht abgespielt werden${ext === "AIFF" || ext === "AIF" ? " (AIFF unterstützt die Windows-WebView nicht)" : ""}.`);
      store.emit("player");
    });
  }

  onPos(fn: PosListener) {
    this.posListeners.add(fn);
    return () => this.posListeners.delete(fn);
  }

  get playing() {
    return !this.audio.paused;
  }

  get duration() {
    return isFinite(this.audio.duration) && this.audio.duration > 0 ? this.audio.duration : this.current?.duration ?? 0;
  }

  frac(): number {
    const d = this.duration;
    return d > 0 ? this.audio.currentTime / d : 0;
  }

  private loop() {
    cancelAnimationFrame(this.raf);
    const step = () => {
      if (this.current) {
        const f = this.frac();
        this.posListeners.forEach((l) => l(this.current!.id, f));
      }
      if (!this.audio.paused) this.raf = requestAnimationFrame(step);
    };
    this.raf = requestAnimationFrame(step);
  }

  /** Titel abspielen, optional ab Anteil (0..1) – Klick in die Wellenform. */
  async play(id: number, frac = 0, queue?: number[]) {
    const t = store.tracks.get(id);
    if (!t) return;
    if (queue) this.queue = queue;
    const my = ++this.token;
    if (this.current?.id === id && this.audio.src) {
      this.seekFrac(frac);
      if (this.audio.paused) await this.audio.play().catch(() => {});
      return;
    }
    await api.allowPlay(t.path);
    if (my !== this.token) return;
    this.current = t;
    this.audio.src = convertFileSrc(t.path);
    const startAt = frac * (t.duration || 0);
    if (startAt > 0) {
      // Startposition sofort setzen, sobald Metadaten da sind
      const onMeta = () => {
        this.audio.currentTime = frac * this.duration;
        this.audio.removeEventListener("loadedmetadata", onMeta);
      };
      this.audio.addEventListener("loadedmetadata", onMeta);
    }
    store.emit("player");
    await this.audio.play().catch(() => {});
  }

  seekFrac(frac: number) {
    if (!this.current) return;
    this.audio.currentTime = Math.max(0, Math.min(0.999, frac)) * this.duration;
    this.posListeners.forEach((l) => l(this.current!.id, this.frac()));
  }

  seekBy(sec: number) {
    if (!this.current) return;
    this.audio.currentTime = Math.max(0, Math.min(this.duration - 0.5, this.audio.currentTime + sec));
    this.posListeners.forEach((l) => l(this.current!.id, this.frac()));
  }

  toggle() {
    if (!this.current) {
      const first = this.queue[0];
      if (first !== undefined) this.play(first);
      return;
    }
    if (this.audio.paused) this.audio.play().catch(() => {});
    else this.audio.pause();
  }

  next() {
    if (!this.current) return;
    const i = this.queue.indexOf(this.current.id);
    const n = this.queue[i + 1];
    if (n !== undefined) this.play(n);
    else store.emit("player");
  }

  prev() {
    if (!this.current) return;
    if (this.audio.currentTime > 3) {
      this.audio.currentTime = 0;
      return;
    }
    const i = this.queue.indexOf(this.current.id);
    const p = this.queue[i - 1];
    if (p !== undefined) this.play(p);
  }

  setVolume(v: number) {
    this.audio.volume = v;
    localStorage.setItem("ml.volume", String(v));
  }

  /** Datei wird gleich verändert/verschoben → Wiedergabe lösen */
  release(ids: number[]) {
    if (this.current && ids.includes(this.current.id)) {
      this.audio.pause();
      this.audio.removeAttribute("src");
      this.audio.load();
      this.current = null;
      store.emit("player");
    }
  }
}

export const player = new Player();

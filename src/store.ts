import { api, type Playlist, type Settings, type Track } from "./api";

type Listener = () => void;

class Store {
  tracks = new Map<number, Track>();
  peaks = new Map<number, Uint8Array>();
  playlists: Playlist[] = [];
  settings: Settings = { roots: [], playlistDir: null, organizeTarget: null, pattern: "", threads: 4, autoAnalyze: true, acoustidKey: "", ripDir: null, ripFormat: "mp3", closeToTray: true, pioneer: false, noCrossDup: [], remoteOff: false, phoneDir: null };
  /** Auswahl in der aktuellen Liste */
  selection = new Set<number>();
  /** Musikordner „nur bei Bedarf“ (Cloud-Laufwerk): Analyse erst beim Abspielen/Export */
  onDemand: string[] = [];
  private listeners = new Map<string, Set<Listener>>();
  private pendingPeaks = new Set<number>();
  private peakTimer = 0;

  on(ev: string, fn: Listener) {
    if (!this.listeners.has(ev)) this.listeners.set(ev, new Set());
    this.listeners.get(ev)!.add(fn);
    return () => this.listeners.get(ev)?.delete(fn);
  }
  emit(ev: string) {
    this.listeners.get(ev)?.forEach((f) => f());
  }

  async loadTracks() {
    const list = await api.listTracks();
    this.tracks = new Map(list.map((t) => [t.id, t]));
    for (const id of [...this.selection]) if (!this.tracks.has(id)) this.selection.delete(id);
    this.emit("tracks");
  }

  async refreshTracks(ids: number[]) {
    if (!ids.length) return;
    const list = await api.getTracks(ids);
    const found = new Set(list.map((t) => t.id));
    for (const t of list) this.tracks.set(t.id, t);
    for (const id of ids) {
      if (!found.has(id)) this.tracks.delete(id);
      this.peaks.delete(id);
    }
    this.emit("tracks");
  }

  async loadPlaylists() {
    this.playlists = await api.playlists();
    this.emit("playlists");
  }

  async loadSettings() {
    this.settings = await api.getSettings();
    this.onDemand = await api.onDemandRoots().catch(() => []);
    this.emit("settings");
  }

  async saveSettings(patch: Partial<Settings>) {
    this.settings = { ...this.settings, ...patch };
    const pruned = await api.setSettings(this.settings);
    this.onDemand = await api.onDemandRoots().catch(() => []);
    this.emit("settings");
    if (pruned > 0) {
      // Musikordner entfernt: Titel daraus sind weg, die laufende Analyse wurde gestoppt
      for (const id of [...this.selection]) this.selection.delete(id);
      await this.loadTracks();
      await this.loadPlaylists();
      this.emit("pruned");
    }
  }

  /** Wellenform anfordern (gebündelt). Gibt vorhandene sofort zurück. */
  isOnDemand(path: string): boolean {
    const p = path.replace(/\\/g, "/").toLowerCase();
    return this.onDemand.some((r) => {
      const root = r.replace(/\\/g, "/").replace(/\/+$/, "").toLowerCase();
      return p === root || p.startsWith(root + "/");
    });
  }

  peaksFor(id: number): Uint8Array | undefined {
    const p = this.peaks.get(id);
    if (p) return p;
    const t = this.tracks.get(id);
    if (t && t.analyzed && !t.analysisError) {
      this.pendingPeaks.add(id);
      if (!this.peakTimer) this.peakTimer = window.setTimeout(() => this.fetchPeaks(), 30);
    }
    return undefined;
  }

  private async fetchPeaks() {
    this.peakTimer = 0;
    // in Paketen zu 200 laden – der Rest bleibt vorgemerkt (vorher fiel er weg: lange Listen ohne Wellenform)
    const all = [...this.pendingPeaks].filter((id) => !this.peaks.has(id));
    const ids = all.slice(0, 200);
    this.pendingPeaks = new Set(all.slice(200));
    if (!ids.length) return;
    const res = await api.getPeaks(ids).catch(() => ({}) as Record<string, string>);
    if (this.pendingPeaks.size && !this.peakTimer) this.peakTimer = window.setTimeout(() => this.fetchPeaks(), 0);
    for (const [id, b64] of Object.entries(res)) {
      const bin = atob(b64);
      const arr = new Uint8Array(bin.length);
      for (let i = 0; i < bin.length; i++) arr[i] = bin.charCodeAt(i);
      this.peaks.set(Number(id), arr);
    }
    this.emit("peaks");
  }

  label(t: Track): string {
    return t.title ?? t.path.split(/[\\/]/).pop() ?? "";
  }
}

export const store = new Store();

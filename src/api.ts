import { invoke } from "@tauri-apps/api/core";

export interface Track {
  id: number;
  path: string;
  size: number;
  mtime: number;
  added: number;
  title: string | null;
  artist: string | null;
  album: string | null;
  albumArtist: string | null;
  genre: string | null;
  year: string | null;
  trackNo: number | null;
  discNo: number | null;
  comment: string | null;
  duration: number;
  bitrate: number | null;
  sampleRate: number | null;
  channels: number | null;
  hasCover: boolean;
  bpm: number | null;
  key: string | null;
  analyzed: boolean;
  analysisError: string | null;
}

export interface Settings {
  roots: string[];
  playlistDir: string | null;
  organizeTarget: string | null;
  pattern: string;
  threads: number;
  autoAnalyze: boolean;
  acoustidKey: string;
  ripDir: string | null;
  ripFormat: "mp3" | "original";
  closeToTray: boolean;
}

export interface ScanReport { found: number; added: number; updated: number; removed: number; errors: string[] }

export interface TagUpdate {
  title?: string; artist?: string; album?: string; albumArtist?: string; genre?: string; year?: string;
  trackNo?: string; discNo?: string; comment?: string; bpm?: string; key?: string;
}

export interface DupGroup { trackIds: number[]; reasons: string[]; audioSimilarity: number | null; keep: number }

export interface PlannedMove { trackId: number; from: string; to: string; renamed: boolean; unchanged: boolean }
export interface ApplyReport { batchId: number; moved: number; errors: string[] }
export interface Batch { id: number; kind: "move" | "tags" | "trash" | "dir"; created: number; description: string; count: number; undone: boolean }
export interface UndoReport { restored: number; errors: string[]; newIds: number[] }
export interface LibraryChange { added: number[]; updated: number; moved: number; removed: number }

export interface Rule { field: string; op: string; value: string; value2: string }
export interface SmartRules { all: boolean; rules: Rule[]; sort: string; desc: boolean; limit: number | null }
export interface Playlist { id: number; name: string; rules: SmartRules | null; count: number; duration: number }

export interface Candidate {
  score: number; title: string; artist: string; album: string | null; albumArtist: string | null; year: string | null;
  trackNo: number | null; discNo: number | null; genre: string | null; releaseId: string | null; length: number | null;
}

export interface DirEntry { name: string; path: string; hasChildren: boolean; tracks: number }

export interface CastDevice { id: string; name: string; kind: "sonos" | "dlna"; model: string }
export interface CastStatus { state: string; position: number; duration: number; volume: number }

export interface ToolStatus { ytdlp: string | null; ffmpeg: boolean; dir: string }
export interface RipEntry { index: number; artist: string; title: string; album: string | null; duration: number | null; source: string; site: string; inLibrary: [number, string] | null }
export interface RipEvent { index: number; stage: "download" | "convert" | "done" | "error"; percent: number; message: string; trackId: number | null }

export type Layout = "numbered" | "flat" | "structured";
export interface ExportReport { copied: number; skipped: number; bytes: number; playlists: string[]; errors: string[] }

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  setSettings: (settings: Settings) => invoke<void>("set_settings", { settings }),
  scan: () => invoke<ScanReport>("scan_library"),
  addPaths: (paths: string[]) => invoke<number[]>("add_paths", { paths }),
  listTracks: () => invoke<Track[]>("list_tracks"),
  getTracks: (ids: number[]) => invoke<Track[]>("get_tracks", { ids }),
  getPeaks: (ids: number[]) => invoke<Record<string, string>>("get_peaks", { ids }),
  allowPlay: (path: string) => invoke<void>("allow_play", { path }),
  analyze: (ids: number[] | null) => invoke<number>("analyze", { ids }),
  cancel: () => invoke<void>("cancel_task"),
  removeFromLibrary: (ids: number[]) => invoke<void>("remove_from_library", { ids }),
  trash: (ids: number[], replace: Record<string, number> | null = null) => invoke<string[]>("trash_tracks", { ids, replace }),
  writeTags: (ids: number[], update: TagUpdate) => invoke<string[]>("write_tags", { ids, update }),
  writeAnalysisToTags: (ids: number[]) => invoke<string[]>("write_analysis_to_tags", { ids }),
  getCover: (id: number) => invoke<string | null>("get_cover", { id }),
  setCoverFile: (ids: number[], file: string) => invoke<string[]>("set_cover_file", { ids, file }),
  removeCover: (ids: number[]) => invoke<string[]>("remove_cover", { ids }),
  mbSearch: (artist: string, title: string, album: string | null) => invoke<Candidate[]>("mb_search", { artist, title, album }),
  acoustidLookup: (id: number) => invoke<Candidate[]>("acoustid_lookup", { id }),
  mbCover: (ids: number[], releaseId: string) => invoke<string[]>("mb_cover", { ids, releaseId }),
  findDuplicates: (modes: { metadata: boolean; filename: boolean; audio: boolean }) => invoke<DupGroup[]>("find_duplicates", { modes }),
  organizePreview: (ids: number[] | null, target: string, pattern: string) => invoke<PlannedMove[]>("organize_preview", { ids, target, pattern }),
  organizeApply: (plan: PlannedMove[], copy: boolean, description: string) => invoke<ApplyReport>("organize_apply", { plan, copy, description }),
  moveBatches: () => invoke<Batch[]>("move_batches"),
  undoBatch: (id: number | null) => invoke<UndoReport>("undo_batch", { id }),
  playlists: () => invoke<Playlist[]>("playlists_list"),
  playlistCreate: (name: string, rules: SmartRules | null, ids: number[] | null) => invoke<number>("playlist_create", { name, rules, ids }),
  playlistUpdate: (id: number, name: string, rules: SmartRules | null) => invoke<void>("playlist_update", { id, name, rules }),
  playlistDelete: (id: number) => invoke<void>("playlist_delete", { id }),
  playlistItems: (id: number) => invoke<number[]>("playlist_items", { id }),
  playlistSetItems: (id: number, ids: number[]) => invoke<void>("playlist_set_items", { id, ids }),
  playlistAddItems: (id: number, ids: number[]) => invoke<void>("playlist_add_items", { id, ids }),
  smartPreview: (rules: SmartRules) => invoke<number[]>("smart_preview", { rules }),
  playlistImport: (file: string) => invoke<[number, number]>("playlist_import", { file }),
  exportSize: (ids: number[]) => invoke<number>("export_size", { ids }),
  exportPlaylists: (ids: number[], dest: string, layout: Layout) => invoke<ExportReport>("export_playlists", { ids, dest, layout }),
  playlistSaveM3u8: (id: number, file: string) => invoke<void>("playlist_save_m3u8", { id, file }),
  syncPlaylistFolder: () => invoke<number>("sync_playlist_folder"),
  dirList: (path: string) => invoke<DirEntry[]>("dir_list", { path }),
  dirImportUntracked: (path: string) => invoke<number[]>("dir_import_untracked", { path }),
  dirCreate: (parent: string, name: string) => invoke<string>("dir_create", { parent, name }),
  dirRename: (path: string, name: string) => invoke<string>("dir_rename", { path, name }),
  dirMove: (path: string, target: string) => invoke<string>("dir_move", { path, target }),
  dirTrash: (path: string) => invoke<number>("dir_trash", { path }),
  moveTracksTo: (ids: number[], dir: string, copy: boolean) => invoke<ApplyReport>("move_tracks_to", { ids, dir, copy }),
  renameTrack: (id: number, name: string) => invoke<ApplyReport>("rename_track", { id, name }),
  renamePreview: (ids: number[], pattern: string) => invoke<[number, string][]>("rename_preview", { ids, pattern }),
  renameByPattern: (ids: number[], pattern: string) => invoke<ApplyReport>("rename_by_pattern", { ids, pattern }),
  ripTools: () => invoke<ToolStatus>("rip_tools"),
  ripInstall: () => invoke<ToolStatus>("rip_install"),
  ripUpdate: () => invoke<string>("rip_update"),
  ripResolve: (url: string) => invoke<{ name: string; entries: RipEntry[] }>("rip_resolve", { url }),
  ripDownload: (entries: RipEntry[], dir: string, format: "mp3" | "original") => invoke<{ ids: number[]; errors: string[] }>("rip_download", { entries, dir, format }),
  castDevices: () => invoke<CastDevice[]>("cast_devices"),
  castPlay: (device: string, id: number, start: number) => invoke<void>("cast_play", { device, id, start }),
  castPause: (device: string) => invoke<void>("cast_pause", { device }),
  castResume: (device: string) => invoke<void>("cast_resume", { device }),
  castStop: (device: string) => invoke<void>("cast_stop", { device }),
  castSeek: (device: string, sec: number) => invoke<void>("cast_seek", { device, sec }),
  castVolume: (device: string, volume: number) => invoke<void>("cast_volume", { device, volume }),
  castStatus: (device: string) => invoke<CastStatus>("cast_status", { device }),
  setTrayTitle: (text: string) => invoke<void>("set_tray_title", { text }),
  quit: () => invoke<void>("quit_app"),
  version: () => invoke<string>("app_version"),
};

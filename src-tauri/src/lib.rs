use base64::Engine;
use musiccore::{
    acoustid, analysis, cast, db, dirs, duplicates, export, history, musicbrainz, organize,
    pioneer, playlists, remote, ripper, scanner, sync, tags, watch,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager};

type Db = Arc<Mutex<Connection>>;

/// Eine Bibliothek – je Tab eine eigene (eigene Datenbank, Einstellungen, Überwachung)
struct AppState {
    /// Fensterkennung des Tabs ("main" oder "tab-…")
    label: String,
    db: Db,
    /// Laufende Aufgaben (Analyse, Export, Sync, Ripper) – jede mit eigenem Abbruch-Schalter,
    /// damit eine neu gestartete Aufgabe eine abgebrochene nicht wieder „anschaltet“
    tasks: Mutex<Vec<std::sync::Weak<AtomicBool>>>,
    watcher: Mutex<Option<watch::Watcher>>,
}

impl AppState {
    /// Neue Aufgabe anmelden: eigener Abbruch-Schalter
    fn task(&self) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        let mut t = self.tasks.lock().unwrap();
        t.retain(|w| w.strong_count() > 0);
        t.push(Arc::downgrade(&flag));
        flag
    }
    /// Alle laufenden Aufgaben dieses Tabs abbrechen
    fn cancel_all(&self) {
        for w in self.tasks.lock().unwrap().iter() {
            if let Some(f) = w.upgrade() {
                f.store(true, Ordering::Relaxed);
            }
        }
    }
}

// ---------- Tabs: jede Registerkarte ist eine eigene, komplette Instanz ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TabInfo {
    label: String,
    name: String,
}

struct Tabs {
    base: PathBuf,
    map: Mutex<HashMap<String, Arc<AppState>>>,
    /// zuletzt sichtbarer Tab (Infobereich „öffnen“ zeigt ihn)
    active: Mutex<String>,
}

impl Tabs {
    fn list_file(&self) -> PathBuf {
        self.base.join("tabs.json")
    }
    fn list(&self) -> Vec<TabInfo> {
        let mut v: Vec<TabInfo> = std::fs::read(self.list_file())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        if !v.iter().any(|t| t.label == "main") {
            v.insert(
                0,
                TabInfo {
                    label: "main".into(),
                    name: "Bibliothek".into(),
                },
            );
        }
        v
    }
    fn save(&self, v: &[TabInfo]) {
        let _ = std::fs::write(
            self.list_file(),
            serde_json::to_vec_pretty(v).unwrap_or_default(),
        );
    }
    fn dir_of(&self, label: &str) -> PathBuf {
        if label == "main" {
            self.base.clone()
        } else {
            self.base.join("tabs").join(label)
        }
    }
}

/// Bibliothek eines Tabs (beim ersten Zugriff öffnen)
fn tab_state(app: &AppHandle, label: &str) -> anyhow::Result<Arc<AppState>> {
    let tabs = app.state::<Tabs>();
    if let Some(s) = tabs.map.lock().unwrap().get(label) {
        return Ok(s.clone());
    }
    let conn = db::open(&tabs.dir_of(label).join("library.db"))?;
    if label == "main" {
        musiccore::webcache::purge(&conn); // Online-Zwischenspeicher nach 3 Tagen löschen
    }
    // Nur, was in den Musikordnern liegt, gehört zur Bibliothek
    let roots: Vec<PathBuf> = load_settings(&conn)
        .roots
        .iter()
        .map(PathBuf::from)
        .collect();
    let _ = dirs::prune_outside(&conn, &roots);
    for r in &roots {
        let _ = app.asset_protocol_scope().allow_directory(r, true);
    }
    let st = Arc::new(AppState {
        label: label.to_string(),
        db: Arc::new(Mutex::new(conn)),
        tasks: Mutex::new(Vec::new()),
        watcher: Mutex::new(None),
    });
    let st = {
        let mut m = tabs.map.lock().unwrap();
        m.entry(label.to_string()).or_insert(st).clone()
    };
    if st.watcher.lock().unwrap().is_none() {
        restart_watcher(app, &st);
    }
    Ok(st)
}

/// Befehlsargument: die Bibliothek des Fensters, aus dem der Aufruf kommt
#[derive(Clone)]
struct Tab(Arc<AppState>);

impl std::ops::Deref for Tab {
    type Target = AppState;
    fn deref(&self) -> &AppState {
        &self.0
    }
}

impl<'de> tauri::ipc::CommandArg<'de, tauri::Wry> for Tab {
    fn from_command(
        cmd: tauri::ipc::CommandItem<'de, tauri::Wry>,
    ) -> Result<Self, tauri::ipc::InvokeError> {
        let wv = cmd.message.webview();
        tab_state(wv.app_handle(), wv.label())
            .map(Tab)
            .map_err(|e| tauri::ipc::InvokeError::from(e.to_string()))
    }
}

/// Musikordner (neu) überwachen
fn restart_watcher(app: &AppHandle, state: &AppState) {
    let roots: Vec<PathBuf> = load_settings(&state.db.lock().unwrap())
        .roots
        .iter()
        .map(PathBuf::from)
        .collect();
    let app2 = app.clone();
    let db = state.db.clone();
    let label = state.label.clone();
    let w = watch::start(state.db.clone(), &roots, move |ch| {
        if !ch.added.is_empty() || ch.moved > 0 || ch.removed > 0 {
            sync_playlists(&db.lock().unwrap());
        }
        let _ = app2.emit_to(&label, "library-changed", ch);
    })
    .ok();
    *state.watcher.lock().unwrap() = w;
}

type R<T> = Result<T, String>;

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressEvent {
    task: String,
    done: usize,
    total: usize,
    message: String,
}

/// Fortschritt an das Fenster des Tabs melden
fn emitter(app: &AppHandle, label: &str, task: &str) -> impl Fn(usize, usize, &str) + Sync + use<> {
    let app = app.clone();
    let task = task.to_string();
    let label = label.to_string();
    move |done, total, msg| {
        let _ = app.emit_to(
            &label,
            "progress",
            ProgressEvent {
                task: task.clone(),
                done,
                total,
                message: msg.to_string(),
            },
        );
    }
}

async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> R<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(err)?
        .map_err(err)
}

// ---------- Einstellungen ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    roots: Vec<String>,
    playlist_dir: Option<String>,
    organize_target: Option<String>,
    pattern: String,
    threads: usize,
    auto_analyze: bool,
    acoustid_key: String,
    rip_dir: Option<String>,
    rip_format: String,
    close_to_tray: bool,
    /// Pioneer-Export (Beatgrid, Pioneer-Wellenform, Cues, OneLibrary/rekordbox-XML) – erst nach Einschalten aktiv
    pioneer: bool,
    /// Musikordner, deren Titel NICHT mit anderen Musikordnern auf Duplikate verglichen werden
    no_cross_dup: Vec<String>,
    /// Bibliothek NICHT für Minimal Player Connected freigeben (Standard: freigegeben)
    remote_off: bool,
    /// Ablage für Musik vom Handy (leer = „Musik/Minimal Library“ des Benutzers); je Gerät ein Unterordner
    phone_dir: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Settings {
            roots: vec![],
            playlist_dir: None,
            organize_target: None,
            pattern: organize::DEFAULT_PATTERN.into(),
            threads: (threads.saturating_sub(1)).clamp(1, 8),
            auto_analyze: true,
            acoustid_key: String::new(),
            rip_dir: None,
            rip_format: "mp3".into(),
            close_to_tray: true,
            pioneer: false,
            no_cross_dup: vec![],
            remote_off: false,
            phone_dir: None,
        }
    }
}

fn load_settings(conn: &Connection) -> Settings {
    db::get_setting(conn, "settings")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

#[tauri::command]
async fn get_settings(state: Tab) -> R<Settings> {
    Ok(load_settings(&state.db.lock().unwrap()))
}

#[tauri::command]
async fn set_settings(app: AppHandle, state: Tab, settings: Settings) -> R<usize> {
    let c = state.db.lock().unwrap();
    db::set_setting(
        &c,
        "settings",
        &serde_json::to_string(&settings).map_err(err)?,
    )
    .map_err(err)?;
    for r in &settings.roots {
        let _ = app.asset_protocol_scope().allow_directory(r, true);
    }
    // Musikordner entfernt → seine Titel verschwinden aus der Bibliothek, laufende Analyse stoppt
    let roots: Vec<PathBuf> = settings.roots.iter().map(PathBuf::from).collect();
    let pruned = dirs::prune_outside(&c, &roots).map_err(err)?;
    if pruned > 0 {
        state.cancel_all();
        sync_playlists(&c);
    }
    drop(c);
    restart_watcher(&app, &state);
    Ok(pruned)
}

/// Playlist-Ordner aktuell halten (falls eingestellt).
fn sync_playlists(conn: &Connection) {
    let s = load_settings(conn);
    if let Some(dir) = s.playlist_dir.filter(|d| !d.is_empty()) {
        let _ = playlists::sync_folder(conn, Path::new(&dir));
    }
}

// ---------- Songtexte ----------

fn track_path(db: &Db, id: i64) -> anyhow::Result<PathBuf> {
    db::tracks_by_ids(&db.lock().unwrap(), &[id])?
        .into_iter()
        .next()
        .map(|t| PathBuf::from(t.path))
        .ok_or_else(|| anyhow::anyhow!("Titel nicht gefunden"))
}

#[tauri::command]
async fn lyrics_get(state: Tab, id: i64) -> R<Option<musiccore::lyrics::Lyrics>> {
    let db = state.db.clone();
    blocking(move || Ok(musiccore::lyrics::read(&track_path(&db, id)?))).await
}

#[tauri::command]
async fn lyrics_save(state: Tab, id: i64, text: String) -> R<()> {
    let db = state.db.clone();
    blocking(move || musiccore::lyrics::save(&track_path(&db, id)?, &text)).await
}

/// Bei LRCLIB nachsehen (speichert nicht – der Editor zeigt das Ergebnis)
#[tauri::command]
async fn lyrics_fetch(state: Tab, id: i64) -> R<Option<musiccore::lyrics::Fetched>> {
    let db = state.db.clone();
    blocking(move || {
        let t = db::tracks_by_ids(&db.lock().unwrap(), &[id])?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Titel nicht gefunden"))?;
        musiccore::lyrics::fetch(
            t.artist.as_deref().unwrap_or(""),
            t.title.as_deref().unwrap_or(""),
            t.album.as_deref().unwrap_or(""),
            t.duration,
        )
    })
    .await
}

#[derive(Serialize)]
struct LyricsReport {
    found: usize,
    synced: usize,
    missing: usize,
    skipped: usize,
}

/// Für mehrere Titel holen und speichern (vorhandene .lrc bleiben)
#[tauri::command]
async fn lyrics_fetch_many(app: AppHandle, state: Tab, ids: Vec<i64>) -> R<LyricsReport> {
    let db = state.db.clone();
    let cancel = state.task();
    let prog = emitter(&app, &state.label, "lyrics");
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut r = LyricsReport {
            found: 0,
            synced: 0,
            missing: 0,
            skipped: 0,
        };
        let n = tracks.len();
        for (i, t) in tracks.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            prog(i, n, t.title.as_deref().unwrap_or(""));
            let p = Path::new(&t.path);
            if musiccore::lyrics::sidecar(p).exists() {
                r.skipped += 1;
                continue;
            }
            match musiccore::lyrics::fetch(
                t.artist.as_deref().unwrap_or(""),
                t.title.as_deref().unwrap_or(""),
                t.album.as_deref().unwrap_or(""),
                t.duration,
            ) {
                Ok(Some(f)) if f.synced.is_some() || f.plain.is_some() => {
                    let synced = f.synced.is_some();
                    musiccore::lyrics::save(
                        p,
                        f.synced.as_deref().or(f.plain.as_deref()).unwrap_or(""),
                    )?;
                    r.found += 1;
                    if synced {
                        r.synced += 1;
                    }
                }
                _ => r.missing += 1,
            }
            std::thread::sleep(std::time::Duration::from_millis(250)); // LRCLIB schonen
        }
        prog(n, n, "Fertig");
        Ok(r)
    })
    .await
}

// ---------- Stern (wie in Minimal Player) ----------

#[tauri::command]
async fn star_ids(state: Tab) -> R<Vec<i64>> {
    let c = state.db.lock().unwrap();
    match playlists::star(&c, false).map_err(err)? {
        Some(id) => playlists::track_ids(&c, id).map_err(err),
        None => Ok(vec![]),
    }
}

/// Stern setzen/entfernen: sind alle schon markiert → entfernen, sonst hinzufügen. Ergebnis: jetzt markiert?
#[tauri::command]
async fn star_toggle(state: Tab, ids: Vec<i64>) -> R<bool> {
    let mut c = state.db.lock().unwrap();
    let id = playlists::star(&c, true).map_err(err)?.ok_or("Stern")?;
    let cur = playlists::track_ids(&c, id).map_err(err)?;
    let all = ids.iter().all(|i| cur.contains(i));
    if all {
        let rest: Vec<i64> = cur.into_iter().filter(|i| !ids.contains(i)).collect();
        playlists::set_items(&mut c, id, &rest).map_err(err)?;
    } else {
        playlists::add_items(&mut c, id, &ids).map_err(err)?;
    }
    sync_playlists(&c);
    Ok(!all)
}

// ---------- Bibliothek fürs Handy (Minimal Player Connected) ----------

fn start_remote(app: &AppHandle, main: &AppState) {
    let db = main.db.clone();
    let db_s = main.db.clone();
    let db_c = main.db.clone();
    let app = app.clone();
    let default_phone = app
        .path()
        .audio_dir()
        .or_else(|_| app.path().home_dir())
        .map(|d| d.join("Minimal Library"))
        .unwrap_or_else(|_| PathBuf::from("Minimal Library"));
    let app_r = app.clone();
    remote::start(remote::Host {
        db,
        settings: Box::new(move || {
            let s = load_settings(&db_s.lock().unwrap());
            remote::HostSettings {
                roots: s.roots,
                pattern: s.pattern,
                organize_target: s.organize_target,
                enabled: !s.remote_off,
                phone_dir: s
                    .phone_dir
                    .filter(|d| !d.is_empty())
                    .map(PathBuf::from)
                    .unwrap_or_else(|| default_phone.clone()),
            }
        }),
        changed: Box::new(move |c| match c {
            remote::Changed::Playlists => {
                sync_playlists(&db_c.lock().unwrap());
                let _ = app.emit_to("main", "playlists-changed", ());
            }
            remote::Changed::Root(dir) => {
                // Handy-Ordner als Musikquelle aufnehmen (wenn nicht schon in einer enthalten)
                if let Ok(st) = tab_state(&app_r, "main") {
                    let added = {
                        let c = st.db.lock().unwrap();
                        let mut s = load_settings(&c);
                        let roots: Vec<PathBuf> = s.roots.iter().map(PathBuf::from).collect();
                        let probe = dir.join("x").to_string_lossy().to_string();
                        if dirs::under_any(&probe, &roots) {
                            false
                        } else {
                            s.roots.push(dir.to_string_lossy().to_string());
                            serde_json::to_string(&s)
                                .ok()
                                .map(|j| db::set_setting(&c, "settings", &j))
                                .is_some()
                        }
                    };
                    if added {
                        restart_watcher(&app_r, &st);
                        let _ = app_r.emit_to("main", "settings-changed", ());
                    }
                }
            }
            remote::Changed::Added(id) => {
                let _ = app.emit_to(
                    "main",
                    "library-changed",
                    watch::Change {
                        added: vec![id],
                        ..Default::default()
                    },
                );
            }
            remote::Changed::Files(n) => {
                let _ = app.emit_to(
                    "main",
                    "library-changed",
                    watch::Change {
                        moved: n,
                        ..Default::default()
                    },
                );
            }
        }),
    });
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoteInfo {
    code: String,
    port: u16,
    paired: Vec<String>,
    name: String,
}

/// Für die Einstellungen: Kopplungscode, gekoppelte Handys (nur Haupt-Bibliothek)
#[tauri::command]
async fn remote_info(app: AppHandle) -> R<RemoteInfo> {
    let st = tab_state(&app, "main").map_err(err)?;
    let paired = remote::paired_names(&st.db.lock().unwrap());
    Ok(RemoteInfo {
        code: remote::code(),
        port: remote::port(),
        paired,
        name: remote::computer_name(),
    })
}

#[tauri::command]
async fn remote_unpair(app: AppHandle, name: String) -> R<()> {
    let st = tab_state(&app, "main").map_err(err)?;
    remote::unpair(&st.db.lock().unwrap(), &name).map_err(err)
}

// ---------- Bibliothek ----------

#[tauri::command]
async fn scan_library(app: AppHandle, state: Tab) -> R<scanner::ScanReport> {
    let db = state.db.clone();
    let roots: Vec<PathBuf> = load_settings(&db.lock().unwrap())
        .roots
        .iter()
        .map(PathBuf::from)
        .collect();
    let prog = emitter(&app, &state.label, "scan");
    blocking(move || {
        let r = scanner::scan(&db, &roots, &prog)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

#[tauri::command]
async fn add_paths(app: AppHandle, state: Tab, paths: Vec<String>) -> R<AddReport> {
    let db = state.db.clone();
    blocking(move || {
        // Nur Musikordner zählen: hineingezogene Ordner außerhalb werden selbst zu Musikordnern,
        // einzelne Dateien außerhalb werden nicht aufgenommen.
        let mut s = load_settings(&db.lock().unwrap());
        let mut roots: Vec<PathBuf> = s.roots.iter().map(PathBuf::from).collect();
        let mut take = Vec::new();
        let mut rep = AddReport::default();
        for p in paths.iter().map(PathBuf::from) {
            if dirs::under_any(&p.to_string_lossy(), &roots) || roots.iter().any(|r| r == &p) {
                take.push(p);
            } else if p.is_dir() {
                s.roots.push(p.to_string_lossy().to_string());
                let _ = app.asset_protocol_scope().allow_directory(&p, true);
                roots.push(p.clone());
                rep.new_roots.push(p.to_string_lossy().to_string());
                take.push(p);
            } else {
                rep.skipped += 1;
            }
        }
        if !rep.new_roots.is_empty() {
            db::set_setting(&db.lock().unwrap(), "settings", &serde_json::to_string(&s)?)?;
        }
        rep.ids = scanner::add_files(&db, &take)?;
        Ok(rep)
    })
    .await
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct AddReport {
    ids: Vec<i64>,
    skipped: usize,
    new_roots: Vec<String>,
}

#[tauri::command]
async fn list_tracks(state: Tab) -> R<Vec<db::Track>> {
    let db = state.db.clone();
    blocking(move || db::all_tracks(&db.lock().unwrap())).await
}

#[tauri::command]
async fn get_tracks(state: Tab, ids: Vec<i64>) -> R<Vec<db::Track>> {
    db::tracks_by_ids(&state.db.lock().unwrap(), &ids).map_err(err)
}

/// Wellenformen als Base64 (je 2000 Bytes).
#[tauri::command]
async fn get_peaks(state: Tab, ids: Vec<i64>) -> R<HashMap<i64, String>> {
    let c = state.db.lock().unwrap();
    let mut out = HashMap::new();
    for id in ids {
        if let Some(p) = db::peaks(&c, id).map_err(err)? {
            out.insert(id, base64::engine::general_purpose::STANDARD.encode(p));
        }
    }
    Ok(out)
}

/// Gibt die Datei für die Wiedergabe frei.
#[tauri::command]
fn allow_play(app: AppHandle, path: String) -> R<()> {
    app.asset_protocol_scope().allow_file(&path).map_err(err)
}

#[tauri::command]
async fn analyze(app: AppHandle, state: Tab, ids: Option<Vec<i64>>) -> R<usize> {
    let db = state.db.clone();
    let cancel = state.task();
    let s = load_settings(&db.lock().unwrap());
    let (threads, pioneer) = (s.threads, s.pioneer);
    let prog = emitter(&app, &state.label, "analyze");
    let app2 = app.clone();
    let label = state.label.clone();
    blocking(move || {
        let n =
            analysis::analyze_pending_with(&db, ids, threads, pioneer, &cancel, &prog, &|id| {
                let _ = app2.emit_to(&label, "analyzed", id);
            })?;
        if cancel.load(Ordering::Relaxed) {
            prog(1, 1, "Abgebrochen"); // Fortschrittsanzeige schließen
        }
        Ok(n)
    })
    .await
}

#[tauri::command]
async fn cancel_task(state: Tab) {
    state.cancel_all();
}

#[tauri::command]
async fn remove_from_library(state: Tab, ids: Vec<i64>) -> R<()> {
    let db = state.db.clone();
    blocking(move || {
        let c = db.lock().unwrap();
        db::remove_tracks(&c, &ids)?;
        sync_playlists(&c);
        Ok(())
    })
    .await
}

/// Dateien in den Windows-Papierkorb verschieben und aus der Bibliothek entfernen.
#[tauri::command]
async fn trash_tracks(
    state: Tab,
    ids: Vec<i64>,
    replace: Option<HashMap<String, i64>>,
) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        // Duplikate: Playlist-Einträge auf den behaltenen Titel umbiegen
        if let Some(map) = replace {
            let c = db.lock().unwrap();
            for (from, to) in map {
                if let Ok(from) = from.parse::<i64>() {
                    c.execute(
                        "UPDATE playlist_items SET track_id = ?2 WHERE track_id = ?1",
                        rusqlite::params![from, to],
                    )?;
                }
            }
        }
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        let mut gone = vec![];
        let batch = history::begin(
            &db.lock().unwrap(),
            "trash",
            &format!("Papierkorb: {}", describe_tracks(&tracks)),
        )?;
        let mut trashed = 0;
        for t in &tracks {
            let p = Path::new(&t.path);
            if !p.exists() {
                gone.push(t.id);
                continue;
            }
            history::backup_trash(&db.lock().unwrap(), batch, p, false, &[t.id])?;
            match trash::delete(p) {
                Ok(()) => {
                    gone.push(t.id);
                    trashed += 1;
                }
                Err(e) => errors.push(format!("{}: {e}", t.path)),
            }
        }
        let c = db.lock().unwrap();
        history::finish(&c, batch, trashed)?;
        db::remove_tracks(&c, &gone)?;
        sync_playlists(&c);
        Ok(errors)
    })
    .await
}

// ---------- Tags ----------

#[tauri::command]
async fn write_tags(state: Tab, ids: Vec<i64>, update: tags::TagUpdate) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        let batch = history::begin(
            &db.lock().unwrap(),
            "tags",
            &format!(
                "Tags ({}): {}",
                changed_fields(&update),
                describe_tracks(&tracks)
            ),
        )?;
        let mut done = 0;
        for t in tracks {
            let p = Path::new(&t.path);
            if let Err(e) = history::backup_tags(&db.lock().unwrap(), batch, &t, false) {
                errors.push(format!("{e:#}"));
                continue;
            }
            match tags::write(p, &update) {
                Ok(()) => {
                    done += 1;
                    let c = db.lock().unwrap();
                    scanner::refresh(&c, p)?;
                    // Eingetragene BPM/Tonart sollen die Analyse überstimmen
                    if update.bpm.is_some() {
                        c.execute("UPDATE tracks SET bpm = NULL WHERE id = ?1", [t.id])?;
                    }
                    if update.key.is_some() {
                        c.execute("UPDATE tracks SET musical_key = NULL WHERE id = ?1", [t.id])?;
                    }
                }
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        let c = db.lock().unwrap();
        history::finish(&c, batch, done)?;
        sync_playlists(&c);
        Ok(errors)
    })
    .await
}

fn describe_tracks(tracks: &[db::Track]) -> String {
    match tracks {
        [t] => t.title.clone().unwrap_or_else(|| {
            Path::new(&t.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        }),
        _ => format!("{} Titel", tracks.len()),
    }
}

fn changed_fields(u: &tags::TagUpdate) -> String {
    let f = [
        (&u.title, "Titel"),
        (&u.artist, "Künstler"),
        (&u.album, "Album"),
        (&u.album_artist, "Albumkünstler"),
        (&u.genre, "Genre"),
        (&u.year, "Jahr"),
        (&u.track_no, "Track"),
        (&u.disc_no, "Disc"),
        (&u.comment, "Kommentar"),
        (&u.bpm, "BPM"),
        (&u.key, "Tonart"),
    ];
    let v: Vec<&str> = f
        .iter()
        .filter(|(x, _)| x.is_some())
        .map(|(_, n)| *n)
        .collect();
    v.join(", ")
}

/// Analysierte BPM/Tonart dauerhaft in die Datei-Tags schreiben.
#[tauri::command]
async fn write_analysis_to_tags(state: Tab, ids: Vec<i64>) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        let batch = history::begin(
            &db.lock().unwrap(),
            "tags",
            &format!("BPM + Tonart in Datei: {}", describe_tracks(&tracks)),
        )?;
        let mut done = 0;
        for t in tracks {
            if t.bpm.is_none() && t.key.is_none() {
                continue;
            }
            if history::backup_tags(&db.lock().unwrap(), batch, &t, false).is_err() {
                continue;
            }
            done += 1;
            let u = tags::TagUpdate {
                bpm: t.bpm.map(|b| b.to_string()),
                key: t.key.clone(),
                ..Default::default()
            };
            let p = Path::new(&t.path);
            match tags::write(p, &u) {
                Ok(()) => {
                    scanner::refresh(&db.lock().unwrap(), p)?;
                }
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        history::finish(&db.lock().unwrap(), batch, done)?;
        Ok(errors)
    })
    .await
}

#[tauri::command]
async fn get_cover(state: Tab, id: i64) -> R<Option<String>> {
    let db = state.db.clone();
    blocking(move || {
        let Some(t) = db::track(&db.lock().unwrap(), id)? else {
            return Ok(None);
        };
        Ok(tags::cover(Path::new(&t.path))
            .ok()
            .flatten()
            .map(|(mime, data)| {
                format!(
                    "data:{mime};base64,{}",
                    base64::engine::general_purpose::STANDARD.encode(data)
                )
            }))
    })
    .await
}

fn apply_cover(db: &Db, ids: &[i64], data: Option<Vec<u8>>) -> anyhow::Result<Vec<String>> {
    let tracks = db::tracks_by_ids(&db.lock().unwrap(), ids)?;
    let mut errors = vec![];
    let what = if data.is_some() {
        "Cover gesetzt"
    } else {
        "Cover entfernt"
    };
    let batch = history::begin(
        &db.lock().unwrap(),
        "tags",
        &format!("{what}: {}", describe_tracks(&tracks)),
    )?;
    let n = tracks.len();
    for t in tracks {
        let p = Path::new(&t.path);
        if let Err(e) = history::backup_tags(&db.lock().unwrap(), batch, &t, true) {
            errors.push(format!("{e:#}"));
            continue;
        }
        let r = match &data {
            Some(d) => tags::set_cover(p, d.clone()),
            None => tags::remove_cover(p),
        };
        match r {
            Ok(()) => {
                scanner::refresh(&db.lock().unwrap(), p)?;
            }
            Err(e) => errors.push(format!("{e:#}")),
        }
    }
    history::finish(&db.lock().unwrap(), batch, n - errors.len().min(n))?;
    Ok(errors)
}

#[tauri::command]
async fn set_cover_file(state: Tab, ids: Vec<i64>, file: String) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let data = std::fs::read(&file)?;
        apply_cover(&db, &ids, Some(data))
    })
    .await
}

#[tauri::command]
async fn remove_cover(state: Tab, ids: Vec<i64>) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || apply_cover(&db, &ids, None)).await
}

// ---------- MusicBrainz ----------

#[tauri::command]
async fn mb_search(
    state: Tab,
    artist: String,
    title: String,
    album: Option<String>,
) -> R<Vec<musicbrainz::Candidate>> {
    let db = state.db.clone();
    blocking(move || musicbrainz::search(Some(&db), &artist, &title, album.as_deref())).await
}

/// Titel am Klang erkennen (AcoustID)
#[tauri::command]
async fn acoustid_lookup(state: Tab, id: i64) -> R<Vec<musicbrainz::Candidate>> {
    let db = state.db.clone();
    blocking(move || {
        let c = db.lock().unwrap();
        let key = load_settings(&c).acoustid_key;
        let (fp, dur): (Option<Vec<u8>>, f64) = c.query_row(
            "SELECT fingerprint, duration FROM tracks WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        drop(c);
        let fp = db::bytes_to_u32(&fp.unwrap_or_default());
        acoustid::lookup(Some(&db), &key, &fp, dur)
    })
    .await
}

#[tauri::command]
async fn mb_cover(state: Tab, ids: Vec<i64>, release_id: String) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let data = musicbrainz::cover(Some(&db), &release_id)?;
        apply_cover(&db, &ids, Some(data))
    })
    .await
}

// ---------- Duplikate ----------

#[tauri::command]
async fn find_duplicates(
    app: AppHandle,
    state: Tab,
    modes: duplicates::Modes,
) -> R<Vec<duplicates::DupGroup>> {
    let db = state.db.clone();
    let prog = emitter(&app, &state.label, "duplicates");
    blocking(move || {
        let (tracks, fps, ignored, settings) = {
            let c = db.lock().unwrap();
            let mut st = c.prepare("SELECT a, b FROM dup_ignore")?;
            let ignored: duplicates::Ignored = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            (
                db::all_tracks(&c)?,
                if modes.audio {
                    db::fingerprints(&c)?
                } else {
                    HashMap::new()
                },
                ignored,
                load_settings(&c),
            )
        };
        // Musikordner je Titel: Paare aus verschiedenen Ordnern nur, wenn beide Ordner das erlauben
        let roots: Vec<PathBuf> = settings.roots.iter().map(PathBuf::from).collect();
        let root_of: Vec<Option<usize>> =
            tracks.iter().map(|t| root_index(&t.path, &roots)).collect();
        let closed: Vec<bool> = settings
            .roots
            .iter()
            .map(|r| settings.no_cross_dup.iter().any(|x| x == r))
            .collect();
        let scope = |a: usize, b: usize| match (root_of[a], root_of[b]) {
            (Some(x), Some(y)) if x != y => !closed[x] && !closed[y],
            _ => true,
        };
        Ok(duplicates::find_scoped(
            &tracks, &fps, modes, &ignored, &scope, &prog,
        ))
    })
    .await
}

/// „Kein Duplikat“: diese Titel nie wieder gegeneinander melden (jedes Paar der Liste)
#[tauri::command]
async fn dup_ignore(state: Tab, ids: Vec<i64>, against: Option<Vec<i64>>) -> R<usize> {
    let c = state.db.lock().unwrap();
    let mut n = 0;
    let mut add = |a: i64, b: i64| {
        if a != b {
            let (x, y) = duplicates::pair(a, b);
            n += c
                .execute(
                    "INSERT OR IGNORE INTO dup_ignore(a, b) VALUES(?1, ?2)",
                    rusqlite::params![x, y],
                )
                .unwrap_or(0);
        }
    };
    match against {
        // ein Titel gehört nicht zu den anderen der Gruppe
        Some(others) => ids
            .iter()
            .for_each(|&a| others.iter().for_each(|&b| add(a, b))),
        // ganze Gruppe: alle Paare
        None => ids
            .iter()
            .enumerate()
            .for_each(|(i, &a)| ids[i + 1..].iter().for_each(|&b| add(a, b))),
    }
    Ok(n)
}

#[tauri::command]
async fn dup_ignore_count(state: Tab) -> R<usize> {
    state
        .db
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM dup_ignore", [], |r| {
            r.get::<_, i64>(0)
        })
        .map(|n| n as usize)
        .map_err(err)
}

#[tauri::command]
async fn dup_ignore_clear(state: Tab) -> R<()> {
    state
        .db
        .lock()
        .unwrap()
        .execute("DELETE FROM dup_ignore", [])
        .map(|_| ())
        .map_err(err)
}

// ---------- Sortieren ----------

/// In welchem Musikordner liegt der Pfad? (längster passender zuerst)
fn root_index(path: &str, roots: &[PathBuf]) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None;
    for (i, r) in roots.iter().enumerate() {
        if dirs::under_any(path, std::slice::from_ref(r)) {
            let len = r.as_os_str().len();
            if best.is_none_or(|(_, l)| len > l) {
                best = Some((i, len));
            }
        }
    }
    best.map(|(i, _)| i)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DupMatch {
    id: i64,
    path: String,
    artist: Option<String>,
    title: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DupCheck {
    track_id: i64,
    matches: Vec<DupMatch>,
    reasons: Vec<String>,
    audio_similarity: Option<f32>,
}

/// Vor dem Kopieren/Verschieben nach `dir`: Gibt es die Titel dort (im ganzen Musikordner des Ziels)
/// schon? `tab` = anderer Tab als Ziel (dessen Bibliothek wird verglichen).
#[tauri::command]
async fn dup_check(
    app: AppHandle,
    state: Tab,
    ids: Vec<i64>,
    dir: String,
    tab: Option<String>,
) -> R<Vec<DupCheck>> {
    let src_db = state.db.clone();
    let dst = match &tab {
        Some(l) if *l != state.label => tab_state(&app, l).map_err(err)?,
        _ => state.0.clone(),
    };
    let other = !Arc::ptr_eq(&dst, &state.0);
    blocking(move || {
        let (src, mut fps, ignored) = {
            let c = src_db.lock().unwrap();
            let src = db::tracks_by_ids(&c, &ids)?;
            let fps = db::fingerprints(&c)?;
            let mut st = c.prepare("SELECT a, b FROM dup_ignore")?;
            let ignored: duplicates::Ignored = st
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            (src, fps, ignored)
        };
        fps.retain(|k, _| ids.contains(k));
        let (mut cand, dfps) = {
            let c = dst.db.lock().unwrap();
            let roots: Vec<PathBuf> = load_settings(&c).roots.iter().map(PathBuf::from).collect();
            // Ziel-Musikordner (bzw. der Zielordner selbst, wenn er in keinem liegt)
            let scope: Vec<PathBuf> = match root_index(&format!("{dir}/x"), &roots) {
                Some(i) => vec![roots[i].clone()],
                None => vec![PathBuf::from(&dir)],
            };
            let cand: Vec<db::Track> = db::all_tracks(&c)?
                .into_iter()
                .filter(|t| dirs::under_any(&t.path, &scope) && (other || !ids.contains(&t.id)))
                .collect();
            (cand, db::fingerprints(&c)?)
        };
        // IDs aus einer anderen Bibliothek können sich mit unseren überschneiden → negativ machen
        let map = |id: i64| if other { -id - 1 } else { id };
        for t in cand.iter_mut() {
            if let Some(f) = dfps.get(&t.id) {
                fps.insert(map(t.id), f.clone());
            }
            t.id = map(t.id);
        }
        let none = duplicates::Ignored::default();
        let hits =
            duplicates::check_against(&src, &cand, &fps, if other { &none } else { &ignored });
        let by_id: HashMap<i64, &db::Track> = cand.iter().map(|t| (t.id, t)).collect();
        let unmap = |id: i64| if other { -(id + 1) } else { id };
        Ok(hits
            .into_iter()
            .map(|h| DupCheck {
                track_id: h.track_id,
                matches: h
                    .matches
                    .iter()
                    .filter_map(|m| by_id.get(m))
                    .map(|t| DupMatch {
                        id: unmap(t.id),
                        path: t.path.clone(),
                        artist: t.artist.clone(),
                        title: t.title.clone(),
                    })
                    .collect(),
                reasons: h.reasons,
                audio_similarity: h.audio_similarity,
            })
            .collect())
    })
    .await
}

/// Musikordner eines anderen Tabs (Ziel beim Ziehen auf einen Tab)
#[tauri::command]
async fn tab_roots(app: AppHandle, label: String) -> R<Vec<String>> {
    let st = tab_state(&app, &label).map_err(err)?;
    let roots = load_settings(&st.db.lock().unwrap()).roots;
    Ok(roots)
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct TransferReport {
    moved: usize,
    errors: Vec<String>,
    new_ids: Vec<i64>,
}

/// Titel in einen anderen Tab kopieren oder verschieben (in einen Ordner dieses Tabs).
/// Ziel außerhalb seiner Musikordner → wird dort als Musikordner aufgenommen.
#[tauri::command]
async fn transfer_to_tab(
    app: AppHandle,
    state: Tab,
    ids: Vec<i64>,
    label: String,
    dir: String,
    copy: bool,
    skip: Vec<i64>,
) -> R<TransferReport> {
    let dst = tab_state(&app, &label).map_err(err)?;
    let src_db = state.db.clone();
    let prog = emitter(&app, &state.label, "organize");
    let _ = app.asset_protocol_scope().allow_directory(&dir, true);
    let app2 = app.clone();
    blocking(move || {
        let tracks: Vec<db::Track> = db::tracks_by_ids(&src_db.lock().unwrap(), &ids)?
            .into_iter()
            .filter(|t| !skip.contains(&t.id))
            .collect();
        let plan = organize::plan_to_folder(&tracks, Path::new(&dir), None);
        let stops = stop_dirs(&src_db.lock().unwrap());
        let mut rep = TransferReport::default();
        let mut done_paths = Vec::new();
        for (i, m) in plan.iter().enumerate() {
            prog(i, plan.len(), &m.to);
            if m.unchanged {
                continue;
            }
            let (from, to) = (Path::new(&m.from), Path::new(&m.to));
            if to.exists() {
                rep.errors.push(format!("Ziel belegt: {}", m.to));
                continue;
            }
            let r = if copy {
                to.parent()
                    .map(std::fs::create_dir_all)
                    .transpose()
                    .and_then(|_| std::fs::copy(from, to).map(|_| ()))
            } else {
                musiccore::paths::move_file(from, to)
            };
            match r {
                Ok(()) => {
                    if !copy {
                        let c = src_db.lock().unwrap();
                        c.execute("DELETE FROM tracks WHERE id = ?1", [m.track_id])?;
                        c.execute(
                            "DELETE FROM playlist_items WHERE track_id = ?1",
                            [m.track_id],
                        )?;
                        if let Some(d) = from.parent() {
                            musiccore::paths::remove_empty_dirs(d.to_path_buf(), &stops);
                        }
                    }
                    done_paths.push(to.to_path_buf());
                    rep.moved += 1;
                }
                Err(e) => rep.errors.push(format!("{}: {e}", m.from)),
            }
        }
        prog(plan.len(), plan.len(), "Fertig");
        // Ziel-Tab: Ordner ggf. als Musikordner aufnehmen, Dateien eintragen
        {
            let c = dst.db.lock().unwrap();
            let mut s = load_settings(&c);
            let roots: Vec<PathBuf> = s.roots.iter().map(PathBuf::from).collect();
            if !dirs::under_any(&format!("{dir}/x"), &roots) {
                s.roots.push(dir.clone());
                db::set_setting(&c, "settings", &serde_json::to_string(&s)?)?;
            }
        }
        rep.new_ids = scanner::add_files(&dst.db, &done_paths)?;
        sync_playlists(&src_db.lock().unwrap());
        let _ = app2.emit_to(
            &dst.label,
            "library-changed",
            watch::Change {
                added: rep.new_ids.clone(),
                ..Default::default()
            },
        );
        Ok(rep)
    })
    .await
}

fn stop_dirs(conn: &Connection) -> Vec<PathBuf> {
    let s = load_settings(conn);
    let mut v: Vec<PathBuf> = s.roots.iter().map(PathBuf::from).collect();
    if let Some(t) = s.organize_target {
        v.push(PathBuf::from(t));
    }
    v
}

#[tauri::command]
async fn organize_preview(
    state: Tab,
    ids: Option<Vec<i64>>,
    target: String,
    pattern: String,
) -> R<Vec<organize::PlannedMove>> {
    let db = state.db.clone();
    blocking(move || {
        let c = db.lock().unwrap();
        let tracks = match ids {
            Some(ids) => db::tracks_by_ids(&c, &ids)?,
            None => db::all_tracks(&c)?,
        };
        Ok(organize::plan(&tracks, Path::new(&target), &pattern))
    })
    .await
}

#[tauri::command]
async fn organize_apply(
    app: AppHandle,
    state: Tab,
    plan: Vec<organize::PlannedMove>,
    copy: bool,
    description: String,
) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    let prog = emitter(&app, &state.label, "organize");
    blocking(move || {
        let stops = stop_dirs(&db.lock().unwrap());
        let r = organize::apply(&db, &plan, copy, &description, &stops, &prog)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

#[tauri::command]
async fn move_batches(state: Tab) -> R<Vec<organize::Batch>> {
    organize::batches(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
async fn undo_batch(state: Tab, id: Option<i64>) -> R<history::UndoReport> {
    let db = state.db.clone();
    blocking(move || {
        let id = match id {
            Some(id) => id,
            None => history::last_open(&db.lock().unwrap())?
                .ok_or_else(|| anyhow::anyhow!("Nichts zum Rückgängigmachen"))?,
        };
        let stops = stop_dirs(&db.lock().unwrap());
        let r = history::undo(&db, id, &stops)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

// ---------- Playlisten ----------

fn delete_playlist_file(conn: &Connection, name: &str) {
    if let Some(dir) = load_settings(conn).playlist_dir.filter(|d| !d.is_empty()) {
        let _ = std::fs::remove_file(
            Path::new(&dir).join(format!("{}.m3u8", musiccore::paths::sanitize(name))),
        );
    }
}

#[tauri::command]
async fn playlists_list(state: Tab) -> R<Vec<playlists::Playlist>> {
    playlists::list(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
async fn playlist_create(
    state: Tab,
    name: String,
    rules: Option<playlists::SmartRules>,
    ids: Option<Vec<i64>>,
) -> R<i64> {
    let mut c = state.db.lock().unwrap();
    let id = playlists::create(&c, &name, rules.as_ref()).map_err(err)?;
    if let Some(ids) = ids {
        playlists::set_items(&mut c, id, &ids).map_err(err)?;
    }
    sync_playlists(&c);
    Ok(id)
}

#[tauri::command]
async fn playlist_update(
    state: Tab,
    id: i64,
    name: String,
    rules: Option<playlists::SmartRules>,
) -> R<()> {
    let c = state.db.lock().unwrap();
    let old = playlists::name(&c, id).map_err(err)?;
    if old != name {
        delete_playlist_file(&c, &old);
    }
    playlists::update(&c, id, &name, rules.as_ref()).map_err(err)?;
    sync_playlists(&c);
    Ok(())
}

#[tauri::command]
async fn playlist_delete(state: Tab, id: i64) -> R<()> {
    let c = state.db.lock().unwrap();
    if let Ok(n) = playlists::name(&c, id) {
        delete_playlist_file(&c, &n);
    }
    playlists::delete(&c, id).map_err(err)
}

#[tauri::command]
async fn playlist_items(state: Tab, id: i64) -> R<Vec<i64>> {
    playlists::track_ids(&state.db.lock().unwrap(), id).map_err(err)
}

#[tauri::command]
async fn playlist_set_items(state: Tab, id: i64, ids: Vec<i64>) -> R<()> {
    let mut c = state.db.lock().unwrap();
    playlists::set_items(&mut c, id, &ids).map_err(err)?;
    sync_playlists(&c);
    Ok(())
}

#[tauri::command]
async fn playlist_add_items(state: Tab, id: i64, ids: Vec<i64>) -> R<()> {
    let mut c = state.db.lock().unwrap();
    playlists::add_items(&mut c, id, &ids).map_err(err)?;
    sync_playlists(&c);
    Ok(())
}

#[tauri::command]
async fn smart_preview(state: Tab, rules: playlists::SmartRules) -> R<Vec<i64>> {
    let all = db::all_tracks(&state.db.lock().unwrap()).map_err(err)?;
    Ok(playlists::evaluate(&all, &rules))
}

/// M3U/M3U8 importieren: unbekannte Dateien werden zur Bibliothek hinzugefügt.
#[tauri::command]
async fn playlist_import(state: Tab, file: String) -> R<(i64, usize)> {
    let db = state.db.clone();
    blocking(move || {
        let paths: Vec<PathBuf> = playlists::read_m3u(Path::new(&file))?
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        let ids = scanner::add_files(&db, &paths)?;
        let name = Path::new(&file)
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Import".into());
        let mut c = db.lock().unwrap();
        let id = playlists::create(&c, &name, None)?;
        playlists::set_items(&mut c, id, &ids)?;
        sync_playlists(&c);
        Ok((id, ids.len()))
    })
    .await
}

#[tauri::command]
async fn playlist_save_m3u8(state: Tab, id: i64, file: String) -> R<()> {
    playlists::write_m3u8(&state.db.lock().unwrap(), id, Path::new(&file)).map_err(err)
}

#[tauri::command]
async fn export_size(state: Tab, ids: Vec<i64>) -> R<u64> {
    export::size_of(&state.db.lock().unwrap(), &ids).map_err(err)
}

#[tauri::command]
async fn export_playlists(
    app: AppHandle,
    state: Tab,
    ids: Vec<i64>,
    dest: String,
    layout: export::Layout,
) -> R<export::ExportReport> {
    let db = state.db.clone();
    let cancel = state.task();
    let prog = emitter(&app, &state.label, "export");
    blocking(move || export::export(&db, &ids, Path::new(&dest), layout, &cancel, &prog)).await
}

// ---------- Pioneer-Export (nur wenn eingeschaltet) ----------

fn pioneer_on(db: &Db) -> Result<(), String> {
    if load_settings(&db.lock().unwrap()).pioneer {
        Ok(())
    } else {
        Err("Pioneer-Export ist in den Einstellungen ausgeschaltet".into())
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PioneerTrack {
    grid: Option<pioneer::Grid>,
    cues: Vec<pioneer::Cue>,
    /// 150 Werte/s × [Spitze, Bass, Mitten, Höhen], Base64
    frames: Option<String>,
}

#[tauri::command]
async fn pioneer_track(state: Tab, id: i64) -> R<PioneerTrack> {
    pioneer_on(&state.db)?;
    use base64::Engine;
    let c = state.db.lock().unwrap();
    Ok(PioneerTrack {
        grid: pioneer::grid(&c, id).map_err(err)?,
        cues: pioneer::cues(&c, id).map_err(err)?,
        frames: pioneer::frames(&c, id)
            .map_err(err)?
            .map(|f| base64::engine::general_purpose::STANDARD.encode(f)),
    })
}

#[tauri::command]
async fn pioneer_set_grid(state: Tab, id: i64, bpm: f64, downbeat_ms: f64, what: String) -> R<i64> {
    pioneer_on(&state.db)?;
    pioneer::set_grid(&state.db.lock().unwrap(), id, bpm, downbeat_ms, &what).map_err(err)
}

#[tauri::command]
async fn pioneer_set_cue(state: Tab, id: i64, cue: pioneer::Cue) -> R<i64> {
    pioneer_on(&state.db)?;
    pioneer::set_cue(&state.db.lock().unwrap(), id, &cue).map_err(err)
}

#[tauri::command]
async fn pioneer_delete_cue(state: Tab, id: i64, cue_id: i64) -> R<i64> {
    pioneer_on(&state.db)?;
    pioneer::delete_cue(&state.db.lock().unwrap(), id, cue_id).map_err(err)
}

#[tauri::command]
async fn pioneer_preview(
    app: AppHandle,
    state: Tab,
    options: pioneer::export::Options,
) -> R<pioneer::export::Preview> {
    pioneer_on(&state.db)?;
    let db = state.db.clone();
    let ff = ripper::find(&tools_dir(&app), ripper::FFMPEG).is_some();
    blocking(move || pioneer::export::preview(&db.lock().unwrap(), &options, ff)).await
}

/// Export: fehlende Analysen nachholen, dann auf das Laufwerk schreiben
#[tauri::command]
async fn pioneer_export(
    app: AppHandle,
    state: Tab,
    options: pioneer::export::Options,
) -> R<pioneer::export::Report> {
    pioneer_on(&state.db)?;
    let db = state.db.clone();
    let cancel = state.task();
    let threads = load_settings(&db.lock().unwrap()).threads;
    let ffmpeg = ripper::find(&tools_dir(&app), ripper::FFMPEG);
    let prog = emitter(&app, &state.label, "pioneer");
    let app2 = app.clone();
    let label = state.label.clone();
    blocking(move || {
        let missing = pioneer::export::preview(&db.lock().unwrap(), &options, ffmpeg.is_some())?
            .missing_analysis;
        if !missing.is_empty() {
            analysis::analyze_pending_with(
                &db,
                Some(missing),
                threads,
                true,
                &cancel,
                &prog,
                &|id| {
                    let _ = app2.emit_to(&label, "analyzed", id);
                },
            )?;
        }
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("Abgebrochen");
        }
        pioneer::export::run(&db, &options, ffmpeg.as_deref(), true, &cancel, &prog)
    })
    .await
}

#[tauri::command]
async fn pioneer_xml(app: AppHandle, state: Tab, playlists: Vec<i64>, file: String) -> R<()> {
    pioneer_on(&state.db)?;
    let db = state.db.clone();
    let version = app.package_info().version.to_string();
    blocking(move || {
        let x = pioneer::xml::build(&db.lock().unwrap(), &playlists, &version)?;
        std::fs::write(&file, x)?;
        Ok(())
    })
    .await
}

#[tauri::command]
async fn sync_playlist_folder(state: Tab) -> R<usize> {
    let c = state.db.lock().unwrap();
    let s = load_settings(&c);
    match s.playlist_dir.filter(|d| !d.is_empty()) {
        Some(d) => playlists::sync_folder(&c, Path::new(&d))
            .map(|v| v.len())
            .map_err(err),
        None => Err("Kein Playlist-Ordner eingestellt".into()),
    }
}

// ---------- Ordneransicht ----------

#[tauri::command]
async fn dir_list(state: Tab, path: String) -> R<Vec<dirs::DirEntry>> {
    let db = state.db.clone();
    blocking(move || dirs::list(&db.lock().unwrap(), Path::new(&path))).await
}

/// Audiodateien im Ordner, die noch nicht in der Bibliothek sind, aufnehmen.
#[tauri::command]
async fn dir_import_untracked(state: Tab, path: String) -> R<Vec<i64>> {
    let db = state.db.clone();
    blocking(move || {
        let files = dirs::untracked_files(&db.lock().unwrap(), Path::new(&path))?;
        scanner::add_files(&db, &files)
    })
    .await
}

#[tauri::command]
fn dir_create(app: AppHandle, parent: String, name: String) -> R<String> {
    let p = dirs::create(Path::new(&parent), &name).map_err(err)?;
    let _ = app.asset_protocol_scope().allow_directory(&p, true);
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
async fn dir_rename(app: AppHandle, state: Tab, path: String, name: String) -> R<String> {
    let db = state.db.clone();
    let p = blocking(move || {
        let c = db.lock().unwrap();
        let p = dirs::rename(&c, Path::new(&path), &name)?;
        history::log_dir_move(
            &c,
            Path::new(&path),
            &p,
            &format!(
                "Ordner umbenannt: {} → {}",
                Path::new(&path)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy(),
                p.file_name().unwrap_or_default().to_string_lossy()
            ),
        )?;
        sync_playlists(&c);
        Ok(p)
    })
    .await?;
    let _ = app.asset_protocol_scope().allow_directory(&p, true);
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
async fn dir_move(app: AppHandle, state: Tab, path: String, target: String) -> R<String> {
    let db = state.db.clone();
    let p = blocking(move || {
        let c = db.lock().unwrap();
        let p = dirs::move_into(&c, Path::new(&path), Path::new(&target))?;
        if !musiccore::paths::same_path(&p, Path::new(&path)) {
            history::log_dir_move(
                &c,
                Path::new(&path),
                &p,
                &format!(
                    "Ordner verschoben: {} → {}",
                    p.file_name().unwrap_or_default().to_string_lossy(),
                    Path::new(&target)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                ),
            )?;
        }
        sync_playlists(&c);
        Ok(p)
    })
    .await?;
    let _ = app.asset_protocol_scope().allow_directory(&p, true);
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
async fn dir_trash(state: Tab, path: String) -> R<usize> {
    let db = state.db.clone();
    blocking(move || {
        {
            let c = db.lock().unwrap();
            let pre = format!(
                "{}{}",
                path.trim_end_matches(['/', '\\']),
                std::path::MAIN_SEPARATOR
            );
            let mut st = c.prepare("SELECT id FROM tracks WHERE substr(path, 1, ?2) = ?1")?;
            let ids: Vec<i64> = st
                .query_map(rusqlite::params![pre, pre.chars().count() as i64], |r| {
                    r.get(0)
                })?
                .collect::<rusqlite::Result<_>>()?;
            let name = Path::new(&path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let b = history::begin(&c, "trash", &format!("Ordner in den Papierkorb: {name}"))?;
            history::backup_trash(&c, b, Path::new(&path), true, &ids)?;
            history::finish(&c, b, 1)?;
        }
        trash::delete(&path)?;
        let c = db.lock().unwrap();
        let n = dirs::remove_tracks_under(&c, Path::new(&path))?;
        sync_playlists(&c);
        Ok(n)
    })
    .await
}

/// Titel manuell in einen Ordner verschieben/kopieren (mit Verlauf + Rückgängig).
#[tauri::command]
async fn move_tracks_to(
    app: AppHandle,
    state: Tab,
    ids: Vec<i64>,
    dir: String,
    copy: bool,
) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    let prog = emitter(&app, &state.label, "organize");
    let _ = app.asset_protocol_scope().allow_directory(&dir, true);
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let plan = organize::plan_to_folder(&tracks, Path::new(&dir), None);
        let stops = stop_dirs(&db.lock().unwrap());
        let name = Path::new(&dir)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let desc = format!(
            "{} {} Datei{} nach „{name}“",
            if copy { "Kopiert:" } else { "Verschoben:" },
            tracks.len(),
            if tracks.len() == 1 { "" } else { "en" }
        );
        let r = organize::apply(&db, &plan, copy, &desc, &stops, &prog)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

/// Datei umbenennen (Name ohne Endung), mit Verlauf + Rückgängig.
#[tauri::command]
async fn rename_track(state: Tab, id: i64, name: String) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &[id])?;
        let t = tracks
            .first()
            .ok_or_else(|| anyhow::anyhow!("Titel nicht gefunden"))?;
        let dir = Path::new(&t.path)
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let plan = organize::plan_to_folder(&tracks, &dir, Some(&[name.clone()]));
        let r = organize::apply(
            &db,
            &plan,
            false,
            &format!("Umbenannt: {name}"),
            &[],
            &|_, _, _| {},
        )?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

#[tauri::command]
async fn rename_preview(state: Tab, ids: Vec<i64>, pattern: String) -> R<Vec<(i64, String)>> {
    let tracks = db::tracks_by_ids(&state.db.lock().unwrap(), &ids).map_err(err)?;
    Ok(tracks
        .iter()
        .map(|t| {
            let ext = Path::new(&t.path)
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            (
                t.id,
                format!("{}{ext}", organize::filename_from_pattern(&pattern, t)),
            )
        })
        .collect())
}

/// Dateien nach Muster aus den Tags umbenennen (bleiben im selben Ordner).
#[tauri::command]
async fn rename_by_pattern(state: Tab, ids: Vec<i64>, pattern: String) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut plan = Vec::new();
        // pro Ordner planen, damit Namenskonflikte erkannt werden
        let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<db::Track>> = Default::default();
        for t in tracks {
            by_dir
                .entry(
                    Path::new(&t.path)
                        .parent()
                        .map(Path::to_path_buf)
                        .unwrap_or_default(),
                )
                .or_default()
                .push(t);
        }
        for (dir, ts) in by_dir {
            let names: Vec<String> = ts
                .iter()
                .map(|t| organize::filename_from_pattern(&pattern, t))
                .collect();
            plan.extend(organize::plan_to_folder(&ts, &dir, Some(&names)));
        }
        let n = plan.iter().filter(|m| !m.unchanged).count();
        let r = organize::apply(
            &db,
            &plan,
            false,
            &format!(
                "Umbenannt nach {pattern}: {n} Datei{}",
                if n == 1 { "" } else { "en" }
            ),
            &[],
            &|_, _, _| {},
        )?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

// ---------- Sync (USB-Stick, SD-Karte …) ----------

#[tauri::command]
async fn sync_targets(state: Tab) -> R<Vec<sync::TargetState>> {
    // Handys werden über MTP abgefragt → nicht auf dem Hauptthread
    let db = state.db.clone();
    blocking(move || sync::list(&db.lock().unwrap())).await
}

#[tauri::command]
async fn sync_save(state: Tab, target: sync::Target) -> R<i64> {
    sync::save(&state.db.lock().unwrap(), &target).map_err(err)
}

#[tauri::command]
async fn sync_delete(state: Tab, id: i64) -> R<()> {
    sync::delete(&state.db.lock().unwrap(), id).map_err(err)
}

#[tauri::command]
async fn sync_preview(state: Tab, id: i64) -> R<sync::Preview> {
    let db = state.db.clone();
    blocking(move || {
        let c = db.lock().unwrap();
        let t = sync::get(&c, id)?;
        sync::preview(&c, &t)
    })
    .await
}

#[tauri::command]
async fn sync_run(app: AppHandle, state: Tab, id: i64, force: Option<bool>) -> R<sync::Report> {
    let db = state.db.clone();
    let cancel = state.task();
    let prog = emitter(&app, &state.label, "sync");
    blocking(move || sync::run_with(&db, id, force.unwrap_or(false), &cancel, &prog)).await
}

#[derive(serde::Serialize)]
struct PhoneDevice {
    id: String,
    name: String,
}

/// Angeschlossene Handys/Player (MTP, nur Windows)
#[tauri::command]
async fn sync_devices() -> R<Vec<PhoneDevice>> {
    blocking(|| {
        Ok(sync::devices()?
            .into_iter()
            .map(|(id, name)| PhoneDevice { id, name })
            .collect())
    })
    .await
}

#[tauri::command]
async fn sync_device_folders(device: String, path: String) -> R<Vec<String>> {
    blocking(move || sync::device_folders(&device, &path)).await
}

#[tauri::command]
async fn sync_eject(path: String) -> R<()> {
    blocking(move || sync::eject(&path)).await
}

// ---------- Link-Ripper ----------

fn tools_dir(app: &AppHandle) -> PathBuf {
    app.path().app_data_dir().unwrap_or_default().join("tools")
}

#[tauri::command]
async fn rip_tools(app: AppHandle) -> R<ripper::ToolStatus> {
    let dir = tools_dir(&app);
    blocking(move || Ok(ripper::status(&dir))).await
}

#[tauri::command]
async fn rip_install(app: AppHandle, window: tauri::WebviewWindow) -> R<ripper::ToolStatus> {
    let dir = tools_dir(&app);
    let prog = emitter(&app, window.label(), "tools");
    blocking(move || ripper::install(&dir, &prog)).await
}

#[tauri::command]
async fn rip_update(app: AppHandle) -> R<String> {
    let dir = tools_dir(&app);
    blocking(move || ripper::update(&dir)).await
}

#[tauri::command]
async fn rip_resolve(app: AppHandle, state: Tab, url: String) -> R<ripper::Resolved> {
    let dir = tools_dir(&app);
    let db = state.db.clone();
    blocking(move || {
        let mut r = ripper::resolve(&dir, &url)?;
        let lib = db::all_tracks(&db.lock().unwrap())?;
        ripper::mark_known(&mut r.entries, &lib);
        Ok(r)
    })
    .await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RipReport {
    ids: Vec<i64>,
    errors: Vec<String>,
}

#[tauri::command]
async fn rip_download(
    app: AppHandle,
    state: Tab,
    entries: Vec<ripper::Entry>,
    dir: String,
    format: ripper::Format,
) -> R<RipReport> {
    let tools = tools_dir(&app);
    let db = state.db.clone();
    let cancel = state.task();
    let _ = app.asset_protocol_scope().allow_directory(&dir, true);
    let label = state.label.clone();
    blocking(move || {
        let mut rep = RipReport {
            ids: vec![],
            errors: vec![],
        };
        let total = entries.len();
        let on = |ev: ripper::RipEvent| {
            let _ = app.emit_to(&label, "rip", ev);
        };
        for (n, e) in entries.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let _ = app.emit_to(
                &label,
                "progress",
                ProgressEvent {
                    task: "rip".into(),
                    done: n,
                    total,
                    message: format!("{} – {}", e.artist, e.title),
                },
            );
            if n > 0 {
                // Schonend: kurze Pause zwischen den Titeln, damit YouTube nicht sperrt
                std::thread::sleep(std::time::Duration::from_millis(2500));
            }
            match ripper::rip_one(&tools, &db, e, Path::new(&dir), format, &cancel, &on) {
                Ok(id) => {
                    rep.ids.push(id);
                    on(ripper::RipEvent {
                        index: e.index,
                        stage: "done".into(),
                        percent: 100.0,
                        message: "fertig".into(),
                        track_id: Some(id),
                    });
                }
                Err(err) => {
                    rep.errors
                        .push(format!("{} – {}: {err}", e.artist, e.title));
                    on(ripper::RipEvent {
                        index: e.index,
                        stage: "error".into(),
                        percent: 0.0,
                        message: err.to_string(),
                        track_id: None,
                    });
                }
            }
        }
        let _ = app.emit_to(
            &label,
            "progress",
            ProgressEvent {
                task: "rip".into(),
                done: total,
                total,
                message: "Fertig".into(),
            },
        );
        sync_playlists(&db.lock().unwrap());
        Ok(rep)
    })
    .await
}

// ---------- Heimnetz (Sonos/DLNA) ----------

#[tauri::command]
async fn cast_devices() -> R<Vec<cast::Device>> {
    blocking(cast::discover).await
}

#[tauri::command]
async fn cast_play(state: Tab, device: String, id: i64, start: f64) -> R<()> {
    let db = state.db.clone();
    blocking(move || {
        let t = db::track(&db.lock().unwrap(), id)?
            .ok_or_else(|| anyhow::anyhow!("Titel nicht gefunden"))?;
        let p = PathBuf::from(&t.path);
        let cover = tags::cover(&p).ok().flatten().map(|(_, b)| b);
        let title = t.title.clone().unwrap_or_else(|| {
            p.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let meta = cast::Meta {
            title: &title,
            artist: t.artist.as_deref().unwrap_or(""),
            album: t.album.as_deref().unwrap_or(""),
            duration: t.duration,
            cover,
        };
        cast::play(&device, &p, meta, start)
    })
    .await
}

#[tauri::command]
async fn cast_pause(device: String) -> R<()> {
    blocking(move || cast::pause(&device)).await
}

#[tauri::command]
async fn cast_resume(device: String) -> R<()> {
    blocking(move || cast::resume(&device)).await
}

#[tauri::command]
async fn cast_stop(device: String) -> R<()> {
    blocking(move || cast::stop(&device)).await
}

#[tauri::command]
async fn cast_seek(device: String, sec: f64) -> R<()> {
    blocking(move || cast::seek(&device, sec)).await
}

#[tauri::command]
async fn cast_volume(device: String, volume: u8) -> R<()> {
    blocking(move || cast::set_volume(&device, volume)).await
}

#[tauri::command]
async fn cast_sound(device: String) -> R<cast::Sound> {
    blocking(move || cast::sound_get(&device)).await
}

#[tauri::command]
async fn cast_set_sound(device: String, sound: cast::Sound) -> R<()> {
    blocking(move || cast::sound_set(&device, &sound)).await
}

#[tauri::command]
async fn cast_status(device: String) -> R<cast::Status> {
    blocking(move || cast::status(&device)).await
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Beenden: Stream im Heimnetz stoppen, dann App schließen
fn quit(app: &AppHandle) {
    if let Some(dev) = cast::active_device() {
        let _ = cast::stop(&dev);
    }
    app.exit(0);
}

fn show_main(app: &AppHandle) {
    let label = app
        .try_state::<Tabs>()
        .map(|t| t.active.lock().unwrap().clone())
        .unwrap_or_else(|| "main".into());
    if let Some(w) = app
        .get_webview_window(&label)
        .or_else(|| app.get_webview_window("main"))
    {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn setup_tray(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    let show = MenuItem::with_id(app, "show", "Minimal Library öffnen", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle", "Abspielen / Pause", true, None::<&str>)?;
    let next = MenuItem::with_id(app, "next", "Nächster Titel", true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;
    let quit_i = MenuItem::with_id(app, "quit", "Beenden", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &next, &sep, &quit_i])?;
    let mut tb = TrayIconBuilder::with_id("main")
        .tooltip("Minimal Library")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| match ev.id.as_ref() {
            "show" => show_main(app),
            "toggle" => {
                let _ = app.emit("tray", "toggle");
            }
            "next" => {
                let _ = app.emit("tray", "next");
            }
            "quit" => quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = ev
            {
                show_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        tb = tb.icon(icon.clone());
    }
    tb.build(app)?;
    Ok(())
}

// ---------- Tabs ----------

#[tauri::command]
fn tabs_list(app: AppHandle) -> Vec<TabInfo> {
    app.state::<Tabs>().list()
}

/// Fenster eines Tabs holen oder (versteckt) anlegen – gleiche Größe/Lage wie das aufrufende Fenster
fn tab_window(
    app: &AppHandle,
    label: &str,
    like: &tauri::WebviewWindow,
) -> anyhow::Result<tauri::WebviewWindow> {
    if let Some(w) = app.get_webview_window(label) {
        return Ok(w);
    }
    let size = like.inner_size()?.to_logical::<f64>(like.scale_factor()?);
    let pos = like
        .outer_position()?
        .to_logical::<f64>(like.scale_factor()?);
    let mut b =
        tauri::WebviewWindowBuilder::new(app, label, tauri::WebviewUrl::App("index.html".into()))
            .title("Minimal Library")
            .inner_size(size.width, size.height)
            .position(pos.x, pos.y)
            .min_inner_size(1000.0, 640.0)
            .visible(false);
    #[cfg(target_os = "macos")]
    {
        b = b
            .decorations(true)
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true)
            .traffic_light_position(tauri::LogicalPosition::new(14.0, 17.0));
    }
    #[cfg(not(target_os = "macos"))]
    {
        b = b.decorations(false).shadow(true);
    }
    Ok(b.build()?)
}

/// Zu einem Tab wechseln: Zielfenster an gleicher Stelle zeigen, aktuelles verstecken
/// Asynchron: unter Windows blockiert das Anlegen eines Fensters in einem synchronen Befehl
#[tauri::command]
async fn tab_show(app: AppHandle, window: tauri::WebviewWindow, label: String) -> R<()> {
    show_tab(app, window, label)
}

fn show_tab(app: AppHandle, window: tauri::WebviewWindow, label: String) -> R<()> {
    if label == window.label() {
        return Ok(());
    }
    let tabs = app.state::<Tabs>();
    if !tabs.list().iter().any(|t| t.label == label) {
        return Err("Tab gibt es nicht mehr".into());
    }
    let target = tab_window(&app, &label, &window).map_err(err)?;
    // Lage und Größe übernehmen (auch maximiert)
    if window.is_maximized().unwrap_or(false) {
        let _ = target.maximize();
    } else {
        let _ = target.unmaximize();
        if let (Ok(sz), Ok(sf)) = (window.inner_size(), window.scale_factor()) {
            let _ = target.set_size(sz.to_logical::<f64>(sf));
        }
        if let Ok(p) = window.outer_position() {
            let _ = target.set_position(p);
        }
    }
    target.show().map_err(err)?;
    let _ = target.set_focus();
    let _ = window.hide();
    *tabs.active.lock().unwrap() = label;
    Ok(())
}

#[tauri::command]
async fn tab_create(app: AppHandle, window: tauri::WebviewWindow) -> R<String> {
    let tabs = app.state::<Tabs>();
    let mut v = tabs.list();
    let id = format!(
        "tab-{:x}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    );
    let n = v.len() + 1;
    v.push(TabInfo {
        label: id.clone(),
        name: format!("Bibliothek {n}"),
    });
    tabs.save(&v);
    let _ = app.emit("tabs-changed", ());
    show_tab(app, window, id.clone())?;
    Ok(id)
}

#[tauri::command]
fn tab_rename(app: AppHandle, label: String, name: String) -> R<()> {
    let tabs = app.state::<Tabs>();
    let mut v = tabs.list();
    if let Some(t) = v.iter_mut().find(|t| t.label == label) {
        t.name = name.trim().chars().take(40).collect::<String>();
        if t.name.is_empty() {
            t.name = "Bibliothek".into();
        }
    }
    tabs.save(&v);
    let _ = app.emit("tabs-changed", ());
    Ok(())
}

/// Tab schließen: Fenster zu, Bibliothek dieses Tabs (Datenbank, Einstellungen) wird gelöscht.
/// Die Musikdateien selbst bleiben unberührt. Der erste Tab lässt sich nicht schließen.
#[tauri::command]
async fn tab_close(app: AppHandle, window: tauri::WebviewWindow, label: String) -> R<()> {
    if label == "main" {
        return Err("Der erste Tab lässt sich nicht schließen".into());
    }
    let tabs = app.state::<Tabs>();
    if window.label() == label {
        show_tab(app.clone(), window.clone(), "main".into())?;
    }
    if let Some(w) = app.get_webview_window(&label) {
        let _ = w.destroy();
    }
    if let Some(st) = tabs.map.lock().unwrap().remove(&label) {
        st.cancel_all();
        *st.watcher.lock().unwrap() = None;
    }
    let v: Vec<TabInfo> = tabs
        .list()
        .into_iter()
        .filter(|t| t.label != label)
        .collect();
    tabs.save(&v);
    // Datenbank erst löschen, wenn nichts mehr darauf zugreift (kurz warten)
    let dir = tabs.dir_of(&label);
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(2));
        let _ = std::fs::remove_dir_all(dir);
    });
    let _ = app.emit("tabs-changed", ());
    Ok(())
}

/// Titel im Infobereich-Tooltip anzeigen
#[tauri::command]
fn set_tray_title(app: AppHandle, text: String) {
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_tooltip(Some(if text.is_empty() {
            "Minimal Library".to_string()
        } else {
            format!("Minimal Library – {text}")
        }));
    }
}

#[tauri::command]
fn quit_app(app: AppHandle) {
    quit(&app);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .on_window_event(|window, event| {
            // Schließen (X, Alt+F4) → in den Infobereich, Musik läuft weiter
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let to_tray = tab_state(window.app_handle(), window.label())
                    .map(|s| load_settings(&s.db.lock().unwrap()).close_to_tray)
                    .unwrap_or(true);
                if to_tray {
                    api.prevent_close();
                    let _ = window.hide();
                } else {
                    quit(window.app_handle());
                }
            }
        })
        .setup(|app| {
            setup_tray(app)?;
            let dir = app.path().app_data_dir()?;
            app.manage(Tabs {
                base: dir,
                map: Mutex::new(HashMap::new()),
                active: Mutex::new("main".into()),
            });
            let main = tab_state(app.handle(), "main")?;
            start_remote(app.handle(), &main);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_settings,
            set_settings,
            scan_library,
            add_paths,
            list_tracks,
            get_tracks,
            get_peaks,
            allow_play,
            analyze,
            cancel_task,
            remove_from_library,
            trash_tracks,
            write_tags,
            write_analysis_to_tags,
            get_cover,
            set_cover_file,
            remove_cover,
            mb_search,
            acoustid_lookup,
            mb_cover,
            find_duplicates,
            dup_ignore,
            dup_ignore_count,
            dup_ignore_clear,
            organize_preview,
            organize_apply,
            move_batches,
            undo_batch,
            playlists_list,
            playlist_create,
            playlist_update,
            playlist_delete,
            playlist_items,
            playlist_set_items,
            playlist_add_items,
            smart_preview,
            playlist_import,
            playlist_save_m3u8,
            export_size,
            export_playlists,
            sync_playlist_folder,
            pioneer_track,
            pioneer_set_grid,
            pioneer_set_cue,
            pioneer_delete_cue,
            pioneer_preview,
            pioneer_export,
            pioneer_xml,
            dir_list,
            dir_import_untracked,
            dir_create,
            dir_rename,
            dir_move,
            dir_trash,
            move_tracks_to,
            rename_track,
            rename_preview,
            rename_by_pattern,
            sync_targets,
            sync_save,
            sync_delete,
            sync_preview,
            sync_run,
            sync_eject,
            sync_devices,
            sync_device_folders,
            remote_info,
            star_ids,
            lyrics_get,
            lyrics_save,
            lyrics_fetch,
            lyrics_fetch_many,
            star_toggle,
            remote_unpair,
            rip_tools,
            rip_install,
            rip_update,
            rip_resolve,
            rip_download,
            cast_devices,
            cast_play,
            cast_pause,
            cast_resume,
            cast_stop,
            cast_seek,
            cast_volume,
            cast_status,
            cast_sound,
            cast_set_sound,
            set_tray_title,
            tabs_list,
            tab_roots,
            dup_check,
            transfer_to_tab,
            tab_show,
            tab_create,
            tab_rename,
            tab_close,
            quit_app,
            app_version,
        ])
        .build(tauri::generate_context!())
        .expect("Fehler beim Starten von Minimal Library")
        .run(|_app, event| match event {
            // macOS: Klick aufs Dock-Symbol holt das versteckte Fenster zurück
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { .. } => show_main(_app),
            // ⌘Q / Beenden: Heimnetz-Stream stoppen
            tauri::RunEvent::ExitRequested { .. } => {
                if let Some(d) = cast::active_device() {
                    let _ = cast::stop(&d);
                }
            }
            _ => {}
        });
}

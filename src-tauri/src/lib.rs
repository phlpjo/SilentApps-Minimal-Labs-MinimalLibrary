use base64::Engine;
use musiccore::{analysis, db, duplicates, export, musicbrainz, organize, playlists, scanner, tags};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{AppHandle, Emitter, Manager, State};

type Db = Arc<Mutex<Connection>>;

struct AppState {
    db: Db,
    cancel: Arc<AtomicBool>,
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

fn emitter(app: &AppHandle, task: &str) -> impl Fn(usize, usize, &str) + Sync + use<> {
    let app = app.clone();
    let task = task.to_string();
    move |done, total, msg| {
        let _ = app.emit("progress", ProgressEvent { task: task.clone(), done, total, message: msg.to_string() });
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> anyhow::Result<T> + Send + 'static) -> R<T> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(err)?.map_err(err)
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
}

impl Default for Settings {
    fn default() -> Self {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
        Settings {
            roots: vec![],
            playlist_dir: None,
            organize_target: None,
            pattern: organize::DEFAULT_PATTERN.into(),
            threads: (threads.saturating_sub(1)).clamp(1, 8),
            auto_analyze: true,
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
fn get_settings(state: State<AppState>) -> R<Settings> {
    Ok(load_settings(&state.db.lock().unwrap()))
}

#[tauri::command]
fn set_settings(app: AppHandle, state: State<AppState>, settings: Settings) -> R<()> {
    let c = state.db.lock().unwrap();
    db::set_setting(&c, "settings", &serde_json::to_string(&settings).map_err(err)?).map_err(err)?;
    for r in &settings.roots {
        let _ = app.asset_protocol_scope().allow_directory(r, true);
    }
    Ok(())
}

/// Playlist-Ordner aktuell halten (falls eingestellt).
fn sync_playlists(conn: &Connection) {
    let s = load_settings(conn);
    if let Some(dir) = s.playlist_dir.filter(|d| !d.is_empty()) {
        let _ = playlists::sync_folder(conn, Path::new(&dir));
    }
}

// ---------- Bibliothek ----------

#[tauri::command]
async fn scan_library(app: AppHandle, state: State<'_, AppState>) -> R<scanner::ScanReport> {
    let db = state.db.clone();
    let roots: Vec<PathBuf> = load_settings(&db.lock().unwrap()).roots.iter().map(PathBuf::from).collect();
    let prog = emitter(&app, "scan");
    blocking(move || {
        let r = scanner::scan(&db, &roots, &prog)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

#[tauri::command]
async fn add_paths(state: State<'_, AppState>, paths: Vec<String>) -> R<Vec<i64>> {
    let db = state.db.clone();
    blocking(move || scanner::add_files(&db, &paths.iter().map(PathBuf::from).collect::<Vec<_>>())).await
}

#[tauri::command]
async fn list_tracks(state: State<'_, AppState>) -> R<Vec<db::Track>> {
    let db = state.db.clone();
    blocking(move || db::all_tracks(&db.lock().unwrap())).await
}

#[tauri::command]
fn get_tracks(state: State<AppState>, ids: Vec<i64>) -> R<Vec<db::Track>> {
    db::tracks_by_ids(&state.db.lock().unwrap(), &ids).map_err(err)
}

/// Wellenformen als Base64 (je 2000 Bytes).
#[tauri::command]
fn get_peaks(state: State<AppState>, ids: Vec<i64>) -> R<HashMap<i64, String>> {
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
async fn analyze(app: AppHandle, state: State<'_, AppState>, ids: Option<Vec<i64>>) -> R<usize> {
    let db = state.db.clone();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let threads = load_settings(&db.lock().unwrap()).threads;
    let prog = emitter(&app, "analyze");
    let app2 = app.clone();
    blocking(move || {
        analysis::analyze_pending(&db, ids, threads, &cancel, &prog, &|id| {
            let _ = app2.emit("analyzed", id);
        })
    })
    .await
}

#[tauri::command]
fn cancel_task(state: State<AppState>) {
    state.cancel.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn remove_from_library(state: State<'_, AppState>, ids: Vec<i64>) -> R<()> {
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
async fn trash_tracks(state: State<'_, AppState>, ids: Vec<i64>, replace: Option<HashMap<String, i64>>) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        // Duplikate: Playlist-Einträge auf den behaltenen Titel umbiegen
        if let Some(map) = replace {
            let c = db.lock().unwrap();
            for (from, to) in map {
                if let Ok(from) = from.parse::<i64>() {
                    c.execute("UPDATE playlist_items SET track_id = ?2 WHERE track_id = ?1", rusqlite::params![from, to])?;
                }
            }
        }
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        let mut gone = vec![];
        for t in tracks {
            let p = Path::new(&t.path);
            if !p.exists() {
                gone.push(t.id);
                continue;
            }
            match trash::delete(p) {
                Ok(()) => gone.push(t.id),
                Err(e) => errors.push(format!("{}: {e}", t.path)),
            }
        }
        let c = db.lock().unwrap();
        db::remove_tracks(&c, &gone)?;
        sync_playlists(&c);
        Ok(errors)
    })
    .await
}

// ---------- Tags ----------

#[tauri::command]
async fn write_tags(state: State<'_, AppState>, ids: Vec<i64>, update: tags::TagUpdate) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        for t in tracks {
            let p = Path::new(&t.path);
            match tags::write(p, &update) {
                Ok(()) => {
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
        sync_playlists(&db.lock().unwrap());
        Ok(errors)
    })
    .await
}

/// Analysierte BPM/Tonart dauerhaft in die Datei-Tags schreiben.
#[tauri::command]
async fn write_analysis_to_tags(state: State<'_, AppState>, ids: Vec<i64>) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        for t in tracks {
            if t.bpm.is_none() && t.key.is_none() {
                continue;
            }
            let u = tags::TagUpdate { bpm: t.bpm.map(|b| b.to_string()), key: t.key.clone(), ..Default::default() };
            let p = Path::new(&t.path);
            match tags::write(p, &u) {
                Ok(()) => {
                    scanner::refresh(&db.lock().unwrap(), p)?;
                }
                Err(e) => errors.push(format!("{e:#}")),
            }
        }
        Ok(errors)
    })
    .await
}

#[tauri::command]
async fn get_cover(state: State<'_, AppState>, id: i64) -> R<Option<String>> {
    let db = state.db.clone();
    blocking(move || {
        let Some(t) = db::track(&db.lock().unwrap(), id)? else { return Ok(None) };
        Ok(tags::cover(Path::new(&t.path))
            .ok()
            .flatten()
            .map(|(mime, data)| format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(data))))
    })
    .await
}

fn apply_cover(db: &Db, ids: &[i64], data: Option<Vec<u8>>) -> anyhow::Result<Vec<String>> {
    let tracks = db::tracks_by_ids(&db.lock().unwrap(), ids)?;
    let mut errors = vec![];
    for t in tracks {
        let p = Path::new(&t.path);
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
    Ok(errors)
}

#[tauri::command]
async fn set_cover_file(state: State<'_, AppState>, ids: Vec<i64>, file: String) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let data = std::fs::read(&file)?;
        apply_cover(&db, &ids, Some(data))
    })
    .await
}

#[tauri::command]
async fn remove_cover(state: State<'_, AppState>, ids: Vec<i64>) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || apply_cover(&db, &ids, None)).await
}

// ---------- MusicBrainz ----------

#[tauri::command]
async fn mb_search(artist: String, title: String, album: Option<String>) -> R<Vec<musicbrainz::Candidate>> {
    blocking(move || musicbrainz::search(&artist, &title, album.as_deref())).await
}

#[tauri::command]
async fn mb_cover(state: State<'_, AppState>, ids: Vec<i64>, release_id: String) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let data = musicbrainz::cover(&release_id)?;
        apply_cover(&db, &ids, Some(data))
    })
    .await
}

// ---------- Duplikate ----------

#[tauri::command]
async fn find_duplicates(app: AppHandle, state: State<'_, AppState>, modes: duplicates::Modes) -> R<Vec<duplicates::DupGroup>> {
    let db = state.db.clone();
    let prog = emitter(&app, "duplicates");
    blocking(move || {
        let (tracks, fps) = {
            let c = db.lock().unwrap();
            (db::all_tracks(&c)?, if modes.audio { db::fingerprints(&c)? } else { HashMap::new() })
        };
        Ok(duplicates::find(&tracks, &fps, modes, &prog))
    })
    .await
}

// ---------- Sortieren ----------

fn stop_dirs(conn: &Connection) -> Vec<PathBuf> {
    let s = load_settings(conn);
    let mut v: Vec<PathBuf> = s.roots.iter().map(PathBuf::from).collect();
    if let Some(t) = s.organize_target {
        v.push(PathBuf::from(t));
    }
    v
}

#[tauri::command]
async fn organize_preview(state: State<'_, AppState>, ids: Option<Vec<i64>>, target: String, pattern: String) -> R<Vec<organize::PlannedMove>> {
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
async fn organize_apply(app: AppHandle, state: State<'_, AppState>, plan: Vec<organize::PlannedMove>, copy: bool, description: String) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    let prog = emitter(&app, "organize");
    blocking(move || {
        let stops = stop_dirs(&db.lock().unwrap());
        let r = organize::apply(&db, &plan, copy, &description, &stops, &prog)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

#[tauri::command]
fn move_batches(state: State<AppState>) -> R<Vec<organize::Batch>> {
    organize::batches(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
async fn undo_batch(state: State<'_, AppState>, id: i64) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    blocking(move || {
        let stops = stop_dirs(&db.lock().unwrap());
        let r = organize::undo(&db, id, &stops)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

// ---------- Playlisten ----------

fn delete_playlist_file(conn: &Connection, name: &str) {
    if let Some(dir) = load_settings(conn).playlist_dir.filter(|d| !d.is_empty()) {
        let _ = std::fs::remove_file(Path::new(&dir).join(format!("{}.m3u8", musiccore::paths::sanitize(name))));
    }
}

#[tauri::command]
fn playlists_list(state: State<AppState>) -> R<Vec<playlists::Playlist>> {
    playlists::list(&state.db.lock().unwrap()).map_err(err)
}

#[tauri::command]
fn playlist_create(state: State<AppState>, name: String, rules: Option<playlists::SmartRules>, ids: Option<Vec<i64>>) -> R<i64> {
    let mut c = state.db.lock().unwrap();
    let id = playlists::create(&c, &name, rules.as_ref()).map_err(err)?;
    if let Some(ids) = ids {
        playlists::set_items(&mut c, id, &ids).map_err(err)?;
    }
    sync_playlists(&c);
    Ok(id)
}

#[tauri::command]
fn playlist_update(state: State<AppState>, id: i64, name: String, rules: Option<playlists::SmartRules>) -> R<()> {
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
fn playlist_delete(state: State<AppState>, id: i64) -> R<()> {
    let c = state.db.lock().unwrap();
    if let Ok(n) = playlists::name(&c, id) {
        delete_playlist_file(&c, &n);
    }
    playlists::delete(&c, id).map_err(err)
}

#[tauri::command]
fn playlist_items(state: State<AppState>, id: i64) -> R<Vec<i64>> {
    playlists::track_ids(&state.db.lock().unwrap(), id).map_err(err)
}

#[tauri::command]
fn playlist_set_items(state: State<AppState>, id: i64, ids: Vec<i64>) -> R<()> {
    let mut c = state.db.lock().unwrap();
    playlists::set_items(&mut c, id, &ids).map_err(err)?;
    sync_playlists(&c);
    Ok(())
}

#[tauri::command]
fn playlist_add_items(state: State<AppState>, id: i64, ids: Vec<i64>) -> R<()> {
    let mut c = state.db.lock().unwrap();
    playlists::add_items(&mut c, id, &ids).map_err(err)?;
    sync_playlists(&c);
    Ok(())
}

#[tauri::command]
fn smart_preview(state: State<AppState>, rules: playlists::SmartRules) -> R<Vec<i64>> {
    let all = db::all_tracks(&state.db.lock().unwrap()).map_err(err)?;
    Ok(playlists::evaluate(&all, &rules))
}

/// M3U/M3U8 importieren: unbekannte Dateien werden zur Bibliothek hinzugefügt.
#[tauri::command]
async fn playlist_import(state: State<'_, AppState>, file: String) -> R<(i64, usize)> {
    let db = state.db.clone();
    blocking(move || {
        let paths: Vec<PathBuf> = playlists::read_m3u(Path::new(&file))?.into_iter().filter(|p| p.exists()).collect();
        let ids = scanner::add_files(&db, &paths)?;
        let name = Path::new(&file).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Import".into());
        let mut c = db.lock().unwrap();
        let id = playlists::create(&c, &name, None)?;
        playlists::set_items(&mut c, id, &ids)?;
        sync_playlists(&c);
        Ok((id, ids.len()))
    })
    .await
}

#[tauri::command]
fn playlist_save_m3u8(state: State<AppState>, id: i64, file: String) -> R<()> {
    playlists::write_m3u8(&state.db.lock().unwrap(), id, Path::new(&file)).map_err(err)
}

#[tauri::command]
fn export_size(state: State<AppState>, ids: Vec<i64>) -> R<u64> {
    export::size_of(&state.db.lock().unwrap(), &ids).map_err(err)
}

#[tauri::command]
async fn export_playlists(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>, dest: String, layout: export::Layout) -> R<export::ExportReport> {
    let db = state.db.clone();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let prog = emitter(&app, "export");
    blocking(move || export::export(&db, &ids, Path::new(&dest), layout, &cancel, &prog)).await
}

#[tauri::command]
fn sync_playlist_folder(state: State<AppState>) -> R<usize> {
    let c = state.db.lock().unwrap();
    let s = load_settings(&c);
    match s.playlist_dir.filter(|d| !d.is_empty()) {
        Some(d) => playlists::sync_folder(&c, Path::new(&d)).map(|v| v.len()).map_err(err),
        None => Err("Kein Playlist-Ordner eingestellt".into()),
    }
}

#[tauri::command]
fn app_version(app: AppHandle) -> String {
    app.package_info().version.to_string()
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            let conn = db::open(&dir.join("library.db"))?;
            for r in load_settings(&conn).roots {
                let _ = app.asset_protocol_scope().allow_directory(&r, true);
            }
            app.manage(AppState { db: Arc::new(Mutex::new(conn)), cancel: Arc::new(AtomicBool::new(false)) });
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
            mb_cover,
            find_duplicates,
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
            app_version,
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten von Minimal Library");
}

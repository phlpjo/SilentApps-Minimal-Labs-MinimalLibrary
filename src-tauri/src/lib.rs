use base64::Engine;
use musiccore::{acoustid, analysis, cast, ripper, db, dirs, duplicates, history, watch, export, musicbrainz, organize, playlists, scanner, tags};
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
    watcher: Mutex<Option<watch::Watcher>>,
}

/// Musikordner (neu) überwachen
fn restart_watcher(app: &AppHandle, state: &AppState) {
    let roots: Vec<PathBuf> = load_settings(&state.db.lock().unwrap()).roots.iter().map(PathBuf::from).collect();
    let app2 = app.clone();
    let db = state.db.clone();
    let w = watch::start(state.db.clone(), &roots, move |ch| {
        if !ch.added.is_empty() || ch.moved > 0 || ch.removed > 0 {
            sync_playlists(&db.lock().unwrap());
        }
        let _ = app2.emit("library-changed", ch);
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
    acoustid_key: String,
    rip_dir: Option<String>,
    rip_format: String,
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
            acoustid_key: String::new(),
            rip_dir: None,
            rip_format: "mp3".into(),
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
    drop(c);
    restart_watcher(&app, &state);
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
        let batch = history::begin(&db.lock().unwrap(), "trash", &format!("Papierkorb: {}", describe_tracks(&tracks)))?;
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
async fn write_tags(state: State<'_, AppState>, ids: Vec<i64>, update: tags::TagUpdate) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        let batch = history::begin(&db.lock().unwrap(), "tags", &format!("Tags ({}): {}", changed_fields(&update), describe_tracks(&tracks)))?;
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
        [t] => t.title.clone().unwrap_or_else(|| Path::new(&t.path).file_name().unwrap_or_default().to_string_lossy().to_string()),
        _ => format!("{} Titel", tracks.len()),
    }
}

fn changed_fields(u: &tags::TagUpdate) -> String {
    let f = [
        (&u.title, "Titel"), (&u.artist, "Künstler"), (&u.album, "Album"), (&u.album_artist, "Albumkünstler"), (&u.genre, "Genre"),
        (&u.year, "Jahr"), (&u.track_no, "Track"), (&u.disc_no, "Disc"), (&u.comment, "Kommentar"), (&u.bpm, "BPM"), (&u.key, "Tonart"),
    ];
    let v: Vec<&str> = f.iter().filter(|(x, _)| x.is_some()).map(|(_, n)| *n).collect();
    v.join(", ")
}

/// Analysierte BPM/Tonart dauerhaft in die Datei-Tags schreiben.
#[tauri::command]
async fn write_analysis_to_tags(state: State<'_, AppState>, ids: Vec<i64>) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut errors = vec![];
        let batch = history::begin(&db.lock().unwrap(), "tags", &format!("BPM + Tonart in Datei: {}", describe_tracks(&tracks)))?;
        let mut done = 0;
        for t in tracks {
            if t.bpm.is_none() && t.key.is_none() {
                continue;
            }
            if history::backup_tags(&db.lock().unwrap(), batch, &t, false).is_err() {
                continue;
            }
            done += 1;
            let u = tags::TagUpdate { bpm: t.bpm.map(|b| b.to_string()), key: t.key.clone(), ..Default::default() };
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
    let what = if data.is_some() { "Cover gesetzt" } else { "Cover entfernt" };
    let batch = history::begin(&db.lock().unwrap(), "tags", &format!("{what}: {}", describe_tracks(&tracks)))?;
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
async fn mb_search(state: State<'_, AppState>, artist: String, title: String, album: Option<String>) -> R<Vec<musicbrainz::Candidate>> {
    let db = state.db.clone();
    blocking(move || musicbrainz::search(Some(&db), &artist, &title, album.as_deref())).await
}

/// Titel am Klang erkennen (AcoustID)
#[tauri::command]
async fn acoustid_lookup(state: State<'_, AppState>, id: i64) -> R<Vec<musicbrainz::Candidate>> {
    let db = state.db.clone();
    blocking(move || {
        let c = db.lock().unwrap();
        let key = load_settings(&c).acoustid_key;
        let (fp, dur): (Option<Vec<u8>>, f64) = c.query_row("SELECT fingerprint, duration FROM tracks WHERE id = ?1", [id], |r| Ok((r.get(0)?, r.get(1)?)))?;
        drop(c);
        let fp = db::bytes_to_u32(&fp.unwrap_or_default());
        acoustid::lookup(Some(&db), &key, &fp, dur)
    })
    .await
}

#[tauri::command]
async fn mb_cover(state: State<'_, AppState>, ids: Vec<i64>, release_id: String) -> R<Vec<String>> {
    let db = state.db.clone();
    blocking(move || {
        let data = musicbrainz::cover(Some(&db), &release_id)?;
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
async fn undo_batch(state: State<'_, AppState>, id: Option<i64>) -> R<history::UndoReport> {
    let db = state.db.clone();
    blocking(move || {
        let id = match id {
            Some(id) => id,
            None => history::last_open(&db.lock().unwrap())?.ok_or_else(|| anyhow::anyhow!("Nichts zum Rückgängigmachen"))?,
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

// ---------- Ordneransicht ----------

#[tauri::command]
async fn dir_list(state: State<'_, AppState>, path: String) -> R<Vec<dirs::DirEntry>> {
    let db = state.db.clone();
    blocking(move || dirs::list(&db.lock().unwrap(), Path::new(&path))).await
}

/// Audiodateien im Ordner, die noch nicht in der Bibliothek sind, aufnehmen.
#[tauri::command]
async fn dir_import_untracked(state: State<'_, AppState>, path: String) -> R<Vec<i64>> {
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
async fn dir_rename(app: AppHandle, state: State<'_, AppState>, path: String, name: String) -> R<String> {
    let db = state.db.clone();
    let p = blocking(move || {
        let c = db.lock().unwrap();
        let p = dirs::rename(&c, Path::new(&path), &name)?;
        history::log_dir_move(&c, Path::new(&path), &p, &format!("Ordner umbenannt: {} → {}", Path::new(&path).file_name().unwrap_or_default().to_string_lossy(), p.file_name().unwrap_or_default().to_string_lossy()))?;
        sync_playlists(&c);
        Ok(p)
    })
    .await?;
    let _ = app.asset_protocol_scope().allow_directory(&p, true);
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
async fn dir_move(app: AppHandle, state: State<'_, AppState>, path: String, target: String) -> R<String> {
    let db = state.db.clone();
    let p = blocking(move || {
        let c = db.lock().unwrap();
        let p = dirs::move_into(&c, Path::new(&path), Path::new(&target))?;
        if !musiccore::paths::same_path(&p, Path::new(&path)) {
            history::log_dir_move(&c, Path::new(&path), &p, &format!("Ordner verschoben: {} → {}", p.file_name().unwrap_or_default().to_string_lossy(), Path::new(&target).file_name().unwrap_or_default().to_string_lossy()))?;
        }
        sync_playlists(&c);
        Ok(p)
    })
    .await?;
    let _ = app.asset_protocol_scope().allow_directory(&p, true);
    Ok(p.to_string_lossy().to_string())
}

#[tauri::command]
async fn dir_trash(state: State<'_, AppState>, path: String) -> R<usize> {
    let db = state.db.clone();
    blocking(move || {
        {
            let c = db.lock().unwrap();
            let pre = format!("{}{}", path.trim_end_matches(['/', '\\']), std::path::MAIN_SEPARATOR);
            let mut st = c.prepare("SELECT id FROM tracks WHERE substr(path, 1, ?2) = ?1")?;
            let ids: Vec<i64> = st.query_map(rusqlite::params![pre, pre.chars().count() as i64], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
            let name = Path::new(&path).file_name().unwrap_or_default().to_string_lossy().to_string();
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
async fn move_tracks_to(app: AppHandle, state: State<'_, AppState>, ids: Vec<i64>, dir: String, copy: bool) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    let prog = emitter(&app, "organize");
    let _ = app.asset_protocol_scope().allow_directory(&dir, true);
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let plan = organize::plan_to_folder(&tracks, Path::new(&dir), None);
        let stops = stop_dirs(&db.lock().unwrap());
        let name = Path::new(&dir).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let desc = format!("{} {} Datei{} nach „{name}“", if copy { "Kopiert:" } else { "Verschoben:" }, tracks.len(), if tracks.len() == 1 { "" } else { "en" });
        let r = organize::apply(&db, &plan, copy, &desc, &stops, &prog)?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

/// Datei umbenennen (Name ohne Endung), mit Verlauf + Rückgängig.
#[tauri::command]
async fn rename_track(state: State<'_, AppState>, id: i64, name: String) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &[id])?;
        let t = tracks.first().ok_or_else(|| anyhow::anyhow!("Titel nicht gefunden"))?;
        let dir = Path::new(&t.path).parent().map(Path::to_path_buf).unwrap_or_default();
        let plan = organize::plan_to_folder(&tracks, &dir, Some(&[name.clone()]));
        let r = organize::apply(&db, &plan, false, &format!("Umbenannt: {name}"), &[], &|_, _, _| {})?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
}

#[tauri::command]
fn rename_preview(state: State<AppState>, ids: Vec<i64>, pattern: String) -> R<Vec<(i64, String)>> {
    let tracks = db::tracks_by_ids(&state.db.lock().unwrap(), &ids).map_err(err)?;
    Ok(tracks
        .iter()
        .map(|t| {
            let ext = Path::new(&t.path).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            (t.id, format!("{}{ext}", organize::filename_from_pattern(&pattern, t)))
        })
        .collect())
}

/// Dateien nach Muster aus den Tags umbenennen (bleiben im selben Ordner).
#[tauri::command]
async fn rename_by_pattern(state: State<'_, AppState>, ids: Vec<i64>, pattern: String) -> R<organize::ApplyReport> {
    let db = state.db.clone();
    blocking(move || {
        let tracks = db::tracks_by_ids(&db.lock().unwrap(), &ids)?;
        let mut plan = Vec::new();
        // pro Ordner planen, damit Namenskonflikte erkannt werden
        let mut by_dir: std::collections::BTreeMap<PathBuf, Vec<db::Track>> = Default::default();
        for t in tracks {
            by_dir.entry(Path::new(&t.path).parent().map(Path::to_path_buf).unwrap_or_default()).or_default().push(t);
        }
        for (dir, ts) in by_dir {
            let names: Vec<String> = ts.iter().map(|t| organize::filename_from_pattern(&pattern, t)).collect();
            plan.extend(organize::plan_to_folder(&ts, &dir, Some(&names)));
        }
        let n = plan.iter().filter(|m| !m.unchanged).count();
        let r = organize::apply(&db, &plan, false, &format!("Umbenannt nach {pattern}: {n} Datei{}", if n == 1 { "" } else { "en" }), &[], &|_, _, _| {})?;
        sync_playlists(&db.lock().unwrap());
        Ok(r)
    })
    .await
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
async fn rip_install(app: AppHandle) -> R<ripper::ToolStatus> {
    let dir = tools_dir(&app);
    let prog = emitter(&app, "tools");
    blocking(move || ripper::install(&dir, &prog)).await
}

#[tauri::command]
async fn rip_update(app: AppHandle) -> R<String> {
    let dir = tools_dir(&app);
    blocking(move || ripper::update(&dir)).await
}

#[tauri::command]
async fn rip_resolve(app: AppHandle, state: State<'_, AppState>, url: String) -> R<ripper::Resolved> {
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
async fn rip_download(app: AppHandle, state: State<'_, AppState>, entries: Vec<ripper::Entry>, dir: String, format: ripper::Format) -> R<RipReport> {
    let tools = tools_dir(&app);
    let db = state.db.clone();
    let cancel = state.cancel.clone();
    cancel.store(false, Ordering::Relaxed);
    let _ = app.asset_protocol_scope().allow_directory(&dir, true);
    blocking(move || {
        let mut rep = RipReport { ids: vec![], errors: vec![] };
        let total = entries.len();
        let on = |ev: ripper::RipEvent| {
            let _ = app.emit("rip", ev);
        };
        for (n, e) in entries.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let _ = app.emit("progress", ProgressEvent { task: "rip".into(), done: n, total, message: format!("{} – {}", e.artist, e.title) });
            match ripper::rip_one(&tools, &db, e, Path::new(&dir), format, &cancel, &on) {
                Ok(id) => {
                    rep.ids.push(id);
                    on(ripper::RipEvent { index: e.index, stage: "done".into(), percent: 100.0, message: "fertig".into(), track_id: Some(id) });
                }
                Err(err) => {
                    rep.errors.push(format!("{} – {}: {err}", e.artist, e.title));
                    on(ripper::RipEvent { index: e.index, stage: "error".into(), percent: 0.0, message: err.to_string(), track_id: None });
                }
            }
        }
        let _ = app.emit("progress", ProgressEvent { task: "rip".into(), done: total, total, message: "Fertig".into() });
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
async fn cast_play(state: State<'_, AppState>, device: String, id: i64, start: f64) -> R<()> {
    let db = state.db.clone();
    blocking(move || {
        let t = db::track(&db.lock().unwrap(), id)?.ok_or_else(|| anyhow::anyhow!("Titel nicht gefunden"))?;
        let p = PathBuf::from(&t.path);
        let cover = tags::cover(&p).ok().flatten().map(|(_, b)| b);
        let title = t.title.clone().unwrap_or_else(|| p.file_stem().unwrap_or_default().to_string_lossy().to_string());
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
async fn cast_status(device: String) -> R<cast::Status> {
    blocking(move || cast::status(&device)).await
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
            musiccore::webcache::purge(&conn); // Online-Zwischenspeicher nach 3 Tagen löschen
            for r in load_settings(&conn).roots {
                let _ = app.asset_protocol_scope().allow_directory(&r, true);
            }
            app.manage(AppState { db: Arc::new(Mutex::new(conn)), cancel: Arc::new(AtomicBool::new(false)), watcher: Mutex::new(None) });
            restart_watcher(app.handle(), &app.state::<AppState>());
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
            app_version,
        ])
        .run(tauri::generate_context!())
        .expect("Fehler beim Starten von Minimal Library");
}

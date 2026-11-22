//! Sync: Playlisten samt Musikdateien auf USB-Sticks, SD-Karten, Festplatten …
//! Mehrere Ziele, je Ziel eigene Playlisten. Nur Fehlendes wird kopiert; was die App früher
//! dorthin kopiert hat und nicht mehr gebraucht wird, kann sie wieder entfernen (nie fremde Dateien).

use crate::db::{self, Track};
use crate::{Progress, now, organize, paths, playlists};
use anyhow::{Result, anyhow};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

const MANIFEST: &str = ".minimal-library-sync.json";
const MUSIC_DIR: &str = "Musik";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Target {
    pub id: i64,
    pub name: String,
    /// Ordner auf dem Gerät, z. B. "E:\\" oder "E:\\Musik"
    pub path: String,
    pub playlists: Vec<i64>,
    /// Sofort synchronisieren, sobald das Ziel erreichbar ist
    pub auto: bool,
    /// Früher kopierte, nicht mehr benötigte Dateien entfernen
    pub mirror: bool,
    pub last_sync: i64,
    /// Aufbau wie beim Export: "numbered" (Ordner je Playlist, nummeriert), "flat" (Ordner je Playlist),
    /// "structured" (gemeinsamer Musikordner Künstler/Album, .m3u8 im Ziel). Leer = "structured" (alte Ziele)
    pub layout: String,
    /// Keine .m3u8-Dateien schreiben (Standard: schreiben)
    pub no_playlist_files: bool,
    /// Ganze Ordner (eine Richtung: Quellordner → Ziel). Auf dem Ziel als „<Ordnername>/…“ mit
    /// gleicher Unterordner-Struktur.
    pub folders: Vec<String>,
    /// Freien Platz nicht prüfen (Cloud-/Netzlaufwerke melden oft nur den lokalen Zwischenspeicher)
    pub ignore_space: bool,
    /// Handy/Player über MTP (nur Windows): Gerätekennung; leer = Ordner/Laufwerk.
    /// `path` ist dann der Ordner auf dem Gerät, z. B. „Interner gemeinsamer Speicher/Music“.
    pub device: String,
    pub device_name: String,
    /// Handy im WLAN (`device` = "wlan:<Kennung>"): Schlüssel aus der Kopplung
    pub token: String,
}

const WLAN: &str = "wlan:";

impl Target {
    fn wlan_id(&self) -> Option<&str> {
        self.device.strip_prefix(WLAN)
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetState {
    #[serde(flatten)]
    pub target: Target,
    pub available: bool,
    pub free: Option<u64>,
}

fn ensure(c: &Connection) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_targets (id INTEGER PRIMARY KEY, data TEXT NOT NULL);",
    )?;
    Ok(())
}

pub fn list(c: &Connection) -> Result<Vec<TargetState>> {
    ensure(c)?;
    let mut st = c.prepare("SELECT id, data FROM sync_targets ORDER BY id")?;
    let rows: Vec<(i64, String)> = st
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, data)| {
            let mut t: Target = serde_json::from_str(&data).ok()?;
            t.id = id;
            if !t.device.is_empty() {
                let (available, free) = device_status(&t);
                return Some(TargetState {
                    available,
                    free,
                    target: t,
                });
            }
            let p = Path::new(&t.path);
            let available = !t.path.is_empty() && p.is_dir();
            Some(TargetState {
                free: if available {
                    fs4::available_space(p).ok()
                } else {
                    None
                },
                available,
                target: t,
            })
        })
        .collect())
}

pub fn get(c: &Connection, id: i64) -> Result<Target> {
    ensure(c)?;
    let data: String = c
        .query_row("SELECT data FROM sync_targets WHERE id = ?1", [id], |r| {
            r.get(0)
        })
        .map_err(|_| anyhow!("Sync-Ziel nicht gefunden"))?;
    let mut t: Target = serde_json::from_str(&data)?;
    t.id = id;
    Ok(t)
}

pub fn save(c: &Connection, t: &Target) -> Result<i64> {
    ensure(c)?;
    let data = serde_json::to_string(t)?;
    if t.id > 0 {
        c.execute(
            "UPDATE sync_targets SET data = ?2 WHERE id = ?1",
            params![t.id, data],
        )?;
        Ok(t.id)
    } else {
        c.execute("INSERT INTO sync_targets(data) VALUES(?1)", [data])?;
        Ok(c.last_insert_rowid())
    }
}

pub fn delete(c: &Connection, id: i64) -> Result<()> {
    ensure(c)?;
    c.execute("DELETE FROM sync_targets WHERE id = ?1", [id])?;
    let _ = c.execute("DELETE FROM sync_manifests WHERE id = ?1", [id]);
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Manifest {
    files: BTreeSet<String>,
    playlists: BTreeSet<String>,
}

/// Zielpfad auf dem Gerät – genau wie „Playlisten exportieren“ (ohne Endung)
fn rel_of(t: &Track, layout: &str, playlist: &str, pos: usize, digits: usize) -> String {
    let stem = match layout {
        "numbered" => format!(
            "{}/{}",
            paths::sanitize(playlist),
            paths::sanitize(&format!("{:0digits$} - {}", pos, crate::export::label(t)))
        ),
        "flat" => format!(
            "{}/{}",
            paths::sanitize(playlist),
            paths::sanitize(&crate::export::label(t))
        ),
        _ => format!(
            "{MUSIC_DIR}/{}",
            organize::render("{albumartist}/{album}/{disc-}{track} - {title}", t).to_string_lossy()
        ),
    };
    stem.replace('\\', "/")
}

/// Ein Ordner je Playlist (nummeriert oder nicht)?
fn per_playlist(layout: &str) -> bool {
    matches!(layout, "numbered" | "flat")
}

fn ext_of(t: &Track) -> String {
    Path::new(&t.path)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

struct Plan {
    /// (Quelle, relativer Zielpfad, Größe)
    files: Vec<(PathBuf, String, u64)>,
    /// (relativer Pfad der .m3u8, Inhalt)
    playlists: Vec<(String, String)>,
    /// (Playlist-ID, Name, Dateien in Reihenfolge) – fürs Handy im WLAN (legt echte Playlisten an)
    lists: Vec<(i64, String, Vec<String>)>,
}

fn plan(c: &Connection, t: &Target) -> Result<Plan> {
    let layout = if t.layout.is_empty() {
        "structured"
    } else {
        t.layout.as_str()
    };
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    // Titel → Zielpfad (bei „je Playlist“ pro Playlist eigene Kopie)
    let mut by_key: HashMap<(i64, i64), String> = HashMap::new();
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut pls = Vec::new();
    let mut lists = Vec::new();
    let root = Path::new(&t.path);
    for &pid in &t.playlists {
        let Ok(name) = playlists::name(c, pid) else {
            continue;
        };
        let tracks = db::tracks_by_ids(c, &playlists::track_ids(c, pid)?)?;
        let digits = tracks.len().to_string().len().max(2);
        let mut on_device: Vec<Track> = Vec::new();
        let mut rels: Vec<String> = Vec::new();
        for (i, tr) in tracks.into_iter().enumerate() {
            let key = (if per_playlist(layout) { pid } else { 0 }, tr.id);
            let rel = by_key.entry(key).or_insert_with(|| {
                let stem = rel_of(&tr, layout, &name, i + 1, digits);
                let ext = ext_of(&tr);
                // gleiche Zielnamen (zwei Titel mit gleichem Namen) auseinanderhalten
                let n = used
                    .entry(format!("{stem}.{ext}").to_lowercase())
                    .or_insert(0);
                *n += 1;
                let r = if *n > 1 {
                    format!("{stem} ({n}).{ext}")
                } else {
                    format!("{stem}.{ext}")
                };
                files.push((PathBuf::from(&tr.path), r.clone(), tr.size.max(0) as u64));
                r
            });
            rels.push(rel.clone());
            let mut copy = tr.clone();
            copy.path = root.join(&*rel).to_string_lossy().to_string();
            on_device.push(copy);
        }
        lists.push((pid, name.clone(), rels));
        if t.no_playlist_files {
            continue;
        }
        let file = format!("{}.m3u8", paths::sanitize(&name));
        // bei „Ordner je Playlist“ liegt die Playlist-Datei im Ordner der Playlist
        let (m3u_rel, base) = if per_playlist(layout) {
            (
                format!("{}/{file}", paths::sanitize(&name)),
                root.join(paths::sanitize(&name)),
            )
        } else {
            (file, root.to_path_buf())
        };
        pls.push((m3u_rel, playlists::m3u8(&on_device, &base)));
    }
    // Ordner: Struktur 1:1 übernehmen – Ziel/<Ordnername>/<Unterordner>/<Datei>
    if !t.folders.is_empty() {
        let all = db::all_tracks(c)?;
        for folder in &t.folders {
            let src = PathBuf::from(folder);
            let name = src
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Ordner".into());
            let mut tracks: Vec<&Track> = all
                .iter()
                .filter(|tr| crate::dirs::under_any(&tr.path, std::slice::from_ref(&src)))
                .collect();
            tracks.sort_by(|a, b| a.path.cmp(&b.path));
            for tr in tracks {
                // Pfad unterhalb des Quellordners (Präfix ist durch under_any gesichert)
                let base_len = folder.trim_end_matches(['/', '\\']).len() + 1;
                let Some(rel) = tr.path.get(base_len..) else {
                    continue;
                };
                let r = format!("{}/{}", paths::sanitize(&name), rel.replace('\\', "/"));
                let n = used.entry(r.to_lowercase()).or_insert(0);
                *n += 1;
                if *n > 1 {
                    continue; // schon aus einer Playlist/einem anderen Ordner an genau dieser Stelle
                }
                files.push((PathBuf::from(&tr.path), r, tr.size.max(0) as u64));
            }
        }
    }
    Ok(Plan {
        files,
        playlists: pls,
        lists,
    })
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Preview {
    pub to_copy: usize,
    pub copy_bytes: u64,
    pub up_to_date: usize,
    pub to_remove: usize,
    pub total_bytes: u64,
}

fn read_manifest(root: &Path) -> Manifest {
    std::fs::read(root.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Handys (MTP): Manifest liegt in der Datenbank, nicht auf dem Gerät
fn db_manifest(c: &Connection, id: i64) -> Manifest {
    let _ = c.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_manifests (id INTEGER PRIMARY KEY, data TEXT NOT NULL);",
    );
    c.query_row("SELECT data FROM sync_manifests WHERE id = ?1", [id], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|s| serde_json::from_str(&s).ok())
    .unwrap_or_default()
}

fn save_db_manifest(c: &Connection, id: i64, m: &Manifest) -> Result<()> {
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS sync_manifests (id INTEGER PRIMARY KEY, data TEXT NOT NULL);",
    )?;
    c.execute(
        "INSERT OR REPLACE INTO sync_manifests(id, data) VALUES(?1, ?2)",
        params![id, serde_json::to_string(m)?],
    )?;
    Ok(())
}

/// Ziel eines Syncs: Ordner (Stick, Karte, Laufwerk) oder Gerät über MTP (Handy)
trait Dest {
    /// Datei schon da und gleich groß?
    fn has(&mut self, rel: &str, size: u64) -> bool;
    fn exists(&mut self, rel: &str) -> bool;
    fn free(&mut self) -> Option<u64>;
    fn put(&mut self, src: &Path, rel: &str, size: u64, cancel: &AtomicBool) -> Result<()>;
    fn write(&mut self, rel: &str, data: &[u8]) -> Result<()>;
    fn remove(&mut self, rel: &str) -> bool;
    /// leere Ordner oberhalb von `rel` entfernen (nie das Ziel selbst)
    fn tidy(&mut self, rel: &str);
    /// Handy im WLAN: Anzahl für die Fortschrittsanzeige auf dem Handy
    fn begin(&mut self, _copies: usize) {}
    /// Handy im WLAN: Playlisten übergeben, Bibliothek auf dem Handy neu einlesen
    fn finish(&mut self, _lists: &[(i64, String, Vec<String>)]) -> Result<()> {
        Ok(())
    }
}

struct FsDest {
    root: PathBuf,
}

impl Dest for FsDest {
    fn has(&mut self, rel: &str, size: u64) -> bool {
        std::fs::metadata(self.root.join(rel))
            .map(|m| m.len() == size)
            .unwrap_or(false)
    }
    fn exists(&mut self, rel: &str) -> bool {
        self.root.join(rel).exists()
    }
    fn free(&mut self) -> Option<u64> {
        fs4::available_space(&self.root).ok()
    }
    fn put(&mut self, src: &Path, rel: &str, _size: u64, _cancel: &AtomicBool) -> Result<()> {
        let dst = self.root.join(rel);
        if let Some(d) = dst.parent() {
            std::fs::create_dir_all(d)?;
        }
        // erst als .part schreiben → nie halbe Dateien auf dem Gerät
        let tmp = dst.with_extension("part");
        std::fs::copy(src, &tmp)?;
        std::fs::rename(&tmp, &dst)?;
        Ok(())
    }
    fn write(&mut self, rel: &str, data: &[u8]) -> Result<()> {
        let f = self.root.join(rel);
        if let Some(d) = f.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(f, data)?;
        Ok(())
    }
    fn remove(&mut self, rel: &str) -> bool {
        std::fs::remove_file(self.root.join(rel)).is_ok()
    }
    fn tidy(&mut self, rel: &str) {
        if let Some(d) = self.root.join(rel).parent() {
            paths::remove_empty_dirs(d.to_path_buf(), std::slice::from_ref(&self.root));
        }
    }
}

/// Handy/Player über WPD (nur Windows)
#[cfg(windows)]
mod mtp {
    use super::*;
    use std::collections::HashSet;
    use std::time::{Duration, Instant};

    pub struct MtpDest {
        s: wpd::Session,
        path: String,
        tree: wpd::Tree,
    }

    fn split(rel: &str) -> (&str, &str) {
        rel.rsplit_once('/').unwrap_or(("", rel))
    }

    impl MtpDest {
        pub fn open(device: &str, path: &str) -> Result<MtpDest> {
            let s = wpd::Session::open(device)?;
            // Zielordner suchen, fehlende Teile anlegen (der Speicher selbst muss existieren)
            let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
            let storage = parts
                .first()
                .ok_or_else(|| anyhow!("Kein Speicher auf dem Gerät gewählt"))?;
            let mut cur = s
                .storages()?
                .into_iter()
                .find(|o| o.name.eq_ignore_ascii_case(storage))
                .ok_or_else(|| anyhow!("Speicher „{storage}“ nicht gefunden – ist das Handy entsperrt und auf „Dateiübertragung“ gestellt?"))?
                .id;
            for p in &parts[1..] {
                cur = match s
                    .children(&cur)?
                    .into_iter()
                    .find(|o| o.folder && o.name.eq_ignore_ascii_case(p))
                {
                    Some(o) => o.id,
                    None => s.mkdir(&cur, p)?,
                };
            }
            let tree = s.tree(&cur)?;
            Ok(MtpDest {
                s,
                path: path.to_string(),
                tree,
            })
        }

        fn dir(&mut self, rel_dir: &str) -> Result<String> {
            let mut key = String::new();
            let mut cur = self.tree.dirs.get("").cloned().unwrap_or_default();
            for part in rel_dir.split('/').filter(|p| !p.is_empty()) {
                key = if key.is_empty() {
                    part.to_lowercase()
                } else {
                    format!("{key}/{}", part.to_lowercase())
                };
                cur = match self.tree.dirs.get(&key) {
                    Some(id) => id.clone(),
                    None => {
                        let id = self.s.mkdir(&cur, part)?;
                        self.tree.dirs.insert(key.clone(), id.clone());
                        id
                    }
                };
            }
            Ok(cur)
        }

        fn upload(
            &mut self,
            rel: &str,
            data: &mut dyn std::io::Read,
            size: u64,
            cancel: &AtomicBool,
        ) -> Result<()> {
            let (d, name) = split(rel);
            let parent = self.dir(d)?;
            // MTP kann nicht überschreiben: alte Fassung zuerst löschen
            if let Some(old) = self.tree.files.remove(&rel.to_lowercase()) {
                let _ = self.s.delete(&old.id);
            }
            let id = self.s.upload(&parent, name, data, size, &|| {
                cancel.load(Ordering::Relaxed)
            })?;
            self.tree.files.insert(
                rel.to_lowercase(),
                wpd::Obj {
                    id,
                    name: name.to_string(),
                    folder: false,
                    size,
                },
            );
            Ok(())
        }
    }

    impl Dest for MtpDest {
        fn has(&mut self, rel: &str, size: u64) -> bool {
            self.tree
                .files
                .get(&rel.to_lowercase())
                .is_some_and(|o| o.size == size)
        }
        fn exists(&mut self, rel: &str) -> bool {
            self.tree.files.contains_key(&rel.to_lowercase())
        }
        fn free(&mut self) -> Option<u64> {
            self.s.free(&self.path)
        }
        fn put(&mut self, src: &Path, rel: &str, size: u64, cancel: &AtomicBool) -> Result<()> {
            let mut f = std::io::BufReader::with_capacity(1 << 20, std::fs::File::open(src)?);
            let size = std::fs::metadata(src).map(|m| m.len()).unwrap_or(size);
            self.upload(rel, &mut f, size, cancel)
        }
        fn write(&mut self, rel: &str, data: &[u8]) -> Result<()> {
            self.upload(
                rel,
                &mut std::io::Cursor::new(data),
                data.len() as u64,
                &AtomicBool::new(false),
            )
        }
        fn remove(&mut self, rel: &str) -> bool {
            match self.tree.files.remove(&rel.to_lowercase()) {
                Some(o) => self.s.delete(&o.id).is_ok(),
                None => false,
            }
        }
        fn tidy(&mut self, rel: &str) {
            let mut d = split(&rel.to_lowercase()).0.to_string();
            while !d.is_empty() {
                let Some(id) = self.tree.dirs.get(&d).cloned() else {
                    break;
                };
                let prefix = format!("{d}/");
                let used = self.tree.files.keys().any(|k| k.starts_with(&prefix))
                    || self.tree.dirs.keys().any(|k| k.starts_with(&prefix));
                // nur leere Ordner – vorsichtshalber auch auf dem Gerät nachsehen (fremde Dateien!)
                if used
                    || !self.s.children(&id).map(|c| c.is_empty()).unwrap_or(false)
                    || self.s.delete(&id).is_err()
                {
                    break;
                }
                self.tree.dirs.remove(&d);
                d = split(&d).0.to_string();
            }
        }
    }

    // ---- Status für die Anzeige (alle 3 s abgefragt → freier Platz zwischengespeichert) ----

    type FreeCache = HashMap<String, (Instant, Option<u64>)>;
    static FREE: Mutex<Option<FreeCache>> = Mutex::new(None);
    pub static BUSY: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    pub fn resolve(id: &str, name: &str) -> Option<String> {
        let list = wpd::devices().ok()?;
        if let Some(d) = list.iter().find(|d| d.id == id) {
            return Some(d.id.clone());
        }
        // gleiches Gerät, neue Kennung (anderer USB-Anschluss): eindeutig über den Namen
        let same: Vec<_> = list
            .iter()
            .filter(|d| !name.is_empty() && d.name == name)
            .collect();
        if same.len() == 1 {
            Some(same[0].id.clone())
        } else {
            None
        }
    }

    pub fn status(id: &str, name: &str, path: &str) -> (bool, Option<u64>) {
        let Some(dev) = resolve(id, name) else {
            return (false, None);
        };
        let mut g = FREE.lock().unwrap();
        let cache = g.get_or_insert_with(HashMap::new);
        let key = format!("{dev}|{path}");
        let busy = BUSY
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|b| b.contains(&dev));
        match cache.get(&key) {
            Some((at, f)) if busy || at.elapsed() < Duration::from_secs(30) => (true, *f),
            _ => {
                drop(g);
                let f = wpd::Session::open(&dev).ok().and_then(|s| s.free(path));
                FREE.lock()
                    .unwrap()
                    .get_or_insert_with(HashMap::new)
                    .insert(key, (Instant::now(), f));
                (true, f)
            }
        }
    }

    pub fn forget_free() {
        if let Some(m) = FREE.lock().unwrap().as_mut() {
            m.clear();
        }
    }
}

/// Angeschlossene Handys/Player: (Kennung, Name)
pub fn devices() -> Result<Vec<(String, String)>> {
    #[cfg(windows)]
    {
        Ok(wpd::devices()?
            .into_iter()
            .map(|d| (d.id, d.name))
            .collect())
    }
    #[cfg(not(windows))]
    {
        Ok(vec![])
    }
}

/// Unterordner auf dem Gerät; `path` leer = Speicher (z. B. „Interner gemeinsamer Speicher“)
pub fn device_folders(device: &str, path: &str) -> Result<Vec<String>> {
    #[cfg(windows)]
    {
        let s = wpd::Session::open(device)?;
        let id = if path.trim_matches(['/', '\\']).is_empty() {
            "DEVICE".to_string()
        } else {
            s.find(path)?
                .ok_or_else(|| anyhow!("Ordner nicht gefunden"))?
        };
        let mut v: Vec<String> = s
            .children(&id)?
            .into_iter()
            .filter(|o| o.folder && !o.name.starts_with('.'))
            .map(|o| o.name)
            .collect();
        v.sort_by_key(|n| n.to_lowercase());
        Ok(v)
    }
    #[cfg(not(windows))]
    {
        let _ = (device, path);
        Err(anyhow!("Handys als Ziel gibt es bisher nur unter Windows"))
    }
}

fn device_status(t: &Target) -> (bool, Option<u64>) {
    if t.wlan_id().is_some() {
        return (false, None); // WLAN-Ziele gibt es nicht mehr (ersetzt durch „offline verfügbar“ in der App)
    }
    #[cfg(windows)]
    {
        mtp::status(&t.device, &t.device_name, &t.path)
    }
    #[cfg(not(windows))]
    {
        let _ = t;
        (false, None)
    }
}

fn open_dest(t: &Target) -> Result<Box<dyn Dest>> {
    if t.device.is_empty() {
        let root = PathBuf::from(&t.path);
        if !root.is_dir() {
            return Err(anyhow!("„{}“ ist nicht verbunden", t.name));
        }
        return Ok(Box::new(FsDest { root }));
    }
    if t.wlan_id().is_some() {
        return Err(anyhow!(
            "„{}“: Handy im WLAN gibt es als Sync-Ziel nicht mehr – in Minimal Player Connected unter „PC“ Ordner oder Playlisten offline verfügbar machen. Ziel bitte löschen.",
            t.name
        ));
    }
    #[cfg(windows)]
    {
        let dev = mtp::resolve(&t.device, &t.device_name)
            .ok_or_else(|| anyhow!("„{}“ ist nicht verbunden", t.name))?;
        Ok(Box::new(mtp::MtpDest::open(&dev, &t.path)?))
    }
    #[cfg(not(windows))]
    {
        Err(anyhow!("Handys als Ziel gibt es bisher nur unter Windows"))
    }
}

/// Was würde passieren? (für die Anzeige „bereit: 12 neue Titel“)
pub fn preview(c: &Connection, t: &Target) -> Result<Preview> {
    let p = plan(c, t)?;
    let mut pv = Preview::default();
    let wanted: BTreeSet<String> = p.files.iter().map(|f| f.1.clone()).collect();
    if !t.device.is_empty() {
        // Handy: nicht jedes Mal das Gerät durchsuchen (MTP ist langsam) – Stand vom letzten Sync
        let m = db_manifest(c, t.id);
        for (_, rel, size) in &p.files {
            pv.total_bytes += size;
            if m.files.contains(rel) {
                pv.up_to_date += 1;
            } else {
                pv.to_copy += 1;
                pv.copy_bytes += size;
            }
        }
        if t.mirror {
            pv.to_remove = m.files.iter().filter(|f| !wanted.contains(*f)).count();
        }
        return Ok(pv);
    }
    let root = Path::new(&t.path);
    let mut fs = FsDest {
        root: root.to_path_buf(),
    };
    for (_, rel, size) in &p.files {
        pv.total_bytes += size;
        if root.is_dir() && fs.has(rel, *size) {
            pv.up_to_date += 1;
        } else {
            pv.to_copy += 1;
            pv.copy_bytes += size;
        }
    }
    if t.mirror && root.is_dir() {
        pv.to_remove = read_manifest(root)
            .files
            .iter()
            .filter(|f| !wanted.contains(*f))
            .count();
    }
    Ok(pv)
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Report {
    pub copied: usize,
    pub skipped: usize,
    pub removed: usize,
    pub bytes: u64,
    pub errors: Vec<String>,
}

pub fn run(
    conn: &Mutex<Connection>,
    id: i64,
    cancel: &AtomicBool,
    progress: Progress,
) -> Result<Report> {
    run_with(conn, id, false, cancel, progress)
}

/// `force = true`: trotz zu wenig gemeldetem Platz synchronisieren
pub fn run_with(
    conn: &Mutex<Connection>,
    id: i64,
    force: bool,
    cancel: &AtomicBool,
    progress: Progress,
) -> Result<Report> {
    let (t, p) = {
        let c = conn.lock().unwrap();
        let t = get(&c, id)?;
        let p = plan(&c, &t)?;
        (t, p)
    };
    let device = !t.device.is_empty();
    if device {
        progress(0, p.files.len().max(1), "Handy wird gelesen …");
    }
    let device_is_wlan = t.wlan_id().is_some();
    #[cfg(windows)]
    let _busy = (device && !device_is_wlan).then(|| Busy::new(&t.device));
    let _ = device_is_wlan;
    let mut dest = open_dest(&t)?;
    let needed: u64 = p
        .files
        .iter()
        .filter(|(_, r, z)| !dest.has(r, *z))
        .map(|f| f.2)
        .sum();
    let check = !force && !t.ignore_space;
    if check
        && let Some(free) = dest.free()
        && needed > free + 1024 * 1024
    {
        {
            // „NOSPACE:“ erkennt die Oberfläche und bietet „Trotzdem synchronisieren“ an
            return Err(anyhow!(
                "NOSPACE:Nicht genug Platz auf „{}“: {:.1} GB nötig, {:.1} GB frei",
                t.name,
                needed as f64 / 1e9,
                free as f64 / 1e9
            ));
        }
    }
    let root = PathBuf::from(&t.path);
    let mut rep = Report::default();
    let old = if device {
        db_manifest(&conn.lock().unwrap(), id)
    } else {
        read_manifest(&root)
    };
    let mut man = Manifest::default();
    let total = p.files.len();
    let copies = p.files.iter().filter(|(_, r, z)| !dest.has(r, *z)).count();
    dest.begin(copies);
    for (i, (src, rel, size)) in p.files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            rep.errors.push("Abgebrochen".into());
            break;
        }
        progress(i, total, rel.rsplit('/').next().unwrap_or(rel));
        man.files.insert(rel.clone());
        if dest.has(rel, *size) {
            rep.skipped += 1;
            put_lyrics(dest.as_mut(), src, rel);
            continue;
        }
        match dest.put(src, rel, *size, cancel) {
            Ok(()) => {
                rep.copied += 1;
                rep.bytes += size;
                put_lyrics(dest.as_mut(), src, rel);
            }
            Err(e) => {
                man.files.remove(rel);
                if cancel.load(Ordering::Relaxed) {
                    rep.errors.push("Abgebrochen".into());
                    break;
                }
                rep.errors.push(format!("{}: {e}", src.display()));
                // Handy abgezogen/gesperrt → nicht für jede weitere Datei erneut scheitern
                if device && rep.copied == 0 && rep.errors.len() >= 3 {
                    rep.errors
                        .push("Abgebrochen – ist das Handy noch verbunden und entsperrt?".into());
                    break;
                }
            }
        }
    }
    // Playlisten schreiben
    for (rel, content) in &p.playlists {
        if dest.write(rel, content.as_bytes()).is_ok() {
            man.playlists.insert(rel.clone());
        }
    }
    // Nicht mehr gewählte Playlisten der App entfernen (immer) – Musikdateien nur bei „aufräumen“
    for rel in old.playlists.difference(&man.playlists) {
        if dest.remove(rel) {
            dest.tidy(rel);
        }
    }
    if t.mirror && !cancel.load(Ordering::Relaxed) {
        for rel in old.files.difference(&man.files) {
            if dest.remove(rel) {
                rep.removed += 1;
                let _ = dest.remove(&lrc_rel(rel));
                dest.tidy(rel);
            }
        }
    } else {
        // nicht gelöschte alte Dateien bleiben im Manifest, damit sie später noch aufgeräumt werden können
        let keep: Vec<String> = old
            .files
            .difference(&man.files)
            .filter(|r| dest.exists(r))
            .cloned()
            .collect();
        man.files.extend(keep);
    }
    if let Err(e) = dest.finish(&p.lists) {
        rep.errors.push(format!("Playlisten aufs Handy: {e}"));
    }
    if device {
        save_db_manifest(&conn.lock().unwrap(), id, &man)?;
        #[cfg(windows)]
        mtp::forget_free();
    } else {
        std::fs::write(root.join(MANIFEST), serde_json::to_vec_pretty(&man)?)?;
    }
    {
        let c = conn.lock().unwrap();
        let mut t2 = get(&c, id)?;
        t2.last_sync = now();
        save(&c, &t2)?;
    }
    progress(total, total, "Fertig");
    Ok(rep)
}

/// Während eines Handy-Syncs das Gerät nicht zusätzlich für die Anzeige öffnen
#[cfg(windows)]
struct Busy(String);
#[cfg(windows)]
impl Busy {
    fn new(dev: &str) -> Busy {
        mtp::BUSY
            .lock()
            .unwrap()
            .get_or_insert_with(Default::default)
            .insert(dev.to_string());
        Busy(dev.to_string())
    }
}
#[cfg(windows)]
impl Drop for Busy {
    fn drop(&mut self) {
        if let Some(b) = mtp::BUSY.lock().unwrap().as_mut() {
            b.remove(&self.0);
        }
    }
}

/// „Ordner/Lied.mp3“ → „Ordner/Lied.lrc“
fn lrc_rel(rel: &str) -> String {
    match rel.rsplit_once('.') {
        Some((stem, _)) if !stem.ends_with('/') => format!("{stem}.lrc"),
        _ => format!("{rel}.lrc"),
    }
}

/// Songtext (.lrc) neben dem Titel mitschicken, wenn er fehlt oder sich geändert hat
fn put_lyrics(dest: &mut dyn Dest, src: &Path, rel: &str) {
    if let Ok(b) = std::fs::read(crate::lyrics::sidecar(src)) {
        let r = lrc_rel(rel);
        if !dest.has(&r, b.len() as u64) {
            let _ = dest.write(&r, &b);
        }
    }
}

/// Laufwerk sicher entfernen
pub fn eject(path: &str) -> Result<()> {
    let p = Path::new(path);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let drive: String = p.to_string_lossy().chars().take(2).collect(); // "E:"
        if !drive.ends_with(':') {
            return Err(anyhow!("Kein Laufwerk erkannt"));
        }
        let ps = format!(
            "(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{drive}').InvokeVerb('Eject')"
        );
        std::process::Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command", &ps])
            .creation_flags(0x0800_0000)
            .status()?;
        // Windows meldet Erfolg nicht zurück → kurz warten und prüfen
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(250));
            if !p.exists() {
                return Ok(());
            }
        }
        Err(anyhow!(
            "Konnte nicht ausgeworfen werden – wird das Laufwerk noch benutzt?"
        ))
    }
    #[cfg(target_os = "macos")]
    {
        // /Volumes/NAME/… → /Volumes/NAME
        let mut vol = PathBuf::new();
        for (i, c) in p.components().enumerate() {
            vol.push(c);
            if i >= 2 {
                break;
            }
        }
        let ok = std::process::Command::new("diskutil")
            .arg("eject")
            .arg(&vol)
            .status()?
            .success();
        if ok {
            Ok(())
        } else {
            Err(anyhow!(
                "Konnte nicht ausgeworfen werden – wird das Laufwerk noch benutzt?"
            ))
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let ok = std::process::Command::new("gio")
            .args(["mount", "-e"])
            .arg(p)
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            Ok(())
        } else {
            Err(anyhow!("Konnte nicht ausgeworfen werden"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sync_copies_and_cleans() {
        let base = std::env::temp_dir().join(format!("mlsync{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("lib")).unwrap();
        std::fs::create_dir_all(base.join("stick")).unwrap();
        let conn = Mutex::new(db::open(&base.join("t.db")).unwrap());
        let mk = |name: &str, title: &str| {
            let f = base.join("lib").join(name);
            std::fs::write(&f, vec![1u8; 1000]).unwrap();
            let c = conn.lock().unwrap();
            c.execute("INSERT INTO tracks(path, size, title, artist, album) VALUES(?1, 1000, ?2, 'Band', 'Platte')", params![f.to_string_lossy(), title]).unwrap();
            c.last_insert_rowid()
        };
        let a = mk("a.mp3", "Eins");
        let b = mk("b.mp3", "Zwei");
        let pid = {
            let mut c = conn.lock().unwrap();
            let p = playlists::create(&c, "Auto", None).unwrap();
            playlists::set_items(&mut c, p, &[a, b]).unwrap();
            p
        };
        let tid = save(
            &conn.lock().unwrap(),
            &Target {
                name: "Stick".into(),
                path: base.join("stick").to_string_lossy().into(),
                playlists: vec![pid],
                mirror: true,
                ..Default::default()
            },
        )
        .unwrap();
        let cancel = AtomicBool::new(false);
        let r = run(&conn, tid, &cancel, &|_, _, _| {}).unwrap();
        assert_eq!(r.copied, 2);
        assert!(base.join("stick/Musik/Band/Platte/Eins.mp3").exists());
        let m3u = std::fs::read_to_string(base.join("stick/Auto.m3u8")).unwrap();
        assert!(m3u.contains("Musik/Band/Platte/Zwei.mp3"), "{m3u}");
        // zweiter Lauf: nichts zu tun
        assert_eq!(run(&conn, tid, &cancel, &|_, _, _| {}).unwrap().skipped, 2);
        // Titel aus Playlist entfernen → wird vom Stick gelöscht, fremde Datei bleibt
        std::fs::write(base.join("stick/fremd.txt"), "x").unwrap();
        playlists::set_items(&mut conn.lock().unwrap(), pid, &[a]).unwrap();
        assert_eq!(run(&conn, tid, &cancel, &|_, _, _| {}).unwrap().removed, 1);
        assert!(!base.join("stick/Musik/Band/Platte/Zwei.mp3").exists());
        assert!(base.join("stick/fremd.txt").exists());
        // Ablage „je Playlist“: Ordner mit Nummern, Playlist-Datei darin
        let mut t = get(&conn.lock().unwrap(), tid).unwrap();
        t.layout = "numbered".into();
        save(&conn.lock().unwrap(), &t).unwrap();
        run(&conn, tid, &cancel, &|_, _, _| {}).unwrap();
        assert!(base.join("stick/Auto/01 - Band - Eins.mp3").exists());
        let m3u = std::fs::read_to_string(base.join("stick/Auto/Auto.m3u8")).unwrap();
        assert!(m3u.contains("\n01 - Band - Eins.mp3"), "{m3u}");
        assert!(!base.join("stick/Auto.m3u8").exists());
        assert!(!base.join("stick/Musik/Band/Platte/Eins.mp3").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn folder_source_keeps_structure() {
        let base = std::env::temp_dir().join(format!("mlsyncdir{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("quelle/Sets/2026")).unwrap();
        std::fs::create_dir_all(base.join("stick")).unwrap();
        std::fs::write(base.join("quelle/Sets/2026/a.mp3"), b"aaaa").unwrap();
        std::fs::write(base.join("quelle/b.mp3"), b"bb").unwrap();
        let conn = Mutex::new(db::open(&base.join("t.db")).unwrap());
        for (p, sz) in [("quelle/Sets/2026/a.mp3", 4), ("quelle/b.mp3", 2)] {
            conn.lock()
                .unwrap()
                .execute(
                    "INSERT INTO tracks(path, size) VALUES(?1, ?2)",
                    params![base.join(p).to_string_lossy(), sz],
                )
                .unwrap();
        }
        let t = Target {
            name: "S".into(),
            path: base.join("stick").to_string_lossy().into(),
            folders: vec![base.join("quelle").to_string_lossy().into()],
            mirror: true,
            ..Default::default()
        };
        let id = save(&conn.lock().unwrap(), &t).unwrap();
        run(&conn, id, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
        assert!(base.join("stick/quelle/Sets/2026/a.mp3").exists());
        assert!(base.join("stick/quelle/b.mp3").exists());
        let _ = std::fs::remove_dir_all(&base);
    }
}

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
    c.execute_batch("CREATE TABLE IF NOT EXISTS sync_targets (id INTEGER PRIMARY KEY, data TEXT NOT NULL);")?;
    Ok(())
}

pub fn list(c: &Connection) -> Result<Vec<TargetState>> {
    ensure(c)?;
    let mut st = c.prepare("SELECT id, data FROM sync_targets ORDER BY id")?;
    let rows: Vec<(i64, String)> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, data)| {
            let mut t: Target = serde_json::from_str(&data).ok()?;
            t.id = id;
            let p = Path::new(&t.path);
            let available = !t.path.is_empty() && p.is_dir();
            Some(TargetState { free: if available { fs4::available_space(p).ok() } else { None }, available, target: t })
        })
        .collect())
}

pub fn get(c: &Connection, id: i64) -> Result<Target> {
    list(c)?.into_iter().find(|t| t.target.id == id).map(|t| t.target).ok_or_else(|| anyhow!("Sync-Ziel nicht gefunden"))
}

pub fn save(c: &Connection, t: &Target) -> Result<i64> {
    ensure(c)?;
    let data = serde_json::to_string(t)?;
    if t.id > 0 {
        c.execute("UPDATE sync_targets SET data = ?2 WHERE id = ?1", params![t.id, data])?;
        Ok(t.id)
    } else {
        c.execute("INSERT INTO sync_targets(data) VALUES(?1)", [data])?;
        Ok(c.last_insert_rowid())
    }
}

pub fn delete(c: &Connection, id: i64) -> Result<()> {
    ensure(c)?;
    c.execute("DELETE FROM sync_targets WHERE id = ?1", [id])?;
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
        "numbered" => format!("{}/{}", paths::sanitize(playlist), paths::sanitize(&format!("{:0digits$} - {}", pos, crate::export::label(t)))),
        "flat" => format!("{}/{}", paths::sanitize(playlist), paths::sanitize(&crate::export::label(t))),
        _ => format!("{MUSIC_DIR}/{}", organize::render("{albumartist}/{album}/{disc-}{track} - {title}", t).to_string_lossy()),
    };
    stem.replace('\\', "/")
}

/// Ein Ordner je Playlist (nummeriert oder nicht)?
fn per_playlist(layout: &str) -> bool {
    matches!(layout, "numbered" | "flat")
}

fn ext_of(t: &Track) -> String {
    Path::new(&t.path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default()
}

struct Plan {
    /// (Quelle, relativer Zielpfad, Größe)
    files: Vec<(PathBuf, String, u64)>,
    /// (relativer Pfad der .m3u8, Inhalt)
    playlists: Vec<(String, String)>,
}

fn plan(c: &Connection, t: &Target) -> Result<Plan> {
    let layout = if t.layout.is_empty() { "structured" } else { t.layout.as_str() };
    let mut files: Vec<(PathBuf, String, u64)> = Vec::new();
    // Titel → Zielpfad (bei „je Playlist“ pro Playlist eigene Kopie)
    let mut by_key: HashMap<(i64, i64), String> = HashMap::new();
    let mut used: HashMap<String, usize> = HashMap::new();
    let mut pls = Vec::new();
    let root = Path::new(&t.path);
    for &pid in &t.playlists {
        let Ok(name) = playlists::name(c, pid) else { continue };
        let tracks = db::tracks_by_ids(c, &playlists::track_ids(c, pid)?)?;
        let digits = tracks.len().to_string().len().max(2);
        let mut on_device: Vec<Track> = Vec::new();
        for (i, tr) in tracks.into_iter().enumerate() {
            let key = (if per_playlist(layout) { pid } else { 0 }, tr.id);
            let rel = by_key.entry(key).or_insert_with(|| {
                let stem = rel_of(&tr, layout, &name, i + 1, digits);
                let ext = ext_of(&tr);
                // gleiche Zielnamen (zwei Titel mit gleichem Namen) auseinanderhalten
                let n = used.entry(format!("{stem}.{ext}").to_lowercase()).or_insert(0);
                *n += 1;
                let r = if *n > 1 { format!("{stem} ({n}).{ext}") } else { format!("{stem}.{ext}") };
                files.push((PathBuf::from(&tr.path), r.clone(), tr.size.max(0) as u64));
                r
            });
            let mut copy = tr.clone();
            copy.path = root.join(&*rel).to_string_lossy().to_string();
            on_device.push(copy);
        }
        if t.no_playlist_files {
            continue;
        }
        let file = format!("{}.m3u8", paths::sanitize(&name));
        // bei „Ordner je Playlist“ liegt die Playlist-Datei im Ordner der Playlist
        let (m3u_rel, base) = if per_playlist(layout) {
            (format!("{}/{file}", paths::sanitize(&name)), root.join(paths::sanitize(&name)))
        } else {
            (file, root.to_path_buf())
        };
        pls.push((m3u_rel, playlists::m3u8(&on_device, &base)));
    }
    Ok(Plan { files, playlists: pls })
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
    std::fs::read(root.join(MANIFEST)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn same_file(src: &Path, dst: &Path, size: u64) -> bool {
    let _ = src;
    std::fs::metadata(dst).map(|m| m.len() == size).unwrap_or(false)
}

/// Was würde passieren? (für die Anzeige „bereit: 12 neue Titel“)
pub fn preview(c: &Connection, t: &Target) -> Result<Preview> {
    let root = Path::new(&t.path);
    let p = plan(c, t)?;
    let mut pv = Preview::default();
    let wanted: BTreeSet<String> = p.files.iter().map(|f| f.1.clone()).collect();
    for (src, rel, size) in &p.files {
        pv.total_bytes += size;
        if root.is_dir() && same_file(src, &root.join(rel), *size) {
            pv.up_to_date += 1;
        } else {
            pv.to_copy += 1;
            pv.copy_bytes += size;
        }
    }
    if t.mirror && root.is_dir() {
        pv.to_remove = read_manifest(root).files.iter().filter(|f| !wanted.contains(*f)).count();
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

pub fn run(conn: &Mutex<Connection>, id: i64, cancel: &AtomicBool, progress: Progress) -> Result<Report> {
    let (t, p) = {
        let c = conn.lock().unwrap();
        let t = get(&c, id)?;
        let p = plan(&c, &t)?;
        (t, p)
    };
    let root = PathBuf::from(&t.path);
    if !root.is_dir() {
        return Err(anyhow!("„{}“ ist nicht verbunden", t.name));
    }
    let needed: u64 = p.files.iter().filter(|(s, r, z)| !same_file(s, &root.join(r), *z)).map(|f| f.2).sum();
    if let Ok(free) = fs4::available_space(&root) {
        if needed > free + 1024 * 1024 {
            return Err(anyhow!(
                "Nicht genug Platz auf „{}“: {:.1} GB nötig, {:.1} GB frei",
                t.name,
                needed as f64 / 1e9,
                free as f64 / 1e9
            ));
        }
    }
    let mut rep = Report::default();
    let old = read_manifest(&root);
    let mut man = Manifest::default();
    let total = p.files.len();
    for (i, (src, rel, size)) in p.files.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            rep.errors.push("Abgebrochen".into());
            break;
        }
        progress(i, total, rel.rsplit('/').next().unwrap_or(rel));
        let dst = root.join(rel);
        man.files.insert(rel.clone());
        if same_file(src, &dst, *size) {
            rep.skipped += 1;
            continue;
        }
        let res = (|| -> std::io::Result<()> {
            if let Some(d) = dst.parent() {
                std::fs::create_dir_all(d)?;
            }
            // erst als .part schreiben → nie halbe Dateien auf dem Gerät
            let tmp = dst.with_extension("part");
            std::fs::copy(src, &tmp)?;
            std::fs::rename(&tmp, &dst)
        })();
        match res {
            Ok(()) => {
                rep.copied += 1;
                rep.bytes += size;
            }
            Err(e) => {
                man.files.remove(rel);
                rep.errors.push(format!("{}: {e}", src.display()));
            }
        }
    }
    // Playlisten schreiben
    for (rel, content) in &p.playlists {
        let f = root.join(rel);
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(&f, content).is_ok() {
            man.playlists.insert(rel.clone());
        }
    }
    // Nicht mehr gewählte Playlisten der App entfernen (immer) – Musikdateien nur bei „aufräumen“
    for rel in old.playlists.difference(&man.playlists) {
        let _ = std::fs::remove_file(root.join(rel));
    }
    if t.mirror && !cancel.load(Ordering::Relaxed) {
        for rel in old.files.difference(&man.files) {
            let f = root.join(rel);
            if std::fs::remove_file(&f).is_ok() {
                rep.removed += 1;
                if let Some(d) = f.parent() {
                    paths::remove_empty_dirs(d.to_path_buf(), &[root.clone(), root.join(MUSIC_DIR).parent().unwrap_or(&root).to_path_buf()]);
                }
            }
        }
    } else {
        // nicht gelöschte alte Dateien bleiben im Manifest, damit sie später noch aufgeräumt werden können
        man.files.extend(old.files.difference(&man.files.clone()).filter(|r| root.join(r).exists()).cloned());
    }
    std::fs::write(root.join(MANIFEST), serde_json::to_vec_pretty(&man)?)?;
    {
        let c = conn.lock().unwrap();
        let mut t2 = get(&c, id)?;
        t2.last_sync = now();
        save(&c, &t2)?;
    }
    progress(total, total, "Fertig");
    Ok(rep)
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
        let ps = format!("(New-Object -ComObject Shell.Application).Namespace(17).ParseName('{drive}').InvokeVerb('Eject')");
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
        Err(anyhow!("Konnte nicht ausgeworfen werden – wird das Laufwerk noch benutzt?"))
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
        let ok = std::process::Command::new("diskutil").arg("eject").arg(&vol).status()?.success();
        if ok { Ok(()) } else { Err(anyhow!("Konnte nicht ausgeworfen werden – wird das Laufwerk noch benutzt?")) }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let ok = std::process::Command::new("gio").args(["mount", "-e"]).arg(p).status().map(|s| s.success()).unwrap_or(false);
        if ok { Ok(()) } else { Err(anyhow!("Konnte nicht ausgeworfen werden")) }
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
        let tid = save(&conn.lock().unwrap(), &Target { name: "Stick".into(), path: base.join("stick").to_string_lossy().into(), playlists: vec![pid], mirror: true, ..Default::default() }).unwrap();
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
}

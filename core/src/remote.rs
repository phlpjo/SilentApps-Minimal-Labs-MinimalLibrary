//! Bibliothek fürs Handy (Minimal Player Connected): solange Minimal Library läuft, sieht die App
//! alle Ordner und Playlisten, spielt Titel direkt ab (Streaming im Heimnetz), bearbeitet Playlisten
//! Zugriff nur nach einmaligem Koppeln mit dem Code aus den Einstellungen.
//!
//! - Finden: UDP-Broadcast „MLPCDISCOVER1“ an Port 47831 → Antwort mit Name, Kennung und Port
//! - HTTP (Port 47830, sonst frei gewählt): /pair, /state, /lib, /a/<id>, /c/<id>, /pl…

use crate::db;
use crate::{playlists, tags};
use anyhow::{Result, anyhow};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

pub const TCP_PORT: u16 = 47830;
pub const UDP_PORT: u16 = 47831;

/// Was der Server von der App braucht
pub struct Host {
    pub db: Arc<Mutex<Connection>>,
    /// Musikordner, Schema und Ziel fürs Sortieren
    pub settings: Box<dyn Fn() -> HostSettings + Send + Sync>,
    /// Etwas wurde vom Handy aus geändert → Oberfläche neu laden
    pub changed: Box<dyn Fn(Changed) + Send + Sync>,
}

pub struct HostSettings {
    pub roots: Vec<String>,
    pub pattern: String,
    pub organize_target: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Changed {
    Playlists,
    Files(usize),
    /// Vom Handy hochgeladene Titel (neu in der Bibliothek)
    Added(i64),
}

static CODE: Mutex<String> = Mutex::new(String::new());
static FAILS: Mutex<u32> = Mutex::new(0);
static PORT: AtomicU16 = AtomicU16::new(0);
static STARTED: AtomicBool = AtomicBool::new(false);

fn rnd(n: usize) -> Vec<u8> {
    use std::hash::{BuildHasher, Hasher};
    let mut out = Vec::with_capacity(n);
    while out.len() < n {
        let mut h = std::collections::hash_map::RandomState::new().build_hasher();
        h.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        );
        h.write_usize(out.len());
        out.extend_from_slice(&h.finish().to_le_bytes());
    }
    out.truncate(n);
    out
}

fn hex(n: usize) -> String {
    rnd(n).iter().map(|b| format!("{b:02x}")).collect()
}

fn new_code() -> String {
    let v = u32::from_le_bytes(rnd(4).try_into().unwrap_or([0; 4]));
    format!("{:06}", 100_000 + v % 900_000)
}

/// Aktueller Kopplungscode (für die Einstellungen)
pub fn code() -> String {
    let mut c = CODE.lock().unwrap();
    if c.is_empty() {
        *c = new_code();
    }
    c.clone()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paired {
    pub token: String,
    pub name: String,
    pub added: i64,
}

fn paired(c: &Connection) -> Vec<Paired> {
    db::get_setting(c, "remote_tokens")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_paired(c: &Connection, p: &[Paired]) -> Result<()> {
    db::set_setting(c, "remote_tokens", &serde_json::to_string(p)?)
}

/// Gekoppelte Handys (Namen)
pub fn paired_names(c: &Connection) -> Vec<String> {
    paired(c).into_iter().map(|p| p.name).collect()
}

pub fn unpair(c: &Connection, name: &str) -> Result<()> {
    let mut p = paired(c);
    p.retain(|x| x.name != name);
    save_paired(c, &p)
}

pub fn port() -> u16 {
    PORT.load(Ordering::Relaxed)
}

fn machine_id(c: &Connection) -> String {
    if let Some(id) = db::get_setting(c, "remote_id").ok().flatten() {
        return id;
    }
    let id = hex(8);
    let _ = db::set_setting(c, "remote_id", &id);
    id
}

/// Server starten (einmal pro Programmlauf; ob er antwortet, entscheidet die Einstellung)
pub fn start(host: Host) {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let host = Arc::new(host);
    let http = tiny_http::Server::http(("0.0.0.0", TCP_PORT))
        .or_else(|_| tiny_http::Server::http("0.0.0.0:0"));
    let Ok(http) = http else { return };
    let port = http.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    PORT.store(port, Ordering::Relaxed);
    let http = Arc::new(http);
    {
        let host = host.clone();
        std::thread::spawn(move || {
            for req in http.incoming_requests() {
                let host = host.clone();
                std::thread::spawn(move || {
                    let _ = handle(&host, req);
                });
            }
        });
    }
    // Finden im WLAN
    std::thread::spawn(move || {
        let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, UDP_PORT)) else {
            return;
        };
        let mut buf = [0u8; 256];
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf) else {
                continue;
            };
            if !buf[..n].starts_with(b"MLPCDISCOVER1") || !(host.settings)().enabled {
                continue;
            }
            let id = machine_id(&host.db.lock().unwrap());
            let reply = serde_json::json!({ "id": id, "name": computer_name(), "port": port, "app": "Minimal Library" });
            let _ = sock.send_to(reply.to_string().as_bytes(), from);
        }
    });
}

fn header(n: &str, v: &str) -> tiny_http::Header {
    tiny_http::Header::from_bytes(n.as_bytes(), v.as_bytes()).unwrap()
}

fn text(req: tiny_http::Request, status: u16, body: String, json: bool) -> std::io::Result<()> {
    let ct = if json {
        "application/json; charset=utf-8"
    } else {
        "text/plain; charset=utf-8"
    };
    req.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(header("Content-Type", ct)),
    )
}

fn handle(host: &Host, mut req: tiny_http::Request) -> std::io::Result<()> {
    let s = (host.settings)();
    if !s.enabled {
        return text(
            req,
            503,
            "Freigabe in Minimal Library ausgeschaltet".into(),
            false,
        );
    }
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let path = path.to_string();
    let method = req.method().clone();
    let mut body = String::new();
    let upload = method == tiny_http::Method::Put && path.starts_with("/up/");
    if !upload && matches!(method, tiny_http::Method::Post | tiny_http::Method::Put) {
        let _ = req
            .as_reader()
            .take(16 * 1024 * 1024)
            .read_to_string(&mut body);
    }
    if path == "/pair" && method == tiny_http::Method::Post {
        return pair(host, req, &body);
    }
    // Schlüssel: Kopfzeile oder ?t= (für den Player, der keine Kopfzeilen setzen kann)
    let token = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("X-Token"))
        .map(|h| h.value.as_str().to_string())
        .or_else(|| {
            query
                .split('&')
                .find_map(|kv| kv.strip_prefix("t=").map(str::to_string))
        })
        .unwrap_or_default();
    let Some(client) = paired(&host.db.lock().unwrap())
        .into_iter()
        .find(|p| !token.is_empty() && p.token == token)
        .map(|p| p.name)
    else {
        return text(req, 401, "nicht gekoppelt".into(), false);
    };
    if upload {
        return match receive_upload(host, &s, &client, &path, &mut req) {
            Ok(id) => text(req, 200, serde_json::json!({ "id": id }).to_string(), true),
            Err(e) => text(req, 400, e.to_string(), false),
        };
    }
    let id_of = |p: &str| {
        p.rsplit('/')
            .next()
            .and_then(|x| x.split('.').next())
            .and_then(|x| x.parse::<i64>().ok())
    };
    let result: Result<Option<(u16, String)>> = (|| {
        Ok(match (&method, path.as_str()) {
            (tiny_http::Method::Get, "/state") => {
                Some((200, state(&host.db.lock().unwrap(), &s.roots)?.to_string()))
            }
            (tiny_http::Method::Get, "/lib") => {
                Some((200, library(&host.db.lock().unwrap(), &s.roots)?))
            }
            (tiny_http::Method::Get | tiny_http::Method::Head, p) if p.starts_with("/a/") => None,
            (tiny_http::Method::Get, p) if p.starts_with("/c/") || p.starts_with("/l/") => None,
            (tiny_http::Method::Post, "/have") => Some((
                200,
                have(&host.db.lock().unwrap(), &s.roots, &body)?.to_string(),
            )),
            (tiny_http::Method::Post, "/plsync") => {
                plsync(&host.db, &body)?;
                (host.changed)(Changed::Playlists);
                Some((200, "{}".into()))
            }
            (tiny_http::Method::Post, "/pl") => {
                let v: serde_json::Value = serde_json::from_str(&body)?;
                let name = v["name"]
                    .as_str()
                    .unwrap_or("Neue Playlist")
                    .trim()
                    .to_string();
                let id = {
                    let mut c = host.db.lock().unwrap();
                    let id = playlists::create(&c, &name, None)?;
                    if let Some(items) = v["items"].as_array() {
                        let ids: Vec<i64> = items.iter().filter_map(|x| x.as_i64()).collect();
                        playlists::set_items(&mut c, id, &ids)?;
                    }
                    id
                };
                (host.changed)(Changed::Playlists);
                Some((200, serde_json::json!({ "id": id }).to_string()))
            }
            (tiny_http::Method::Post, p) if p.starts_with("/pl/") => {
                let id = id_of(p).ok_or_else(|| anyhow!("Playlist?"))?;
                let v: serde_json::Value = serde_json::from_str(&body)?;
                {
                    let mut c = host.db.lock().unwrap();
                    let smart = playlists::list(&c)?
                        .into_iter()
                        .find(|x| x.id == id)
                        .ok_or_else(|| anyhow!("Playlist gibt es nicht mehr"))?
                        .rules;
                    if let Some(name) = v["name"].as_str().filter(|n| !n.trim().is_empty()) {
                        playlists::update(&c, id, name.trim(), smart.as_ref())?;
                    }
                    if let Some(items) = v["items"].as_array() {
                        if smart.is_some() {
                            return Err(anyhow!(
                                "Smarte Playlisten lassen sich nur in Minimal Library ändern"
                            ));
                        }
                        let ids: Vec<i64> = items.iter().filter_map(|x| x.as_i64()).collect();
                        playlists::set_items(&mut c, id, &ids)?;
                    }
                }
                (host.changed)(Changed::Playlists);
                Some((200, "{}".into()))
            }
            (tiny_http::Method::Delete, p) if p.starts_with("/pl/") => {
                let id = id_of(p).ok_or_else(|| anyhow!("Playlist?"))?;
                playlists::delete(&host.db.lock().unwrap(), id)?;
                (host.changed)(Changed::Playlists);
                Some((200, "{}".into()))
            }
            _ => Some((404, String::new())),
        })
    })();
    match result {
        Ok(Some((st, b))) => text(req, st, b, st == 200),
        Ok(None) => {
            let Some(id) = id_of(&path) else {
                return text(req, 404, String::new(), false);
            };
            let t = db::tracks_by_ids(&host.db.lock().unwrap(), &[id])
                .ok()
                .and_then(|v| v.into_iter().next());
            let Some(t) = t.filter(|t| crate::dirs::under_any(&t.path, &roots_of(&s.roots))) else {
                return text(req, 404, String::new(), false);
            };
            if path.starts_with("/l/") {
                return match crate::lyrics::read(Path::new(&t.path)) {
                    Some(l) => text(req, 200, l.text, false),
                    None => text(req, 404, String::new(), false),
                };
            }
            if path.starts_with("/c/") {
                match tags::cover(Path::new(&t.path)).ok().flatten() {
                    Some((mime, data)) => req.respond(
                        tiny_http::Response::from_data(data)
                            .with_header(header("Content-Type", &mime))
                            .with_header(header("Cache-Control", "max-age=86400")),
                    ),
                    None => text(req, 404, String::new(), false),
                }
            } else {
                serve_file(req, Path::new(&t.path))
            }
        }
        Err(e) => text(req, 400, e.to_string(), false),
    }
}

fn pair(host: &Host, req: tiny_http::Request, body: &str) -> std::io::Result<()> {
    let given: String = body.chars().filter(|c| c.is_ascii_digit()).collect();
    if given != code() {
        let mut f = FAILS.lock().unwrap();
        *f += 1;
        if *f >= 5 {
            *f = 0;
            *CODE.lock().unwrap() = new_code();
        }
        return text(req, 403, "Code falsch".into(), false);
    }
    let name = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("X-Client"))
        .map(|h| h.value.as_str().chars().take(60).collect::<String>())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "Handy".into());
    let token = hex(24);
    {
        let c = host.db.lock().unwrap();
        let mut p = paired(&c);
        p.retain(|x| x.name != name); // gleiches Handy neu gekoppelt
        p.push(Paired {
            token: token.clone(),
            name,
            added: crate::now(),
        });
        let _ = save_paired(&c, &p);
    }
    *CODE.lock().unwrap() = new_code();
    let id = machine_id(&host.db.lock().unwrap());
    text(
        req,
        200,
        serde_json::json!({ "token": token, "id": id, "name": computer_name() }).to_string(),
        true,
    )
}

fn roots_of(roots: &[String]) -> Vec<PathBuf> {
    roots.iter().map(PathBuf::from).collect()
}

/// Kurzer Fingerabdruck: ändert sich, sobald Titel, Tags oder Playlisten sich ändern
fn state(c: &Connection, roots: &[String]) -> Result<serde_json::Value> {
    let (n, max, mt, len): (i64, i64, i64, i64) = c.query_row(
        "SELECT COUNT(*), COALESCE(MAX(id),0), COALESCE(SUM(mtime),0) % 1000000007, COALESCE(SUM(LENGTH(path) + LENGTH(COALESCE(title,'')) + LENGTH(COALESCE(artist,''))),0) FROM tracks",
        [],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    let (pn, ps): (i64, i64) = c.query_row(
        "SELECT COUNT(*), COALESCE(SUM(track_id * (pos + 1)), 0) % 1000000007 FROM playlist_items",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let names: String = c.query_row("SELECT COALESCE(group_concat(id || ':' || name || ':' || COALESCE(rules,''), '|'), '') FROM playlists", [], |r| r.get(0))?;
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (n, max, mt, len, pn, ps, names, roots).hash(&mut h);
    Ok(serde_json::json!({ "version": format!("{:x}", h.finish()), "tracks": n }))
}

/// Ganze Bibliothek kompakt:
/// {"name", "version", "roots": ["Musik", …], "tracks": [[id, root, "Ordner/Unterordner", "Datei.mp3", Titel, Künstler, Album, ms, Größe, Cover], …],
///  "playlists": [{"id", "name", "smart", "items": [ids]}]}
fn library(c: &Connection, roots: &[String]) -> Result<String> {
    let all = db::all_tracks(c)?;
    let norm: Vec<String> = roots
        .iter()
        .map(|r| r.trim_end_matches(['/', '\\']).to_string())
        .collect();
    let mut tracks = Vec::with_capacity(all.len());
    for t in &all {
        let Some((ri, r)) = norm
            .iter()
            .enumerate()
            .filter(|(_, r)| crate::dirs::under_any(&t.path, &[PathBuf::from(r.as_str())]))
            .max_by_key(|(_, r)| r.len())
        else {
            continue;
        };
        let rel = t.path.get(r.len() + 1..).unwrap_or("").replace('\\', "/");
        let (dir, file) = rel.rsplit_once('/').unwrap_or(("", &rel));
        tracks.push(serde_json::json!([
            t.id,
            ri,
            dir,
            file,
            t.title.clone().unwrap_or_default(),
            t.artist.clone().unwrap_or_default(),
            t.album.clone().unwrap_or_default(),
            (t.duration * 1000.0).round() as i64,
            t.size,
            t.has_cover
        ]));
    }
    let known: std::collections::HashSet<i64> =
        tracks.iter().filter_map(|v| v[0].as_i64()).collect();
    let mut pls = Vec::new();
    for p in playlists::list(c)? {
        let items: Vec<i64> = playlists::track_ids(c, p.id)?
            .into_iter()
            .filter(|i| known.contains(i))
            .collect();
        pls.push(serde_json::json!({ "id": p.id, "name": p.name, "smart": p.rules.is_some(), "items": items }));
    }
    let names: Vec<String> = norm
        .iter()
        .map(|r| {
            Path::new(r)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| r.clone())
        })
        .collect();
    let v = state(c, roots)?;
    Ok(serde_json::json!({
        "name": computer_name(),
        "version": v["version"],
        "roots": names,
        "tracks": tracks,
        "playlists": pls
    })
    .to_string())
}

/// Name dieses Computers (zeigt das Handy an)
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "Computer".into())
}

fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Welche Titel vom Handy gibt es am PC schon? Gleich = gleicher Dateiname und Größe
/// oder gleicher Interpret + Titel (Länge ±3 s). → {"found": {key: id}, "missing": [key]}
fn have(c: &Connection, roots: &[String], body: &str) -> Result<serde_json::Value> {
    let items: Vec<serde_json::Value> = serde_json::from_str(body)?;
    let rp = roots_of(roots);
    let all: Vec<db::Track> = db::all_tracks(c)?
        .into_iter()
        .filter(|t| crate::dirs::under_any(&t.path, &rp))
        .collect();
    let mut by_file = std::collections::HashMap::new();
    let mut by_tag: std::collections::HashMap<String, Vec<(i64, f64)>> =
        std::collections::HashMap::new();
    for t in &all {
        let name = Path::new(&t.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        by_file.insert((name, t.size), t.id);
        if let (Some(a), Some(ti)) = (&t.artist, &t.title) {
            by_tag
                .entry(format!("{}|{}", norm(a), norm(ti)))
                .or_default()
                .push((t.id, t.duration));
        }
    }
    let mut found = serde_json::Map::new();
    let mut missing = Vec::new();
    for it in items {
        let k = it["k"].as_str().unwrap_or_default().to_string();
        let name = it["name"].as_str().unwrap_or_default().to_lowercase();
        let size = it["size"].as_i64().unwrap_or(-1);
        let ms = it["ms"].as_i64().unwrap_or(0) as f64;
        let mut hit = by_file.get(&(name, size)).copied();
        if hit.is_none() {
            let (a, t) = (
                norm(it["artist"].as_str().unwrap_or_default()),
                norm(it["title"].as_str().unwrap_or_default()),
            );
            if !a.is_empty() && !t.is_empty() {
                hit = by_tag.get(&format!("{a}|{t}")).and_then(|v| {
                    v.iter()
                        .find(|(_, d)| ms <= 0.0 || (d * 1000.0 - ms).abs() < 3000.0)
                        .map(|x| x.0)
                });
            }
        }
        match hit {
            Some(id) => {
                found.insert(k, id.into());
            }
            None => missing.push(k),
        }
    }
    Ok(serde_json::json!({ "found": found, "missing": missing }))
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

fn clean(part: &str) -> String {
    let c: String = part
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    c.trim().trim_end_matches('.').to_string()
}

/// Titel vom Handy speichern: <erster Musikordner>/Vom Handy/<Handy>/<Ordner>/<Datei>
fn receive_upload(
    host: &Host,
    s: &HostSettings,
    client: &str,
    path: &str,
    req: &mut tiny_http::Request,
) -> Result<i64> {
    let root = s
        .roots
        .first()
        .ok_or_else(|| anyhow!("In Minimal Library ist kein Musikordner eingerichtet"))?;
    let rel: Vec<String> = decode(path.trim_start_matches("/up/"))
        .split('/')
        .map(clean)
        .filter(|p| !p.is_empty() && p != "..")
        .collect();
    if rel.is_empty() {
        return Err(anyhow!("Dateiname fehlt"));
    }
    let mut dest = PathBuf::from(root).join("Vom Handy").join(clean(client));
    for p in &rel {
        dest.push(p);
    }
    if !crate::is_audio(&dest) {
        return Err(anyhow!("Keine Audiodatei"));
    }
    std::fs::create_dir_all(dest.parent().unwrap_or(Path::new(root)))?;
    let tmp = dest.with_file_name(format!(
        "{}.part",
        dest.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default()
    ));
    {
        let mut f = std::fs::File::create(&tmp)?;
        std::io::copy(req.as_reader(), &mut f)?;
    }
    if dest.exists() {
        let _ = std::fs::remove_file(&tmp);
    } else {
        std::fs::rename(&tmp, &dest)?;
    }
    let ids = crate::scanner::add_files(&host.db, std::slice::from_ref(&dest))?;
    let id = ids.first().copied().or_else(|| {
        host.db
            .lock()
            .unwrap()
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                [dest.to_string_lossy()],
                |r| r.get(0),
            )
            .ok()
    });
    let id = id.ok_or_else(|| anyhow!("Datei nicht lesbar"))?;
    (host.changed)(Changed::Added(id));
    Ok(id)
}

/// Playlisten vom Handy: {"prefix", "playlists": [{"uid", "name", "items": [ids]}]} – Zuordnung Handy→PC gemerkt
fn plsync(db_: &Mutex<Connection>, body: &str) -> Result<()> {
    let v: serde_json::Value = serde_json::from_str(body)?;
    let prefix = v["prefix"].as_str().unwrap_or_default();
    if prefix.is_empty() {
        return Err(anyhow!("prefix fehlt"));
    }
    let mut c = db_.lock().unwrap();
    let mut map: std::collections::HashMap<String, i64> = db::get_setting(&c, "remote_plmap")
        .ok()
        .flatten()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    let existing: std::collections::HashSet<i64> =
        playlists::list(&c)?.into_iter().map(|p| p.id).collect();
    let mut seen = std::collections::HashSet::new();
    for p in v["playlists"].as_array().cloned().unwrap_or_default() {
        let uid = p["uid"].as_str().unwrap_or_default().to_string();
        if !uid.starts_with(prefix) {
            continue;
        }
        let name = p["name"].as_str().unwrap_or("Playlist").trim().to_string();
        let ids: Vec<i64> = p["items"]
            .as_array()
            .map(|a| a.iter().filter_map(|x| x.as_i64()).collect())
            .unwrap_or_default();
        // „Stern“ vom Handy → Stern-Playlist am PC (zusammenführen, nichts entfernen)
        if p["star"].as_bool() == Some(true) {
            if let Some(sid) = playlists::star(&c, true)? {
                playlists::add_items(&mut c, sid, &ids)?;
            }
            continue;
        }
        let id = match map.get(&uid).copied().filter(|id| existing.contains(id)) {
            Some(id) => {
                playlists::update(&c, id, &name, None)?;
                id
            }
            None => playlists::create(&c, &name, None)?,
        };
        playlists::set_items(&mut c, id, &ids)?;
        map.insert(uid.clone(), id);
        seen.insert(uid);
    }
    // am Handy gelöschte Playlisten auch hier entfernen
    let gone: Vec<String> = map
        .keys()
        .filter(|k| k.starts_with(prefix) && !seen.contains(*k))
        .cloned()
        .collect();
    for k in gone {
        if let Some(id) = map.remove(&k) {
            let _ = playlists::delete(&c, id);
        }
    }
    db::set_setting(&c, "remote_plmap", &serde_json::to_string(&map)?)?;
    Ok(())
}

fn serve_file(req: tiny_http::Request, path: &Path) -> std::io::Result<()> {
    let Ok(mut f) = std::fs::File::open(path) else {
        return text(req, 404, String::new(), false);
    };
    let len = f.metadata()?.len();
    let range = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| h.value.as_str().strip_prefix("bytes=").map(str::to_string));
    let (start, end, status) = match range.and_then(|r| {
        let (a, b) = r.split_once('-')?;
        if a.trim().is_empty() {
            // „bytes=-500“: die letzten 500 Byte
            let n: u64 = b.trim().parse().ok()?;
            return Some((len.saturating_sub(n), len.saturating_sub(1)));
        }
        let a: u64 = a.trim().parse().ok()?;
        let b: u64 = b.trim().parse().unwrap_or(len.saturating_sub(1));
        Some((a, b.min(len.saturating_sub(1))))
    }) {
        Some((a, b)) if a <= b && a < len => (a, b, 206),
        _ => (0, len.saturating_sub(1), 200),
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    f.seek(SeekFrom::Start(start))?;
    let mut headers = vec![
        header("Content-Type", crate::cast::mime(path)),
        header("Accept-Ranges", "bytes"),
    ];
    if status == 206 {
        headers.push(header(
            "Content-Range",
            &format!("bytes {start}-{end}/{len}"),
        ));
    }
    let body: Box<dyn Read + Send> = if *req.method() == tiny_http::Method::Head {
        Box::new(std::io::empty())
    } else {
        Box::new(f.take(n))
    };
    req.respond(tiny_http::Response::new(
        tiny_http::StatusCode(status),
        headers,
        body,
        Some(n as usize),
        None,
    ))
}

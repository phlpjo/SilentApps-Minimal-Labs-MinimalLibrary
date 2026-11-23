//! Web-Server des NAS-Pakets: Oberfläche (statische Dateien), Befehle (/api/invoke), Ereignisse (/api/events, SSE),
//! Musikdateien zum Abspielen (/api/file, mit Range) und Anmeldung mit Passwort.
use crate::tauri::AppHandle;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, StatusCode};

pub struct Options {
    pub data: PathBuf,
    pub music: PathBuf,
    pub web: PathBuf,
    pub port: u16,
}

impl Options {
    pub fn from_env() -> Self {
        let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)).unwrap_or_default();
        let data = std::env::var("ML_DATA").map(PathBuf::from).unwrap_or_else(|_| exe_dir.join("data"));
        let music = std::env::var("ML_MUSIC").map(PathBuf::from).unwrap_or_else(|_| {
            ["/volume1/music", "/volume1/Music"].iter().map(PathBuf::from).find(|p| p.is_dir()).unwrap_or_else(|| data.join("music"))
        });
        let web = std::env::var("ML_WEB").map(PathBuf::from).unwrap_or_else(|_| exe_dir.join("web"));
        let port = std::env::var("ML_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8642);
        Options { data, music, web, port }
    }
}

// ───────── Ereignisse an alle Browser ─────────

#[derive(Clone, Default)]
pub struct Bus(Arc<Mutex<Vec<Sender<String>>>>);

impl Bus {
    pub fn send(&self, event: &str, payload: Value) {
        let line = format!("data: {}\n\n", json!({ "event": event, "payload": payload }));
        self.0.lock().unwrap().retain(|tx| tx.send(line.clone()).is_ok());
    }
    fn subscribe(&self) -> Receiver<String> {
        let (tx, rx) = channel();
        let _ = tx.send(": hallo\n\n".into());
        self.0.lock().unwrap().push(tx);
        rx
    }
}

/// Liest SSE-Zeilen aus dem Kanal; alle 20 s ein Lebenszeichen, damit Proxys die Verbindung halten
struct SseBody {
    rx: Receiver<String>,
    buf: Vec<u8>,
    pos: usize,
}
impl Read for SseBody {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        while self.pos >= self.buf.len() {
            match self.rx.recv_timeout(Duration::from_secs(20)) {
                Ok(s) => self.buf = s.into_bytes(),
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => self.buf = b": ping\n\n".to_vec(),
                Err(_) => return Ok(0),
            }
            self.pos = 0;
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

// ───────── Argumente der Befehle ─────────

pub fn arg<T: serde::de::DeserializeOwned>(args: &Value, key: &str) -> Result<T, String> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null)).map_err(|e| format!("Argument {key}: {e}"))
}

// ───────── Anmeldung ─────────

struct Auth {
    file: PathBuf,
    sessions: Mutex<HashMap<String, i64>>,
}

impl Auth {
    fn load_pw(&self) -> Option<(String, String)> {
        let v: Value = serde_json::from_slice(&std::fs::read(&self.file).ok()?).ok()?;
        Some((v["salt"].as_str()?.into(), v["hash"].as_str()?.into()))
    }
    fn hash(salt: &str, pw: &str) -> String {
        let mut h = Sha256::new();
        h.update(salt.as_bytes());
        h.update(pw.as_bytes());
        // ein paar Runden, damit Durchprobieren teuer wird
        let mut d = h.finalize().to_vec();
        for _ in 0..100_000 {
            d = Sha256::digest(&d).to_vec();
        }
        d.iter().map(|b| format!("{b:02x}")).collect()
    }
    fn set(&self, pw: &str) -> anyhow::Result<()> {
        let salt = token(16);
        std::fs::write(&self.file, json!({ "salt": salt, "hash": Self::hash(&salt, pw) }).to_string())?;
        Ok(())
    }
    fn check(&self, pw: &str) -> bool {
        self.load_pw().is_some_and(|(s, h)| Self::hash(&s, pw) == h)
    }
    fn session_file(&self) -> PathBuf {
        self.file.with_file_name("web-sessions.json")
    }
    fn new_session(&self) -> String {
        let t = token(24);
        let mut m = self.sessions.lock().unwrap();
        m.insert(t.clone(), now() + 60 * 86400);
        let _ = std::fs::write(self.session_file(), serde_json::to_vec(&*m).unwrap_or_default());
        t
    }
    fn valid(&self, t: &str) -> bool {
        self.sessions.lock().unwrap().get(t).is_some_and(|&exp| exp > now())
    }
    fn logout(&self, t: &str) {
        let mut m = self.sessions.lock().unwrap();
        m.remove(t);
        let _ = std::fs::write(self.session_file(), serde_json::to_vec(&*m).unwrap_or_default());
    }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn token(n: usize) -> String {
    let mut b = vec![0u8; n];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn header(k: &str, v: &str) -> Header {
    Header::from_bytes(k.as_bytes(), v.as_bytes()).unwrap()
}

fn cookie(req: &Request, name: &str) -> Option<String> {
    req.headers().iter().find(|h| h.field.equiv("Cookie")).and_then(|h| {
        h.value.as_str().split(';').find_map(|kv| kv.trim().strip_prefix(&format!("{name}=")).map(str::to_string))
    })
}

// ───────── Server ─────────

pub fn serve(app: AppHandle, rt: tokio::runtime::Handle, opt: Options) -> anyhow::Result<()> {
    let auth = Arc::new(Auth { file: opt.data.join("web-password.json"), sessions: Mutex::new(HashMap::new()) });
    if let Ok(b) = std::fs::read(auth.session_file())
        && let Ok(m) = serde_json::from_slice::<HashMap<String, i64>>(&b)
    {
        *auth.sessions.lock().unwrap() = m;
    }
    let server = Arc::new(tiny_http::Server::http(("0.0.0.0", opt.port)).map_err(|e| anyhow::anyhow!("Port {}: {e}", opt.port))?);
    eprintln!("Minimal Library läuft auf Port {} (Daten: {})", opt.port, opt.data.display());
    let web = Arc::new(opt.web);
    for req in server.incoming_requests() {
        let app = app.clone();
        let rt = rt.clone();
        let auth = auth.clone();
        let web = web.clone();
        std::thread::spawn(move || {
            let _ = handle(&app, &rt, &auth, &web, req);
        });
    }
    Ok(())
}

fn json_resp(req: Request, status: u16, v: &Value) -> std::io::Result<()> {
    req.respond(
        Response::from_string(v.to_string())
            .with_status_code(StatusCode(status))
            .with_header(header("Content-Type", "application/json; charset=utf-8"))
            .with_header(header("Cache-Control", "no-store")),
    )
}

fn text_resp(req: Request, status: u16, s: &str) -> std::io::Result<()> {
    req.respond(Response::from_string(s).with_status_code(StatusCode(status)).with_header(header("Content-Type", "text/plain; charset=utf-8")))
}

fn handle(app: &AppHandle, rt: &tokio::runtime::Handle, auth: &Auth, web: &Path, mut req: Request) -> std::io::Result<()> {
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let path = percent_encoding::percent_decode_str(path).decode_utf8_lossy().to_string();
    let method = req.method().clone();

    // Anmeldung (ohne Sitzung erreichbar)
    if path == "/api/auth" {
        let has_pw = auth.load_pw().is_some();
        let ok = cookie(&req, "mls").is_some_and(|t| auth.valid(&t));
        return json_resp(req, 200, &json!({ "setup": !has_pw, "loggedIn": ok }));
    }
    if path == "/api/login" && method == Method::Post {
        let mut body = String::new();
        req.as_reader().take(4096).read_to_string(&mut body)?;
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        let pw = v["password"].as_str().unwrap_or("");
        let ok = if auth.load_pw().is_none() {
            // Erste Anmeldung: Passwort festlegen
            if pw.chars().count() < 6 {
                return json_resp(req, 400, &json!({ "error": "Mindestens 6 Zeichen" }));
            }
            auth.set(pw).is_ok()
        } else {
            std::thread::sleep(Duration::from_millis(400)); // Durchprobieren bremsen
            auth.check(pw)
        };
        if !ok {
            return json_resp(req, 401, &json!({ "error": "Falsches Passwort" }));
        }
        let t = auth.new_session();
        return req.respond(
            Response::from_string(json!({ "ok": true }).to_string())
                .with_header(header("Content-Type", "application/json"))
                .with_header(header("Set-Cookie", &format!("mls={t}; Path=/; HttpOnly; SameSite=Strict; Max-Age=5184000"))),
        );
    }
    let session = cookie(&req, "mls").filter(|t| auth.valid(t));
    if path.starts_with("/api/") && session.is_none() {
        return json_resp(req, 401, &json!({ "error": "Bitte anmelden" }));
    }
    if path == "/api/logout" {
        if let Some(t) = &session {
            auth.logout(t);
        }
        return json_resp(req, 200, &json!({ "ok": true }));
    }
    if path == "/api/password" && method == Method::Post {
        let mut body = String::new();
        req.as_reader().take(4096).read_to_string(&mut body)?;
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        let (old, new) = (v["old"].as_str().unwrap_or(""), v["new"].as_str().unwrap_or(""));
        if !auth.check(old) {
            return json_resp(req, 401, &json!({ "error": "Altes Passwort stimmt nicht" }));
        }
        if new.chars().count() < 6 {
            return json_resp(req, 400, &json!({ "error": "Mindestens 6 Zeichen" }));
        }
        let _ = auth.set(new);
        return json_resp(req, 200, &json!({ "ok": true }));
    }

    // Befehle
    if let Some(cmd) = path.strip_prefix("/api/invoke/") {
        let mut body = String::new();
        req.as_reader().read_to_string(&mut body)?;
        let args: Value = if body.trim().is_empty() { json!({}) } else { serde_json::from_str(&body).unwrap_or(json!({})) };
        let cmd = cmd.to_string();
        let app2 = app.clone();
        let r = rt.block_on(async move { crate::dispatch(&app2, &cmd, &args).await });
        return match r {
            Ok(v) => json_resp(req, 200, &v),
            Err(e) => json_resp(req, 400, &Value::String(e)),
        };
    }
    // Ereignisse vom Browser an alle (z. B. „Wiedergabe gestartet“)
    if path == "/api/emit" && method == Method::Post {
        let mut body = String::new();
        req.as_reader().take(1 << 20).read_to_string(&mut body)?;
        let v: Value = serde_json::from_str(&body).unwrap_or_default();
        if let Some(ev) = v["event"].as_str() {
            app.0.bus.send(ev, v["payload"].clone());
        }
        return json_resp(req, 200, &json!({}));
    }
    if path == "/api/events" {
        let body = SseBody { rx: app.0.bus.subscribe(), buf: vec![], pos: 0 };
        let resp = Response::new(
            StatusCode(200),
            vec![header("Content-Type", "text/event-stream"), header("Cache-Control", "no-store"), header("X-Accel-Buffering", "no")],
            body,
            None,
            None,
        );
        return req.respond(resp);
    }
    // Musikdatei / Bild zum Abspielen
    if path == "/api/file" {
        let p = query
            .split('&')
            .find_map(|kv| kv.strip_prefix("p="))
            .map(|v| percent_encoding::percent_decode_str(v).decode_utf8_lossy().to_string())
            .unwrap_or_default();
        let p = PathBuf::from(p);
        if !app.may_serve(&p) {
            return text_resp(req, 403, "nicht freigegeben");
        }
        return serve_file(req, &p);
    }
    if path.starts_with("/api/") {
        return text_resp(req, 404, "unbekannt");
    }

    // Oberfläche (ohne Anmeldung nur die Anmeldeseite; die Oberfläche fragt /api/auth)
    let rel = path.trim_start_matches('/');
    let mut file = web.join(if rel.is_empty() { "index.html" } else { rel });
    if rel.contains("..") || !file.is_file() {
        file = web.join("index.html");
    }
    serve_file(req, &file)
}

fn mime(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("woff2") => "font/woff2",
        Some("json") => "application/json",
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("m4a") | Some("aac") | Some("alac") => "audio/mp4",
        Some("ogg") | Some("oga") | Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("aif") | Some("aiff") => "audio/aiff",
        _ => "application/octet-stream",
    }
}

/// Datei mit Range-Unterstützung (Spulen im Browser)
fn serve_file(req: Request, p: &Path) -> std::io::Result<()> {
    let Ok(mut f) = std::fs::File::open(p) else { return text_resp(req, 404, "nicht gefunden") };
    let len = f.metadata()?.len();
    let range = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| h.value.as_str().strip_prefix("bytes=").map(str::to_string));
    let ty = mime(p);
    let cache = if ty.starts_with("text/html") { "no-store" } else { "max-age=3600" };
    if let Some(r) = range
        && let Some((a, b)) = r.split_once('-')
    {
        let start: u64 = a.parse().unwrap_or(0);
        let end: u64 = b.parse().unwrap_or(len.saturating_sub(1)).min(len.saturating_sub(1));
        if start <= end && start < len {
            use std::io::Seek;
            f.seek(std::io::SeekFrom::Start(start))?;
            let n = end - start + 1;
            return req.respond(Response::new(
                StatusCode(206),
                vec![
                    header("Content-Type", ty),
                    header("Accept-Ranges", "bytes"),
                    header("Content-Range", &format!("bytes {start}-{end}/{len}")),
                    header("Cache-Control", cache),
                ],
                f.take(n),
                Some(n as usize),
                None,
            ));
        }
    }
    req.respond(Response::new(
        StatusCode(200),
        vec![header("Content-Type", ty), header("Accept-Ranges", "bytes"), header("Cache-Control", cache)],
        f,
        Some(len as usize),
        None,
    ))
}

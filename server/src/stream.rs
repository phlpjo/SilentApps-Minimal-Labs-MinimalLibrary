//! „Minimal Library Stream“: schlanke NAS-Variante nur fürs Handy.
//! Liest die Musikordner ein (beim Start, alle 6 Stunden und bei Änderungen), keine Analyse,
//! keine große Oberfläche – nur eine kleine Seite zum Koppeln, für Musikordner und Lizenz.

use crate::AppHandle;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::OnceLock;

static SCANNING: AtomicBool = AtomicBool::new(false);
static LAST: AtomicI64 = AtomicI64::new(0);
static FOUND: AtomicI64 = AtomicI64::new(-1);
static RT: OnceLock<tokio::runtime::Handle> = OnceLock::new();

pub fn enabled() -> bool {
    std::env::var("ML_MODE").is_ok_and(|m| m == "stream")
}

pub fn start(app: &AppHandle, rt: tokio::runtime::Handle) {
    let _ = RT.set(rt);
    rescan(app);
    let app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(6 * 3600));
        rescan(&app);
    });
}

/// Alle Musikordner neu einlesen (im Hintergrund; läuft schon einer, passiert nichts)
pub fn rescan(app: &AppHandle) {
    let Some(rt) = RT.get().cloned() else { return };
    if SCANNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        let r = rt.block_on(async { crate::dispatch(&app, "scan_library", &json!({})).await });
        if let Ok(v) = &r {
            FOUND.store(v["found"].as_i64().unwrap_or(-1), Ordering::Relaxed);
        }
        if let Err(e) = &r {
            musiccore::diag::log(format!("Stream: Einlesen fehlgeschlagen: {e}"));
        }
        LAST.store(musiccore::now(), Ordering::Relaxed);
        SCANNING.store(false, Ordering::SeqCst);
    });
}

/// Adressen dieses NAS im Netz (für „Adresse eingeben“ in der Handy-App)
fn addresses() -> Vec<String> {
    let mut out = Vec::new();
    // Hauptadresse: Weg nach draußen (es wird nichts gesendet)
    if let Ok(s) = std::net::UdpSocket::bind("0.0.0.0:0")
        && s.connect("8.8.8.8:80").is_ok()
        && let Ok(a) = s.local_addr()
    {
        out.push(a.ip().to_string());
    }
    // weitere aus `hostname -I` (Synology/Linux)
    if let Ok(o) = std::process::Command::new("hostname").arg("-I").output() {
        for a in String::from_utf8_lossy(&o.stdout).split_whitespace() {
            if !a.contains(':') && !a.starts_with("127.") && !a.starts_with("169.254.") && !out.iter().any(|x| x == a) {
                out.push(a.to_string());
            }
        }
    }
    out
}

/// Zustand für die kleine Seite
pub fn status(app: &AppHandle) -> Result<Value, String> {
    let st = crate::tab_state(app, "main").map_err(|e| e.to_string())?;
    let c = st.db.lock().unwrap();
    let roots = crate::load_settings(&c).roots;
    let n = |q: &str| c.query_row(q, [], |r| r.get::<_, i64>(0)).unwrap_or(0);
    let mut rs = Vec::new();
    for r in &roots {
        let pre = format!("{}/", r.trim_end_matches('/'));
        let cnt: i64 = c
            .query_row("SELECT COUNT(*) FROM tracks WHERE substr(path, 1, length(?1)) = ?1", [&pre], |x| x.get(0))
            .unwrap_or(0);
        let ok = std::fs::read_dir(r).is_ok();
        rs.push(json!({ "path": r, "tracks": cnt, "ok": ok }));
    }
    Ok(json!({
        "tracks": n("SELECT COUNT(*) FROM tracks"),
        "playlists": n("SELECT COUNT(*) FROM playlists"),
        "roots": rs,
        "scanning": SCANNING.load(Ordering::Relaxed),
        "lastScan": LAST.load(Ordering::Relaxed),
        "addresses": addresses(),
        "port": musiccore::remote::port(),
        "name": musiccore::remote::computer_name(),
        "version": app.0.version,
    }))
}

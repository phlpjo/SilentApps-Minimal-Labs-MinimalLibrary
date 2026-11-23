//! Befehle, die es nur am PC gibt (Fenster, Tabs, Infobereich), und eigene Befehle des NAS-Servers.
use crate::tauri::AppHandle;
use serde_json::{Value, json};

/// Am NAS: keine Infobereich-Titel und kein Sprachmenü im Infobereich
pub fn set_tray_title(_app: &AppHandle, _en: bool) {}

pub(crate) async fn call(app: &AppHandle, name: &str, args: &Value) -> Result<Value, String> {
    match name {
        // Nur eine Bibliothek (keine Tabs) im Browser
        "tabs_list" => Ok(json!([{ "label": "main", "name": "Bibliothek" }])),
        "tab_show" | "tab_rename" | "set_lang" | "set_tray_title" | "quit_app" => Ok(Value::Null),
        "tab_create" | "tab_close" => Err("Auf dem NAS gibt es nur eine Bibliothek.".into()),
        "quit" => Ok(Value::Null),
        // Ordnerauswahl im Browser: Inhalt eines Ordners auf dem NAS
        "server_browse" => browse(app, args),
        "server_info" => Ok(json!({ "nas": true, "music": app.0.music, "data": app.0.data, "version": app.0.version })),
        other => Err(format!("Befehl „{other}“ gibt es auf dem NAS nicht")),
    }
}

/// Ordner auf dem NAS auflisten (für die Ordnerauswahl). Start: /volume*-Freigaben.
fn browse(_app: &AppHandle, args: &Value) -> Result<Value, String> {
    let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let files = args.get("files").and_then(|v| v.as_bool()).unwrap_or(false);
    if path.is_empty() {
        // Volumes und ihre Freigaben
        let mut out = vec![];
        for vol in std::fs::read_dir("/").map_err(|e| e.to_string())?.flatten() {
            let n = vol.file_name().to_string_lossy().to_string();
            if !n.starts_with("volume") {
                continue;
            }
            if let Ok(rd) = std::fs::read_dir(vol.path()) {
                for sh in rd.flatten() {
                    let s = sh.file_name().to_string_lossy().to_string();
                    if s.starts_with('@') || s.starts_with('#') || s.starts_with('.') || !sh.path().is_dir() {
                        continue;
                    }
                    // Freigaben ohne Rechte fürs Paket kennzeichnen
                    let ok = std::fs::read_dir(sh.path()).is_ok();
                    let label = if ok { format!("{s} ({n})") } else { format!("{s} ({n}) – keine Freigabe") };
                    out.push(json!({ "name": label, "path": sh.path(), "dir": true }));
                }
            }
        }
        if out.is_empty() {
            // kein DSM (Test am PC): Wurzel
            out.push(json!({ "name": "/", "path": "/", "dir": true }));
        }
        return Ok(json!({ "path": "", "parent": null, "entries": out }));
    }
    let p = std::path::Path::new(path);
    let mut entries = vec![];
    let rd = std::fs::read_dir(p).map_err(|e| {
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            no_access(path)
        } else {
            format!("{path}: {e}")
        }
    })?;
    for e in rd.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if n.starts_with('@') || n.starts_with('#') || n.starts_with('.') {
            continue;
        }
        let dir = e.path().is_dir();
        if !dir && !files {
            continue;
        }
        entries.push(json!({ "name": n, "path": e.path(), "dir": dir }));
    }
    entries.sort_by(|a, b| {
        (!a["dir"].as_bool().unwrap_or(false), a["name"].as_str().unwrap_or("").to_lowercase())
            .cmp(&(!b["dir"].as_bool().unwrap_or(false), b["name"].as_str().unwrap_or("").to_lowercase()))
    });
    let parent = p.parent().map(|x| x.to_string_lossy().to_string()).filter(|x| x != "/" && !x.is_empty());
    Ok(json!({ "path": path, "parent": parent, "entries": entries }))
}

/// Verständliche Meldung, wenn dem Paket die Freigabe für einen Ordner fehlt
pub fn no_access(path: &str) -> String {
    let share = path.trim_start_matches('/').split('/').nth(1).unwrap_or(path);
    format!(
        "Kein Zugriff auf „{share}“. In DSM freigeben: Systemsteuerung → Gemeinsamer Ordner → „{share}“ → Bearbeiten → Berechtigungen → oben „Systeminterner Benutzer“ wählen → „MinimalLibrary“: Lesen/Schreiben → Speichern. Danach hier erneut versuchen."
    )
}

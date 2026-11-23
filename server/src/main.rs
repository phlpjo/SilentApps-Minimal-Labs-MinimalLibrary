//! Minimal Library für Synology DSM: dieselbe Bibliothek wie am PC, die Oberfläche läuft im Browser.
//! Befehle und Logik kommen unverändert aus der Desktop-App (siehe build.rs).
#![allow(unused_imports, dead_code, unused_braces, clippy::all)]

mod stubs;
mod tauri;
mod web;

// Befehle der Desktop-App (aus src-tauri/src/lib.rs erzeugt)
include!(concat!(env!("OUT_DIR"), "/app.rs"));

fn main() -> anyhow::Result<()> {
    let opt = web::Options::from_env();
    std::fs::create_dir_all(&opt.data)?;
    let bus = web::Bus::default();
    let app = AppHandle::new(opt.data.clone(), opt.music.clone(), env!("CARGO_PKG_VERSION"), bus);
    app.manage(Tabs {
        base: opt.data.clone(),
        map: Mutex::new(HashMap::new()),
        active: Mutex::new("main".into()),
    });
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let _guard = rt.enter();
    let main = tab_state(&app, "main")?;
    // Erster Start: den bei der Installation freigegebenen Musikordner gleich als Bibliothek eintragen
    {
        let c = main.db.lock().unwrap();
        let mut st = load_settings(&c);
        if st.roots.is_empty() && opt.music.is_dir() && std::fs::read_dir(&opt.music).is_ok() {
            st.roots.push(opt.music.to_string_lossy().to_string());
            let _ = db::set_setting(&c, "settings", &serde_json::to_string(&st).unwrap_or_default());
        }
        for r in &st.roots {
            let _ = app.asset_protocol_scope().allow_directory(r, true);
        }
    }
    let _ = license_compute(&app);
    start_remote(&app, &main);
    {
        let h = app.clone();
        std::thread::spawn(move || {
            loop {
                license_check_revoked(&h);
                let before = musiccore::license::unlocked();
                if let Ok(s) = license_compute(&h)
                    && s.unlocked() != before
                {
                    let _ = h.emit("license-changed", &s);
                }
                std::thread::sleep(std::time::Duration::from_secs(3600));
            }
        });
    }
    web::serve(app, rt.handle().clone(), opt)
}

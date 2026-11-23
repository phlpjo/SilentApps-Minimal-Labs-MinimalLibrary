//! Startet die Handy-Freigabe mit einer Testbibliothek (für Tests mit curl / dem Handy)
use musiccore::{db, playlists, remote};
use rusqlite::params;
use std::sync::{Arc, Mutex};
fn main() {
    let base = std::env::temp_dir().join("remoteself");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("Musik/Band/Platte")).unwrap();
    let conn = db::open(&base.join("t.db")).unwrap();
    for (n, t) in [("a.mp3", "Eins"), ("b.mp3", "Zwei")] {
        let f = base.join("Musik/Band/Platte").join(n);
        std::fs::write(
            &f,
            (0..1000u32).map(|i| (i % 251) as u8).collect::<Vec<_>>(),
        )
        .unwrap();
        conn.execute("INSERT INTO tracks(path, size, title, artist, album, duration) VALUES(?1, 1000, ?2, 'Band', 'Platte', 61.5)", params![f.to_string_lossy(), t]).unwrap();
    }
    let mut conn = conn;
    let p = playlists::create(&conn, "Auto", None).unwrap();
    playlists::set_items(&mut conn, p, &[2, 1]).unwrap();
    let root = base.join("Musik").to_string_lossy().to_string();
    remote::start(remote::Host {
        db: Arc::new(Mutex::new(conn)),
        settings: Box::new(move || remote::HostSettings {
            roots: vec![root.clone()],
            pattern: "{artist}/{title}".into(),
            organize_target: None,
            enabled: true,
            phone_dir: std::path::Path::new(&root).join("Handys"),
            device_dirs: Default::default(),
        }),
        changed: Box::new(|c| println!("geändert: {c:?}")),
    });
    println!("Code {} Port {}", remote::code(), remote::port());
    std::thread::sleep(std::time::Duration::from_secs(
        std::env::args()
            .nth(1)
            .and_then(|a| a.parse().ok())
            .unwrap_or(60),
    ));
}

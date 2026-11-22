//! Prüft den WLAN-Sync gegen ein (nachgebautes) Handy: suchen, koppeln, synchronisieren
use musiccore::{db, playlists, sync, wlan};
use rusqlite::params;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
fn main() {
    let base = std::env::temp_dir().join("wlanself");
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(base.join("lib")).unwrap();
    let phones = wlan::phones();
    println!("gefunden: {phones:?}");
    let p = phones.first().expect("kein Handy");
    assert!(wlan::pair(&p.addr, "111111", "PC").is_err());
    let token = wlan::pair(&p.addr, "123 456", "PC").unwrap();
    let conn = Mutex::new(db::open(&base.join("t.db")).unwrap());
    let mut ids = vec![];
    for (n, t) in [("a.mp3", "Eins & Zwei"), ("b.mp3", "Drei+Vier")] {
        let f = base.join("lib").join(n);
        std::fs::write(&f, vec![7u8; 300_000]).unwrap();
        let c = conn.lock().unwrap();
        c.execute("INSERT INTO tracks(path, size, title, artist, album) VALUES(?1, 300000, ?2, 'Bänd', 'Platte')", params![f.to_string_lossy(), t]).unwrap();
        ids.push(c.last_insert_rowid());
    }
    let pid = {
        let mut c = conn.lock().unwrap();
        let p = playlists::create(&c, "Auto", None).unwrap();
        playlists::set_items(&mut c, p, &ids).unwrap();
        p
    };
    let t = sync::Target {
        name: p.name.clone(),
        device: format!("wlan:{}", p.id),
        device_name: p.name.clone(),
        token,
        playlists: vec![pid],
        mirror: true,
        layout: "numbered".into(),
        ..Default::default()
    };
    let id = sync::save(&conn.lock().unwrap(), &t).unwrap();
    let st = sync::list(&conn.lock().unwrap()).unwrap();
    println!("verfügbar: {} frei {:?}", st[0].available, st[0].free);
    let r = sync::run(&conn, id, &AtomicBool::new(false), &|i, n, s| {
        println!("{i}/{n} {s}")
    })
    .unwrap();
    println!("1: {r:?}");
    assert_eq!(r.copied, 2);
    let r = sync::run(&conn, id, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    println!("2: {r:?}");
    assert_eq!(r.skipped, 2);
    playlists::set_items(&mut conn.lock().unwrap(), pid, &ids[..1]).unwrap();
    let r = sync::run(&conn, id, &AtomicBool::new(false), &|_, _, _| {}).unwrap();
    println!("3: {r:?}");
    assert_eq!(r.removed, 1);
    println!("OK");
}

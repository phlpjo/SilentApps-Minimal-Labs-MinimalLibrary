//! Ende-zu-Ende-Test: cargo run -p musiccore --release --example selftest -- <musikordner> <arbeitsordner>
use musiccore::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let music = PathBuf::from(&args[1]);
    let work = PathBuf::from(&args[2]);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work)?;
    let conn = Mutex::new(db::open(&work.join("lib.db"))?);
    let prog = |d: usize, t: usize, m: &str| eprintln!("  [{d}/{t}] {m}");

    let r = scanner::scan(&conn, &[music.clone()], &prog)?;
    println!("Scan: {r:?}");
    let cancel = AtomicBool::new(false);
    let t0 = std::time::Instant::now();
    analysis::analyze_pending(&conn, None, 4, &cancel, &prog, &|_| {})?;
    println!("Analyse in {:?}", t0.elapsed());
    let tracks = db::all_tracks(&conn.lock().unwrap())?;
    for t in &tracks {
        let pk = db::peaks(&conn.lock().unwrap(), t.id)?.unwrap_or_default();
        println!(
            "{:>3} {:<28} bpm={:?} key={:?} dur={:.1} peaks={} max={} err={:?}",
            t.id,
            std::path::Path::new(&t.path).file_name().unwrap().to_string_lossy(),
            t.bpm, t.key, t.duration, pk.len(), pk.iter().max().unwrap_or(&0), t.analysis_error
        );
    }
    let fps = db::fingerprints(&conn.lock().unwrap())?;
    let g = duplicates::find(&tracks, &fps, duplicates::Modes { metadata: true, filename: true, audio: true }, &prog);
    println!("Duplikate: {g:#?}");
    if tracks.len() >= 2 {
        let a = &fps[&tracks[0].id];
        for t in &tracks[1..] {
            println!("sim {} ↔ {}: {:.3}", tracks[0].id, t.id, duplicates::fp_similarity(a, &fps[&t.id]));
        }
    }

    // Tags schreiben
    let t = &tracks[0];
    tags::write(std::path::Path::new(&t.path), &tags::TagUpdate { genre: Some("Techno".into()), bpm: Some("128".into()), key: Some("Am".into()), ..Default::default() })?;
    println!("Neu gelesen: {:?}", tags::read(std::path::Path::new(&t.path))?);

    // Sortieren: kopieren in work/sorted
    let plan = organize::plan(&tracks, &work.join("sorted"), organize::DEFAULT_PATTERN);
    for m in &plan { println!("Plan: {} → {} {}", m.from, m.to, if m.renamed {"(umbenannt)"} else {""}); }
    let rep = organize::apply(&conn, &plan, false, "Test", &[music.clone()], &prog)?;
    println!("Verschoben: {} Fehler: {:?}", rep.moved, rep.errors);
    let undo = organize::undo(&conn, rep.batch_id, &[work.clone()])?;
    println!("Rückgängig: {} Fehler: {:?}", undo.moved, undo.errors);

    // Playlist + Export
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    let pid = { let mut c = conn.lock().unwrap(); let p = playlists::create(&c, "Mein Mix", None)?; playlists::set_items(&mut c, p, &ids)?; p };
    let smart = playlists::SmartRules { all: true, rules: vec![playlists::Rule { field: "bpm".into(), op: "gt".into(), value: "100".into(), value2: String::new() }], sort: "bpm".into(), desc: false, limit: None };
    { let c = conn.lock().unwrap(); playlists::create(&c, "Schnell", Some(&smart))?; }
    println!("Playlisten: {:?}", playlists::list(&conn.lock().unwrap())?);
    playlists::sync_folder(&conn.lock().unwrap(), &music.join("Playlists"))?;
    println!("{}", std::fs::read_to_string(music.join("Playlists/Mein Mix.m3u8"))?);
    let ex = export::export(&conn, &[pid], &work.join("usb"), export::Layout::Numbered, &cancel, &prog)?;
    println!("Export: {ex:?}");
    println!("{}", std::fs::read_to_string(work.join("usb/Mein Mix/Mein Mix.m3u8"))?);
    Ok(())
}

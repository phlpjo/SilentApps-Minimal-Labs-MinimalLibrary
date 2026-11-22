//! Ende-zu-Ende für den Pioneer-Export:
//! cargo run -p musiccore --release --example pioneer_selftest -- <musikordner> <arbeitsordner>
//! Musikordner → Analyse (mit Beatgrid/Wellenform) → Cues → Export auf <arbeit>/stick + rekordbox.xml.
//! Schreibt <arbeit>/expected.json für scripts/verify_pioneer.py (Prüfung mit pyrekordbox).
use musiccore::pioneer::{self, Cue};
use musiccore::*;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let music = PathBuf::from(&args[1]);
    let work = PathBuf::from(&args[2]);
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(work.join("stick"))?;
    let conn = Mutex::new(db::open(&work.join("lib.db"))?);
    let prog = |d: usize, t: usize, m: &str| eprintln!("  [{d}/{t}] {m}");
    scanner::scan(&conn, &[music.clone()], &prog)?;
    let cancel = AtomicBool::new(false);
    analysis::analyze_pending_with(&conn, None, 4, true, &cancel, &prog, &|_| {})?;
    let mut tracks = db::all_tracks(&conn.lock().unwrap())?;
    tracks.sort_by(|a, b| a.path.cmp(&b.path));
    let ids: Vec<i64> = tracks.iter().map(|t| t.id).collect();
    {
        let c = conn.lock().unwrap();
        pioneer::set_cue(
            &c,
            ids[0],
            &Cue {
                id: 0,
                kind: 0,
                start_ms: 2000.0,
                end_ms: None,
                color: None,
                comment: None,
            },
        )?;
        pioneer::set_cue(
            &c,
            ids[0],
            &Cue {
                id: 0,
                kind: 1,
                start_ms: 4000.0,
                end_ms: None,
                color: Some(3),
                comment: Some("Drop".into()),
            },
        )?;
        pioneer::set_cue(
            &c,
            ids[0],
            &Cue {
                id: 0,
                kind: 2,
                start_ms: 6000.0,
                end_ms: Some(8000.0),
                color: Some(6),
                comment: None,
            },
        )?;
    }
    let pid = {
        let mut c = conn.lock().unwrap();
        let p = playlists::create(&c, "Test-Set", None)?;
        playlists::set_items(&mut c, p, &ids.iter().rev().copied().collect::<Vec<_>>())?;
        p
    };
    let o = pioneer::export::Options {
        dest: work.join("stick").to_string_lossy().into(),
        playlists: vec![pid],
        unsupported: Default::default(),
    };
    println!(
        "Vorschau: {:?}",
        pioneer::export::preview(&conn.lock().unwrap(), &o, false)?
    );
    let r = pioneer::export::run(&conn, &o, None, true, &cancel, &prog)?;
    println!("Export: {r:?}");
    let r2 = pioneer::export::run(&conn, &o, None, true, &cancel, &prog)?;
    println!("Zweiter Export: {r2:?}");
    assert_eq!(r2.copied, 0, "zweiter Export darf nichts neu kopieren");
    std::fs::write(
        work.join("rekordbox.xml"),
        pioneer::xml::build(&conn.lock().unwrap(), &[pid], "selftest")?,
    )?;

    // Erwartungen für die unabhängige Prüfung
    let c = conn.lock().unwrap();
    let exp: Vec<serde_json::Value> = tracks
        .iter()
        .map(|t| {
            let g = pioneer::grid(&c, t.id).unwrap();
            let cs = pioneer::cues(&c, t.id).unwrap();
            serde_json::json!({
                "title": t.title.clone().unwrap_or_else(|| std::path::Path::new(&t.path).file_stem().unwrap().to_string_lossy().to_string()),
                "path": t.path,
                "bpm": g.map(|g| g.bpm),
                "downbeat_ms": g.map(|g| g.downbeat_ms),
                "cues": cs.iter().map(|q| serde_json::json!({"kind": q.kind, "start_ms": q.start_ms, "end_ms": q.end_ms, "comment": q.comment})).collect::<Vec<_>>(),
            })
        })
        .collect();
    let order: Vec<i64> = ids.iter().rev().copied().collect();
    let order_titles: Vec<String> = order
        .iter()
        .map(|id| {
            exp[ids.iter().position(|x| x == id).unwrap()]["title"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    std::fs::write(
        work.join("expected.json"),
        serde_json::to_vec_pretty(
            &serde_json::json!({ "tracks": exp, "playlist": "Test-Set", "order": order_titles, "key": pioneer::onelibrary::key() }),
        )?,
    )?;
    println!("OK");
    Ok(())
}

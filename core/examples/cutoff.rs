//! Grenzfrequenz messen: cargo run --example cutoff -- datei…
use std::sync::atomic::AtomicBool;
fn main() {
    for p in std::env::args().skip(1) {
        let r = musiccore::analysis::analyze(std::path::Path::new(&p), &AtomicBool::new(false));
        match r {
            Ok(a) => println!(
                "{p}: cutoff {:?} Hz → ≈ {} kbps",
                a.cutoff,
                a.cutoff
                    .map(musiccore::analysis::kbps_from_cutoff)
                    .unwrap_or(0)
            ),
            Err(e) => println!("{p}: {e}"),
        }
    }
}

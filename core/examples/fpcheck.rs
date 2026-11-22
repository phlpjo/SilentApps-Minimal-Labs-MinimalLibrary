// Vergleich mit fpcalc: cargo run -p musiccore --release --example fpcheck -- <datei> "<fpcalc -raw Werte>"
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let r = musiccore::analysis::analyze(
        std::path::Path::new(&a[1]),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let theirs: Vec<u32> = a[2]
        .split(',')
        .filter_map(|x| x.trim().parse::<i64>().ok())
        .map(|x| x as u32)
        .collect();
    println!(
        "ours {} items, fpcalc {} items",
        r.fingerprint.len(),
        theirs.len()
    );
    println!(
        "similarity {:.3}",
        musiccore::duplicates::fp_similarity(&r.fingerprint, &theirs)
    );
    println!(
        "ours encoded: {}",
        &musiccore::acoustid::encode(&r.fingerprint)[..60]
    );
}

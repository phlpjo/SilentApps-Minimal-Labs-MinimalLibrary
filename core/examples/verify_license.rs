//! Lizenzschlüssel prüfen: cargo run --example verify_license -- MLIB-…
fn main() {
    let k = std::env::args().nth(1).expect("Schlüssel angeben");
    match musiccore::license::verify(&k) {
        Ok(p) => println!("gültig: {} <{}> · {} · id {}", p.n, p.e, p.k, p.id),
        Err(e) => println!("ungültig: {e}"),
    }
}

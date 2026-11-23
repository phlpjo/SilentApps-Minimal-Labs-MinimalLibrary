//! Lizenzschlüssel erzeugen (z. B. Gratis-Lizenzen):
//!   LICENSE_PRIVATE_KEY=… cargo run --example make_license -- --name "Max Muster" --email max@example.org --kind free
//! oder mit --key-file private-key.txt statt der Umgebungsvariable.
use musiccore::license::{Payload, sign, verify};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let get = |k: &str| {
        args.iter()
            .position(|a| a == k)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let private = match get("--key-file") {
        Some(f) => std::fs::read_to_string(f).expect("Schlüsseldatei lesen"),
        None => std::env::var("LICENSE_PRIVATE_KEY")
            .expect("LICENSE_PRIVATE_KEY oder --key-file angeben"),
    };
    let name = get("--name").expect("--name fehlt");
    // zufällige ID (16 Zeichen Base32)
    let mut seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u128;
    let mut id = String::new();
    let abc = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    for _ in 0..16 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407 ^ std::process::id() as u128);
        id.push(abc[((seed >> 64) % 32) as usize] as char);
    }
    let p = Payload {
        p: "minimal-library".into(),
        v: 1,
        id,
        n: name,
        e: get("--email").unwrap_or_default(),
        k: get("--kind").unwrap_or_else(|| "free".into()),
        t: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64,
    };
    let key = sign(&private, &p).expect("signieren");
    verify(&key).expect("Kontrolle fehlgeschlagen – passt der private Schlüssel zur App?");
    println!("{key}");
    println!("\nFreischalt-Link: minimallibrary://activate?key={key}");
}

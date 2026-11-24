//! Protokoll für die Fehlersuche: was die Bibliothek entfernt, neu aufnimmt oder neu analysiert – und warum.
//! Eine kleine Textdatei im App-Ordner (wird ab 2 MB umbenannt in .1).

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

static FILE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn init(path: PathBuf) {
    *FILE.lock().unwrap() = Some(path);
}

pub fn path() -> Option<PathBuf> {
    FILE.lock().unwrap().clone()
}

/// Zeit als „JJJJ-MM-TT hh:mm:ss“ (UTC)
fn stamp() -> String {
    let s = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let (days, rem) = (s.div_euclid(86400), s.rem_euclid(86400));
    // Tage → Datum (Algorithmus nach H. Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}:{:02} UTC", rem / 3600, rem % 3600 / 60, rem % 60)
}

pub fn log(msg: impl AsRef<str>) {
    let Some(p) = path() else { return };
    if std::fs::metadata(&p).map(|m| m.len() > 2_000_000).unwrap_or(false) {
        let _ = std::fs::rename(&p, p.with_extension("log.1"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&p) {
        let _ = writeln!(f, "{} {}", stamp(), msg.as_ref());
    }
}

/// Bis zu `n` Beispiele einer Liste für das Protokoll
pub fn sample<T: AsRef<str>>(items: &[T], n: usize) -> String {
    let mut s: Vec<&str> = items.iter().take(n).map(|x| x.as_ref()).collect();
    if items.len() > n {
        s.push("…");
    }
    s.join(" | ")
}

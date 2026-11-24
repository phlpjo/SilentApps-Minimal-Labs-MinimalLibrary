use std::path::{Component, Path, PathBuf};

const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Macht einen Text zu einem gültigen Windows-Datei- oder Ordnernamen.
pub fn sanitize(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let s = s.trim_end_matches(['.', ' ']).trim_start().to_string();
    let mut s = if s.is_empty() { "_".to_string() } else { s };
    let stem = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        s.insert(0, '_');
    }
    // Einzelne Segmente kurz halten (MAX_PATH-Reserve)
    if s.chars().count() > 120 {
        s = s
            .chars()
            .take(120)
            .collect::<String>()
            .trim_end_matches(['.', ' '])
            .to_string();
    }
    s
}

/// Relativer Pfad von `base_dir` zu `target`, mit `/` als Trenner (funktioniert unter Windows und Android).
/// Liegen beide auf verschiedenen Laufwerken, kommt der absolute Pfad zurück.
pub fn relative_to(base_dir: &Path, target: &Path) -> String {
    let b: Vec<Component> = base_dir.components().collect();
    let t: Vec<Component> = target.components().collect();
    let same_root = match (b.first(), t.first()) {
        (Some(Component::Prefix(x)), Some(Component::Prefix(y))) => {
            x.as_os_str().to_string_lossy().to_lowercase()
                == y.as_os_str().to_string_lossy().to_lowercase()
        }
        (Some(x), Some(y)) => x == y,
        _ => false,
    };
    if !same_root {
        return target.to_string_lossy().replace('\\', "/");
    }
    let eq = |a: &Component, c: &Component| {
        if cfg!(windows) {
            a.as_os_str().to_string_lossy().to_lowercase()
                == c.as_os_str().to_string_lossy().to_lowercase()
        } else {
            a == c
        }
    };
    let mut i = 0;
    while i < b.len() && i < t.len() && eq(&b[i], &t[i]) {
        i += 1;
    }
    let mut parts: Vec<String> = Vec::new();
    for _ in i..b.len() {
        parts.push("..".into());
    }
    for c in &t[i..] {
        parts.push(c.as_os_str().to_string_lossy().to_string());
    }
    parts.join("/")
}

/// Freien Pfad finden: "Titel.mp3" → "Titel (2).mp3" …
pub fn unique_path(p: &Path, taken: &dyn Fn(&Path) -> bool) -> PathBuf {
    if !taken(p) {
        return p.to_path_buf();
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|s| format!(".{}", s.to_string_lossy()))
        .unwrap_or_default();
    let dir = p.parent().map(Path::to_path_buf).unwrap_or_default();
    for n in 2..10_000 {
        let cand = dir.join(format!("{stem} ({n}){ext}"));
        if !taken(&cand) {
            return cand;
        }
    }
    p.to_path_buf()
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Verschieben, notfalls über Laufwerksgrenzen (kopieren + löschen).
pub fn move_file(from: &Path, to: &Path) -> std::io::Result<()> {
    if let Some(d) = to.parent() {
        std::fs::create_dir_all(d)?;
    }
    match std::fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            std::fs::copy(from, to)?;
            std::fs::remove_file(from)
        }
    }
}

/// Leere Ordner nach oben hin entfernen, höchstens bis `stop` (exklusiv).
pub fn remove_empty_dirs(mut dir: PathBuf, stop: &[PathBuf]) {
    loop {
        if stop.iter().any(|s| same_path(s, &dir)) {
            return;
        }
        let empty = std::fs::read_dir(&dir)
            .map(|mut r| r.next().is_none())
            .unwrap_or(false);
        if !empty || std::fs::remove_dir(&dir).is_err() {
            return;
        }
        match dir.parent() {
            Some(p) => dir = p.to_path_buf(),
            None => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sanitize_works() {
        assert_eq!(sanitize("AC/DC: Live?"), "AC_DC_ Live_");
        assert_eq!(sanitize("con"), "_con");
        assert_eq!(sanitize("Name. "), "Name");
    }
    #[test]
    fn relative_works() {
        assert_eq!(
            relative_to(Path::new("/m/pl"), Path::new("/m/a/b.mp3")),
            "../a/b.mp3"
        );
        assert_eq!(
            relative_to(Path::new("/m"), Path::new("/m/a/b.mp3")),
            "a/b.mp3"
        );
    }
}

/// Liegt der Ordner in einem Cloud-Laufwerk (pCloud, OneDrive, iCloud, Google Drive, Dropbox …)?
/// Dort lädt jedes vollständige Lesen die Datei herunter – solche Ordner werden nur bei Bedarf analysiert.
/// „Y:“ bzw. „Y:Ordner“ meint unter Windows den *aktuellen* Ordner auf Y – gemeint ist aber immer „Y:\\…“.
pub fn fix_drive_str(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
        if b.len() == 2 {
            return format!("{s}\\");
        }
        if b[2] != b'\\' && b[2] != b'/' {
            return format!("{}\\{}", &s[..2], &s[2..]);
        }
    }
    s.to_string()
}

/// Wie `fix_drive_str`, nur unter Windows
pub fn fix_drive(s: &str) -> String {
    if cfg!(windows) { fix_drive_str(s) } else { s.to_string() }
}

/// Vergleichsschlüssel für Pfade: unter Windows ohne Groß/klein und mit einheitlichem Trenner
pub fn norm_key(s: &str) -> String {
    if cfg!(windows) {
        s.replace('/', "\\").to_lowercase()
    } else {
        s.to_string()
    }
}

/// SQL-Bedingung „Pfad gleich ?1“ – unter Windows ohne Groß/klein
pub const PATH_EQ: &str = if cfg!(windows) { "path = ?1 COLLATE NOCASE" } else { "path = ?1" };

pub fn is_cloud(p: &Path) -> bool {
    let s = p.to_string_lossy().to_lowercase().replace('\\', "/");
    const HINTS: &[&str] = &[
        "pcloud",
        "onedrive",
        "google drive",
        "googledrive",
        "/my drive",
        "dropbox",
        "icloud",
        "/library/cloudstorage/",
        "/mobile documents/",
        "nextcloud",
        "owncloud",
        "megasync",
        "/mega/",
        "box drive",
        "synology drive",
        "tresorit",
        "/proton drive",
    ];
    if HINTS.iter().any(|h| s.contains(h)) {
        return true;
    }
    #[cfg(windows)]
    {
        if win::cloud(p) {
            return true;
        }
    }
    false
}

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use std::path::{Component, Path};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetVolumeInformationW(
            root: *const u16,
            name: *mut u16,
            name_len: u32,
            serial: *mut u32,
            max_len: *mut u32,
            flags: *mut u32,
            fs: *mut u16,
            fs_len: u32,
        ) -> i32;
    }

    /// Laufwerksname/Dateisystem verrät das Cloud-Laufwerk (z. B. „pCloud Drive“ auf P:),
    /// Platzhalter-Attribute verraten Dateien, die erst beim Lesen geladen werden (OneDrive & Co.)
    pub fn cloud(p: &Path) -> bool {
        const RECALL: u32 = 0x0040_0000 | 0x0004_0000 | 0x0010_0000 | 0x0000_1000;
        if let Ok(m) = std::fs::metadata(p) {
            if m.file_attributes() & RECALL != 0 {
                return true;
            }
        }
        let mut root = std::path::PathBuf::new();
        for c in p.components() {
            match c {
                Component::Prefix(_) | Component::RootDir => root.push(c.as_os_str()),
                _ => break,
            }
        }
        if root.as_os_str().is_empty() {
            return false;
        }
        let mut r: Vec<u16> = root.as_os_str().encode_wide().collect();
        if r.last() != Some(&(b'\\' as u16)) {
            r.push(b'\\' as u16);
        }
        r.push(0);
        let mut name = [0u16; 261];
        let mut fs = [0u16; 261];
        let (mut serial, mut max, mut flags) = (0u32, 0u32, 0u32);
        let ok = unsafe {
            GetVolumeInformationW(
                r.as_ptr(),
                name.as_mut_ptr(),
                261,
                &mut serial,
                &mut max,
                &mut flags,
                fs.as_mut_ptr(),
                261,
            )
        };
        if ok == 0 {
            return false;
        }
        let z = |b: &[u16]| {
            String::from_utf16_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(b.len())])
                .to_lowercase()
        };
        let (n, f) = (z(&name), z(&fs));
        [
            "pcloud", "onedrive", "google", "dropbox", "icloud", "mega", "box", "cloud",
        ]
        .iter()
        .any(|h| n.contains(h) || f.contains(h))
    }
}

#[cfg(test)]
mod cloud_tests {
    #[test]
    fn hints() {
        assert!(super::is_cloud(std::path::Path::new(
            "/Volumes/pCloud Drive/Musik"
        )));
        assert!(super::is_cloud(std::path::Path::new(
            "/Users/a/Library/CloudStorage/OneDrive-Privat/Musik"
        )));
        assert!(!super::is_cloud(std::path::Path::new("/home/a/Musik")));
    }
}

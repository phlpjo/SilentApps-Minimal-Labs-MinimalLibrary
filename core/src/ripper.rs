//! Link-Ripper: YouTube, SoundCloud (und alles, was yt-dlp kann) sowie Spotify-Links
//! (Titelliste von Spotify, Audio über die YouTube-Suche). Prüft vorher, was schon in der Bibliothek ist.

use crate::db::Track;
use crate::duplicates::normalize_text;
use crate::{Progress, paths, scanner, tags};
use anyhow::{Context, Result, anyhow, bail};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[cfg(windows)]
const YTDLP: &str = "yt-dlp.exe";
#[cfg(not(windows))]
const YTDLP: &str = "yt-dlp";
#[cfg(windows)]
pub const FFMPEG: &str = "ffmpeg.exe";
#[cfg(not(windows))]
pub const FFMPEG: &str = "ffmpeg";

fn cmd(program: &Path) -> Command {
    #[allow(unused_mut)]
    let mut c = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: kein Konsolenfenster
    }
    c
}

/// Programm im Werkzeug-Ordner, sonst im Systempfad
pub fn find(tools: &Path, name: &str) -> Option<PathBuf> {
    let local = tools.join(name);
    if local.exists() {
        return Some(local);
    }
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    // Mac-Apps aus dem Finder bekommen keinen Homebrew-Pfad mit
    if cfg!(target_os = "macos") {
        dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.exists())
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub ytdlp: Option<String>,
    pub ffmpeg: bool,
    pub dir: String,
}

pub fn status(tools: &Path) -> ToolStatus {
    let ytdlp = find(tools, YTDLP).and_then(|p| {
        let out = cmd(&p).arg("--version").output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|v| !v.is_empty())
    });
    ToolStatus { ytdlp, ffmpeg: find(tools, FFMPEG).is_some(), dir: tools.to_string_lossy().to_string() }
}

fn download(url: &str, progress: Progress, label: &str) -> Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).build().into();
    let mut resp = agent.get(url).call().map_err(|e| anyhow!("Download fehlgeschlagen ({label}): {e}"))?;
    let total: usize = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(400 * 1024 * 1024).reader();
    let mut buf = Vec::with_capacity(total);
    let mut chunk = vec![0u8; 256 * 1024];
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        progress(buf.len() / 1024, total / 1024, label);
    }
    Ok(buf)
}

#[cfg(unix)]
#[allow(dead_code)]
fn make_executable(p: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))?;
    Ok(())
}

/// Erste Datei mit diesem Namen aus einem ZIP holen
#[allow(dead_code)]
fn unzip_one(data: Vec<u8>, want: &str, dest: &Path) -> Result<()> {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).context("Archiv defekt")?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let name = f.name()?.replace('\\', "/");
        if name.rsplit('/').next() == Some(want) {
            let mut out = Vec::new();
            f.read_to_end(&mut out)?;
            std::fs::write(dest, out)?;
            return Ok(());
        }
    }
    bail!("{want} nicht im Archiv gefunden")
}

/// yt-dlp und ffmpeg herunterladen (Windows und macOS; unter Linux aus dem System)
pub fn install(tools: &Path, progress: Progress) -> Result<ToolStatus> {
    std::fs::create_dir_all(tools)?;
    #[cfg(target_os = "macos")]
    {
        if find(tools, YTDLP).is_none() {
            let data = download("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp_macos", progress, "yt-dlp")?;
            let p = tools.join(YTDLP);
            std::fs::write(&p, data)?;
            make_executable(&p)?;
        }
        if find(tools, FFMPEG).is_none() {
            let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "amd64" };
            let data = download(&format!("https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/ffmpeg.zip"), progress, "ffmpeg")?;
            let p = tools.join(FFMPEG);
            unzip_one(data, "ffmpeg", &p)?;
            make_executable(&p)?;
            if let Ok(data) = download(&format!("https://ffmpeg.martin-riedl.de/redirect/latest/macos/{arch}/release/ffprobe.zip"), progress, "ffprobe") {
                let p = tools.join("ffprobe");
                if unzip_one(data, "ffprobe", &p).is_ok() {
                    make_executable(&p)?;
                }
            }
        }
        return Ok(status(tools));
    }
    if !cfg!(windows) {
        let st = status(tools);
        if st.ytdlp.is_none() || !st.ffmpeg {
            bail!("Bitte yt-dlp und ffmpeg über die Paketverwaltung installieren");
        }
        return Ok(st);
    }
    if find(tools, YTDLP).is_none() {
        let data = download("https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe", progress, "yt-dlp")?;
        std::fs::write(tools.join(YTDLP), data)?;
    }
    if find(tools, FFMPEG).is_none() {
        let data = download(
            "https://github.com/yt-dlp/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip",
            progress,
            "ffmpeg",
        )?;
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(data)).context("ffmpeg-Archiv defekt")?;
        for i in 0..zip.len() {
            let mut f = zip.by_index(i)?;
            let name = f.name()?.replace('\\', "/");
            let file = name.rsplit('/').next().unwrap_or("").to_string();
            if name.contains("/bin/") && (file == "ffmpeg.exe" || file == "ffprobe.exe") {
                let mut out = Vec::new();
                f.read_to_end(&mut out)?;
                std::fs::write(tools.join(&file), out)?;
            }
        }
    }
    Ok(status(tools))
}

/// yt-dlp auf den neuesten Stand bringen (YouTube ändert sich oft)
pub fn update(tools: &Path) -> Result<String> {
    let p = find(tools, YTDLP).ok_or_else(|| anyhow!("yt-dlp ist nicht installiert"))?;
    let out = cmd(&p).arg("-U").output()?;
    Ok(String::from_utf8_lossy(&out.stdout).lines().last().unwrap_or("").to_string())
}

// ---------- Links auflösen ----------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub index: usize,
    pub artist: String,
    pub title: String,
    pub album: Option<String>,
    pub duration: Option<f64>,
    /// Was yt-dlp herunterlädt (URL oder "ytsearch1:…")
    pub source: String,
    pub site: String,
    /// Schon in der Bibliothek: (ID, Anzeige)
    pub in_library: Option<(i64, String)>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub name: String,
    pub entries: Vec<Entry>,
}

fn site_of(url: &str) -> String {
    let u = url.to_lowercase();
    if u.contains("spotify.com") {
        "Spotify"
    } else if u.contains("soundcloud.com") {
        "SoundCloud"
    } else if u.contains("youtu") {
        "YouTube"
    } else if u.contains("bandcamp.com") {
        "Bandcamp"
    } else {
        "Web"
    }
    .to_string()
}

/// "Künstler - Titel (Official Video)" → (Künstler, Titel)
pub fn split_title(raw: &str, uploader: &str) -> (String, String) {
    let mut t = raw.to_string();
    // typische Zusätze entfernen
    let junk = [
        "official music video", "official video", "official audio", "official lyric video", "lyric video", "lyrics", "official visualizer",
        "visualizer", "audio", "hd", "hq", "4k", "music video", "videoclip", "clip officiel", "official",
    ];
    loop {
        let before = t.clone();
        for (o, c) in [('(', ')'), ('[', ']')] {
            if let (Some(a), Some(b)) = (t.rfind(o), t.rfind(c)) {
                if a < b && junk.iter().any(|j| t[a + 1..b].trim().eq_ignore_ascii_case(j)) {
                    t.replace_range(a..=b, "");
                }
            }
        }
        t = t.trim().trim_end_matches(['-', '|', ' ']).to_string();
        if t == before {
            break;
        }
    }
    for sep in [" - ", " – ", " — ", " | "] {
        if let Some((a, b)) = t.split_once(sep) {
            if !a.trim().is_empty() && !b.trim().is_empty() {
                return (a.trim().to_string(), b.trim().trim_matches('"').to_string());
            }
        }
    }
    let up = uploader.trim_end_matches(" - Topic").replace("VEVO", "").trim().to_string();
    (up, t.trim_matches('"').to_string())
}

/// YouTube-Videolink ohne Playlist-/Mix-Anhang ("&list=…", "&index=…") → nur dieser eine Titel.
/// Reine Playlist-Links (/playlist?list=…) bleiben unverändert.
pub fn clean_youtube(url: &str) -> String {
    let u = url.trim();
    let lower = u.to_lowercase();
    if !(lower.contains("youtube.com") || lower.contains("youtu.be")) || lower.contains("/playlist") {
        return u.to_string();
    }
    let id = if let Some(i) = lower.find("youtu.be/") {
        u[i + 9..].split(['?', '&', '/', '#']).next().map(str::to_string)
    } else if let Some(i) = lower.find("/shorts/") {
        u[i + 8..].split(['?', '&', '/', '#']).next().map(str::to_string)
    } else {
        u.split(['?', '&', '#']).find_map(|p| p.strip_prefix("v=").map(str::to_string))
    };
    match id.filter(|i| i.len() >= 6) {
        Some(id) => format!("https://www.youtube.com/watch?v={id}"),
        None => u.to_string(),
    }
}

fn ytdlp_json(tools: &Path, url: &str, flat: bool) -> Result<Value> {
    let p = find(tools, YTDLP).ok_or_else(|| anyhow!("yt-dlp ist nicht installiert"))?;
    let mut c = cmd(&p);
    c.args(["-J", "--no-warnings", "--ignore-errors", "--no-playlist"]);
    if flat {
        c.arg("--flat-playlist");
    }
    let out = c.arg(url).output()?;
    if out.stdout.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("{}", err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or("Link konnte nicht gelesen werden"));
    }
    Ok(serde_json::from_slice(&out.stdout)?)
}

fn entry_from(v: &Value, index: usize, site: &str) -> Option<Entry> {
    let title = v["title"].as_str().unwrap_or("").to_string();
    let url = clean_youtube(v["webpage_url"].as_str().or(v["url"].as_str())?);
    if title.is_empty() || title == "[Private video]" || title == "[Deleted video]" {
        return None;
    }
    let uploader = v["uploader"].as_str().or(v["channel"].as_str()).unwrap_or("");
    let (artist, track) = match (v["artist"].as_str().or(v["creator"].as_str()), v["track"].as_str()) {
        (Some(a), Some(t)) => (a.split(',').next().unwrap_or(a).trim().to_string(), t.to_string()),
        _ => split_title(&title, uploader),
    };
    Some(Entry {
        index,
        artist,
        title: track,
        album: v["album"].as_str().map(str::to_string),
        duration: v["duration"].as_f64(),
        source: url,
        site: site.to_string(),
        in_library: None,
    })
}

/// Spotify-Titelliste über die öffentliche Einbettungsseite (ohne Konto/Schlüssel)
fn spotify(url: &str) -> Result<Resolved> {
    let clean = url.split('?').next().unwrap_or(url);
    let mut parts = clean.trim_end_matches('/').rsplit('/');
    let id = parts.next().unwrap_or("");
    let kind = parts.next().unwrap_or("");
    if !["track", "album", "playlist"].contains(&kind) {
        bail!("Spotify: nur Titel-, Album- und Playlist-Links");
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(20))).build().into();
    let html = agent
        .get(&format!("https://open.spotify.com/embed/{kind}/{id}"))
        .header("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130 Safari/537.36")
        .call()
        .map_err(|e| anyhow!("Spotify nicht erreichbar: {e}"))?
        .body_mut()
        .read_to_string()?;
    let start = html.find("id=\"__NEXT_DATA__\"").context("Spotify-Seite unbekannt")?;
    let json_start = html[start..].find('>').map(|i| start + i + 1).context("Spotify-Seite unbekannt")?;
    let json_end = html[json_start..].find("</script>").map(|i| json_start + i).context("Spotify-Seite unbekannt")?;
    let v: Value = serde_json::from_str(&html[json_start..json_end])?;
    let entity = &v["props"]["pageProps"]["state"]["data"]["entity"];
    if entity.is_null() {
        bail!("Spotify: Inhalt nicht gefunden (privat oder regional gesperrt?)");
    }
    let name = entity["name"].as_str().or(entity["title"].as_str()).unwrap_or("Spotify").to_string();
    let album = if kind == "album" { Some(name.clone()) } else { None };
    let mut entries = Vec::new();
    let mk = |i: usize, title: &str, artist: &str, dur_ms: Option<f64>, album: Option<String>| Entry {
        index: i,
        artist: artist.to_string(),
        title: title.to_string(),
        album,
        duration: dur_ms.map(|d| d / 1000.0),
        source: format!("ytsearch1:{artist} - {title}"),
        site: "Spotify".into(),
        in_library: None,
    };
    if let Some(list) = entity["trackList"].as_array() {
        for (i, t) in list.iter().enumerate() {
            let title = t["title"].as_str().unwrap_or("");
            let artist = t["subtitle"].as_str().unwrap_or("").split(',').next().unwrap_or("").trim().to_string();
            if !title.is_empty() {
                entries.push(mk(i, title, &artist, t["duration"].as_f64(), album.clone()));
            }
        }
    } else {
        let artist = entity["artists"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|a| a["name"].as_str())
            .or(entity["subtitle"].as_str())
            .unwrap_or("")
            .to_string();
        entries.push(mk(0, &name, &artist, entity["duration"].as_f64(), None));
    }
    Ok(Resolved { name, entries })
}

pub fn resolve(tools: &Path, url: &str) -> Result<Resolved> {
    let url = url.trim();
    if !url.starts_with("http") {
        bail!("Bitte einen vollständigen Link einfügen (https://…)");
    }
    let site = site_of(url);
    if site == "Spotify" {
        return spotify(url);
    }
    let url = &clean_youtube(url);
    let v = ytdlp_json(tools, url, true)?;
    if v["_type"] == "playlist" {
        let name = v["title"].as_str().unwrap_or("Playlist").to_string();
        let mut entries: Vec<Entry> =
            v["entries"].as_array().into_iter().flatten().enumerate().filter_map(|(i, e)| entry_from(e, i, &site)).collect();
        // SoundCloud-Sets liefern flach oft keine Titel → vollständig lesen
        if entries.is_empty() && v["entries"].as_array().is_some_and(|a| !a.is_empty()) {
            let full = ytdlp_json(tools, url, false)?;
            entries = full["entries"].as_array().into_iter().flatten().enumerate().filter_map(|(i, e)| entry_from(e, i, &site)).collect();
        }
        Ok(Resolved { name, entries })
    } else {
        let e = entry_from(&v, 0, &site).ok_or_else(|| anyhow!("Kein Titel gefunden"))?;
        Ok(Resolved { name: e.title.clone(), entries: vec![e] })
    }
}

// ---------- Abgleich mit der Bibliothek ----------

pub fn mark_known(entries: &mut [Entry], library: &[Track]) {
    let idx: Vec<(String, String, f64, &Track)> = library
        .iter()
        .filter_map(|t| {
            let title = t.title.clone().unwrap_or_else(|| Path::new(&t.path).file_stem().unwrap_or_default().to_string_lossy().to_string());
            let nt = normalize_text(&title);
            if nt.is_empty() { None } else { Some((normalize_text(t.artist.as_deref().unwrap_or("")), nt, t.duration, t)) }
        })
        .collect();
    for e in entries.iter_mut() {
        let (na, nt) = (normalize_text(&e.artist), normalize_text(&e.title));
        if nt.is_empty() {
            continue;
        }
        let hit = idx.iter().find(|(a, t, dur, _)| {
            let title_ok = *t == nt || (nt.len() >= 5 && t.len() >= 5 && (t.contains(&nt) || nt.contains(t.as_str())));
            let artist_ok = na.is_empty() || a.is_empty() || *a == na || a.contains(&na) || na.contains(a.as_str())
                // "Künstler - Titel" ohne Künstler-Tag: Künstler steckt im Dateinamen/Titel
                || t.contains(&na);
            let dur_ok = match e.duration {
                Some(d) if *dur > 0.0 && d > 0.0 => (d - dur).abs() <= 8.0,
                _ => true,
            };
            title_ok && artist_ok && dur_ok
        });
        e.in_library = hit.map(|(_, _, _, t)| {
            (t.id, format!("{} – {}", t.artist.clone().unwrap_or_default(), t.title.clone().unwrap_or_default()).trim_matches([' ', '–']).to_string())
        });
    }
}

// ---------- Herunterladen ----------

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Format {
    Mp3,
    Original,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RipEvent {
    pub index: usize,
    /// "download", "convert", "done", "error"
    pub stage: String,
    pub percent: f64,
    pub message: String,
    pub track_id: Option<i64>,
}

/// Lädt einen Eintrag, setzt saubere Tags und nimmt die Datei in die Bibliothek auf.
/// Wie `rip_once`, aber bei vorübergehender YouTube-Sperre einmal nach kurzer Pause erneut.
pub fn rip_one(
    tools: &Path,
    conn: &Mutex<Connection>,
    e: &Entry,
    dir: &Path,
    format: Format,
    cancel: &AtomicBool,
    on: &(dyn Fn(RipEvent) + Sync),
) -> Result<i64> {
    match rip_once(tools, conn, e, dir, format, cancel, on) {
        Err(err) if is_block(&err.to_string()) && !cancel.load(Ordering::Relaxed) => {
            on(RipEvent { index: e.index, stage: "download".into(), percent: 0.0, message: "YouTube bremst – neuer Versuch in 8 s".into(), track_id: None });
            std::thread::sleep(Duration::from_secs(8));
            rip_once(tools, conn, e, dir, format, cancel, on).map_err(|e2| {
                if is_block(&e2.to_string()) {
                    anyhow!("YouTube blockt gerade (zu viele Anfragen). Bitte yt-dlp aktualisieren, ein paar Minuten warten und erneut versuchen.")
                } else {
                    e2
                }
            })
        }
        r => r,
    }
}

fn is_block(msg: &str) -> bool {
    let m = msg.to_lowercase();
    m.contains("page needs to be reloaded") || m.contains("not a bot") || m.contains("http error 403") || m.contains("http error 429")
}

fn rip_once(
    tools: &Path,
    conn: &Mutex<Connection>,
    e: &Entry,
    dir: &Path,
    format: Format,
    cancel: &AtomicBool,
    on: &(dyn Fn(RipEvent) + Sync),
) -> Result<i64> {
    let p = find(tools, YTDLP).ok_or_else(|| anyhow!("yt-dlp ist nicht installiert"))?;
    std::fs::create_dir_all(dir)?;
    let base = if e.artist.is_empty() { e.title.clone() } else { format!("{} - {}", e.artist, e.title) };
    let stem = paths::sanitize(&base);
    let out_tpl = dir.join(format!("{stem}.%(ext)s"));
    let mut c = cmd(&p);
    c.args(["--no-playlist", "--newline", "--no-warnings", "--progress", "--sleep-requests", "1", "-f", "bestaudio/best", "-x"]);
    if format == Format::Mp3 {
        c.args(["--audio-format", "mp3", "--audio-quality", "320K"]);
    }
    c.args(["--embed-thumbnail", "--convert-thumbnails", "jpg", "--embed-metadata", "--no-overwrites"]);
    c.args(["--progress-template", "download:[ML] %(progress._percent_str)s"]);
    c.args(["--print", "after_move:[MLFILE]%(filepath)s"]);
    if let Some(ff) = find(tools, FFMPEG) {
        c.arg("--ffmpeg-location").arg(ff.parent().unwrap_or(tools));
    }
    c.arg("-o").arg(&out_tpl).arg(&e.source);
    c.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().context("yt-dlp konnte nicht gestartet werden")?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let err_buf = Arc::new(Mutex::new(String::new()));
    let eb = err_buf.clone();
    let err_thread = std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
            let mut b = eb.lock().unwrap();
            b.push_str(&line);
            b.push('\n');
        }
    });
    let mut file: Option<PathBuf> = None;
    let ev = |stage: &str, percent: f64, message: &str| on(RipEvent { index: e.index, stage: stage.into(), percent, message: message.into(), track_id: None });
    for line in BufReader::new(stdout).lines().map_while(|l| l.ok()) {
        if cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            bail!("Abgebrochen");
        }
        if let Some(rest) = line.strip_prefix("[ML]") {
            let pct: f64 = rest.trim().trim_end_matches('%').trim().parse().unwrap_or(0.0);
            ev("download", pct, "lädt");
        } else if let Some(f) = line.strip_prefix("[MLFILE]") {
            file = Some(PathBuf::from(f.trim()));
        } else if line.contains("[ExtractAudio]") || line.contains("[EmbedThumbnail]") || line.contains("[Metadata]") {
            ev("convert", 100.0, "wandelt um");
        }
    }
    let status = child.wait()?;
    let _ = err_thread.join();
    let Some(file) = file.filter(|f| f.exists()) else {
        let err = err_buf.lock().unwrap().clone();
        let msg = err.lines().rev().find(|l| l.contains("ERROR")).unwrap_or(if status.success() { "Datei nicht gefunden" } else { "Download fehlgeschlagen" });
        bail!("{}", msg.trim_start_matches("ERROR: "));
    };
    // Saubere Tags aus dem Link (statt Kanalname/Videotitel)
    let u = tags::TagUpdate {
        title: Some(e.title.clone()),
        artist: if e.artist.is_empty() { None } else { Some(e.artist.clone()) },
        album: e.album.clone(),
        comment: Some(String::new()),
        ..Default::default()
    };
    let _ = tags::write(&file, &u);
    let ids = scanner::add_files(conn, &[file])?;
    ids.first().copied().ok_or_else(|| anyhow!("Datei konnte nicht aufgenommen werden"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn split() {
        assert_eq!(split_title("Daft Punk - One More Time (Official Video)", "DaftPunkVEVO"), ("Daft Punk".into(), "One More Time".into()));
        assert_eq!(split_title("Strobe [Official Audio]", "deadmau5 - Topic"), ("deadmau5".into(), "Strobe".into()));
        assert_eq!(split_title("Artist – Song (Extended Mix)", "x"), ("Artist".into(), "Song (Extended Mix)".into()));
        assert_eq!(clean_youtube("https://www.youtube.com/watch?v=abcdefgh123&list=RDabc&index=3"), "https://www.youtube.com/watch?v=abcdefgh123");
        assert_eq!(clean_youtube("https://youtu.be/abcdefgh123?si=x"), "https://www.youtube.com/watch?v=abcdefgh123");
        assert_eq!(clean_youtube("https://music.youtube.com/watch?v=abcdefgh123&list=OLAK"), "https://www.youtube.com/watch?v=abcdefgh123");
        assert_eq!(clean_youtube("https://www.youtube.com/playlist?list=PL123"), "https://www.youtube.com/playlist?list=PL123");
    }
    #[test]
    fn known() {
        let lib = vec![Track { id: 7, title: Some("One More Time".into()), artist: Some("Daft Punk".into()), duration: 320.0, ..Default::default() }];
        let mut e = vec![
            Entry { index: 0, artist: "Daft Punk".into(), title: "One More Time".into(), album: None, duration: Some(322.0), source: String::new(), site: String::new(), in_library: None },
            Entry { index: 1, artist: "Daft Punk".into(), title: "Aerodynamic".into(), album: None, duration: None, source: String::new(), site: String::new(), in_library: None },
        ];
        mark_known(&mut e, &lib);
        assert_eq!(e[0].in_library.as_ref().map(|x| x.0), Some(7));
        assert!(e[1].in_library.is_none());
    }
}

//! Abspielen im Heimnetz: Sonos (inkl. Gruppen) und DLNA/UPnP-Renderer.
//! Die App stellt die Datei über einen kleinen HTTP-Server bereit, das Gerät holt sie sich.

use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    /// "sonos" oder "dlna"
    pub kind: String,
    pub model: String,
    #[serde(skip)]
    av_control: String,
    #[serde(skip)]
    rc_control: Option<String>,
    #[serde(skip)]
    group_volume: bool,
    #[serde(skip)]
    local_ip: Ipv4Addr,
    /// Sonos: Basis-URLs aller Lautsprecher der Gruppe (für Klang-Einstellungen)
    #[serde(skip)]
    members: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// PLAYING, PAUSED_PLAYBACK, STOPPED, TRANSITIONING, NO_MEDIA_PRESENT
    pub state: String,
    pub position: f64,
    pub duration: f64,
    /// 0..100, -1 = unbekannt
    pub volume: i32,
}

/// Infos zum Titel für die Anzeige am Gerät
pub struct Meta<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub duration: f64,
    pub cover: Option<Vec<u8>>,
}

fn devices() -> &'static Mutex<HashMap<String, Device>> {
    static D: OnceLock<Mutex<HashMap<String, Device>>> = OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}

// ---------- Hilfen ----------

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

fn hms(sec: f64) -> String {
    let s = sec.max(0.0).round() as u64;
    format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn parse_hms(s: &str) -> f64 {
    let s = s.split('.').next().unwrap_or("");
    let p: Vec<f64> = s.split(':').filter_map(|x| x.trim().parse().ok()).collect();
    match p.as_slice() {
        [h, m, sec] => h * 3600.0 + m * 60.0 + sec,
        [m, sec] => m * 60.0 + sec,
        _ => 0.0,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(6)))
        .http_status_as_error(false)
        .proxy(None) // Heimnetz: nie über einen Proxy
        .build()
        .into()
}

fn base_of(url: &str) -> String {
    // http://192.168.1.20:1400/xml/x.xml → http://192.168.1.20:1400
    let after = url.find("://").map(|i| i + 3).unwrap_or(0);
    match url[after..].find('/') {
        Some(i) => url[..after + i].to_string(),
        None => url.to_string(),
    }
}

fn host_ip(url: &str) -> Option<Ipv4Addr> {
    let after = url.find("://")? + 3;
    let rest = &url[after..];
    let host = rest.split(['/', ':']).next()?;
    host.parse().ok()
}

fn join(base: &str, path: &str) -> String {
    if path.starts_with("http") { path.to_string() } else if path.starts_with('/') { format!("{base}{path}") } else { format!("{base}/{path}") }
}

/// Lokale IPv4-Adressen mit Netzmaske (ohne Loopback)
fn local_ifaces() -> Vec<(Ipv4Addr, Ipv4Addr)> {
    if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|i| match i.addr {
            if_addrs::IfAddr::V4(v) if !v.ip.is_loopback() && !v.ip.is_link_local() => Some((v.ip, v.netmask)),
            _ => None,
        })
        .collect()
}

/// Eigene Adresse, unter der das Gerät uns erreicht (gleiches Subnetz)
fn local_ip_for(dev: Ipv4Addr) -> Ipv4Addr {
    let ifs = local_ifaces();
    let same = |(ip, mask): &(Ipv4Addr, Ipv4Addr)| (u32::from(*ip) & u32::from(*mask)) == (u32::from(dev) & u32::from(*mask));
    ifs.iter().find(|i| same(i)).or(ifs.first()).map(|i| i.0).unwrap_or(Ipv4Addr::UNSPECIFIED)
}

// ---------- SOAP ----------

fn soap(control: &str, service: &str, action: &str, args: &[(&str, String)]) -> Result<String> {
    let body_args: String = args.iter().map(|(k, v)| format!("<{k}>{}</{k}>", xml_escape(v))).collect();
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/"><s:Body><u:{action} xmlns:u="{service}">{body_args}</u:{action}></s:Body></s:Envelope>"#
    );
    let mut resp = agent()
        .post(control)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPACTION", &format!("\"{service}#{action}\""))
        .send(body.as_bytes())
        .map_err(|e| anyhow!("Gerät nicht erreichbar: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status >= 400 {
        let code = tag(&text, "errorCode").unwrap_or_default();
        let desc = tag(&text, "errorDescription").unwrap_or_default();
        bail!("{action} abgelehnt (HTTP {status}{}{})", if code.is_empty() { "" } else { ", Fehler " }, if desc.is_empty() { code } else { format!("{code} {desc}") });
    }
    Ok(text)
}

/// Inhalt des ersten Elements mit diesem lokalen Namen (unescaped)
fn tag(xml: &str, name: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants().find(|n| n.tag_name().name() == name).map(|n| n.text().unwrap_or("").to_string())
}

const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
const RC: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
const GRC: &str = "urn:schemas-sonos-com:service:GroupRenderingControl:1";
const ZGT: &str = "urn:schemas-upnp-org:service:ZoneGroupTopology:1";

// ---------- Suche ----------

fn ssdp_search(timeout: Duration) -> Vec<String> {
    let targets = ["urn:schemas-upnp-org:device:MediaRenderer:1", "urn:schemas-upnp-org:device:ZonePlayer:1"];
    let mut locations: Vec<String> = Vec::new();
    let mut sockets = Vec::new();
    let mut ips: Vec<Ipv4Addr> = local_ifaces().into_iter().map(|i| i.0).collect();
    if ips.is_empty() {
        ips.push(Ipv4Addr::UNSPECIFIED);
    }
    for ip in ips {
        let Ok(sock) = UdpSocket::bind(SocketAddrV4::new(ip, 0)) else { continue };
        let _ = sock.set_multicast_ttl_v4(2);
        let _ = sock.set_read_timeout(Some(Duration::from_millis(200)));
        for st in targets {
            let msg = format!(
                "M-SEARCH * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nMAN: \"ssdp:discover\"\r\nMX: 2\r\nST: {st}\r\nUSER-AGENT: Windows/10 UPnP/1.1 MinimalLibrary/1.0\r\n\r\n"
            );
            for _ in 0..2 {
                let _ = sock.send_to(msg.as_bytes(), "239.255.255.250:1900");
            }
        }
        sockets.push(sock);
    }
    let end = Instant::now() + timeout;
    let mut buf = [0u8; 4096];
    while Instant::now() < end {
        for sock in &sockets {
            if let Ok((n, _)) = sock.recv_from(&mut buf) {
                let text = String::from_utf8_lossy(&buf[..n]);
                for line in text.lines() {
                    if let Some((k, v)) = line.split_once(':') {
                        if k.trim().eq_ignore_ascii_case("location") {
                            let v = v.trim().to_string();
                            if !locations.contains(&v) {
                                locations.push(v);
                            }
                        }
                    }
                }
            }
        }
    }
    locations
}

struct Desc {
    udn: String,
    name: String,
    model: String,
    manufacturer: String,
    services: HashMap<String, String>, // serviceType → controlURL
}

fn describe(location: &str) -> Result<Desc> {
    let xml = agent().get(location).call()?.body_mut().read_to_string()?;
    let doc = roxmltree::Document::parse(&xml)?;
    let root_dev = doc.descendants().find(|n| n.tag_name().name() == "device").context("kein device")?;
    let child = |n: roxmltree::Node, name: &str| {
        n.children().find(|c| c.tag_name().name() == name).and_then(|c| c.text()).unwrap_or("").to_string()
    };
    let mut services = HashMap::new();
    for s in doc.descendants().filter(|n| n.tag_name().name() == "service") {
        services.entry(child(s, "serviceType")).or_insert_with(|| child(s, "controlURL"));
    }
    Ok(Desc {
        udn: child(root_dev, "UDN"),
        name: child(root_dev, "friendlyName"),
        model: child(root_dev, "modelName"),
        manufacturer: child(root_dev, "manufacturer"),
        services,
    })
}

/// Sonos: Gruppen auslesen → ein Eintrag pro Gruppe (Koordinator steuert)
fn sonos_groups(any_base: &str) -> Result<Vec<(String, String, String, Vec<String>)>> {
    let resp = soap(&format!("{any_base}/ZoneGroupTopology/Control"), ZGT, "GetZoneGroupState", &[])?;
    let state = tag(&resp, "ZoneGroupState").context("kein ZoneGroupState")?;
    let doc = roxmltree::Document::parse(&state)?;
    let mut out = Vec::new(); // (Koordinator-UUID, Name, Basis-URL)
    for g in doc.descendants().filter(|n| n.tag_name().name() == "ZoneGroup") {
        let coord = g.attribute("Coordinator").unwrap_or("").to_string();
        let mut names: Vec<String> = Vec::new();
        let mut base = String::new();
        let mut members: Vec<String> = Vec::new();
        for m in g.children().filter(|n| n.tag_name().name() == "ZoneGroupMember") {
            if m.attribute("Invisible") == Some("1") {
                continue; // Sub, Surround-Boxen
            }
            let zn = m.attribute("ZoneName").unwrap_or("").to_string();
            if !zn.is_empty() && !names.contains(&zn) {
                names.push(zn);
            }
            let mb = base_of(m.attribute("Location").unwrap_or(""));
            if !mb.is_empty() && !members.contains(&mb) {
                members.push(mb);
            }
            if m.attribute("UUID") == Some(coord.as_str()) {
                base = base_of(m.attribute("Location").unwrap_or(""));
            }
        }
        if !coord.is_empty() && !base.is_empty() && !names.is_empty() {
            out.push((coord, names.join(" + "), base, members));
        }
    }
    Ok(out)
}

/// Geräte im Heimnetz suchen (dauert ca. 2,5 s)
pub fn discover() -> Result<Vec<Device>> {
    let locs = ssdp_search(Duration::from_millis(2500));
    let mut found: Vec<Device> = Vec::new();
    let mut sonos_base: Option<String> = None;
    for loc in locs {
        let Ok(d) = describe(&loc) else { continue };
        let base = base_of(&loc);
        let is_sonos = d.manufacturer.to_lowercase().contains("sonos") || d.services.contains_key(ZGT);
        if is_sonos {
            sonos_base.get_or_insert(base);
            continue;
        }
        let Some(av) = d.services.get(AVT) else { continue };
        let ip = host_ip(&base).unwrap_or(Ipv4Addr::UNSPECIFIED);
        if found.iter().any(|x| x.id == d.udn) {
            continue;
        }
        found.push(Device {
            id: d.udn.clone(),
            name: if d.name.is_empty() { d.model.clone() } else { d.name.clone() },
            kind: "dlna".into(),
            model: d.model.clone(),
            av_control: join(&base, av),
            rc_control: d.services.get(RC).map(|c| join(&base, c)),
            group_volume: false,
            local_ip: local_ip_for(ip),
            members: vec![],
        });
    }
    if let Some(b) = sonos_base {
        if let Ok(groups) = sonos_groups(&b) {
            for (coord, name, base, members) in groups {
                let ip = host_ip(&base).unwrap_or(Ipv4Addr::UNSPECIFIED);
                found.push(Device {
                    id: format!("uuid:{coord}"),
                    name,
                    kind: "sonos".into(),
                    model: "Sonos".into(),
                    av_control: format!("{base}/MediaRenderer/AVTransport/Control"),
                    rc_control: Some(format!("{base}/MediaRenderer/GroupRenderingControl/Control")),
                    group_volume: true,
                    local_ip: local_ip_for(ip),
                    members,
                });
            }
        }
    }
    found.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    let mut map = devices().lock().unwrap();
    for d in &found {
        map.insert(d.id.clone(), d.clone());
    }
    Ok(found)
}

fn device(id: &str) -> Result<Device> {
    devices().lock().unwrap().get(id).cloned().ok_or_else(|| anyhow!("Gerät nicht mehr gefunden – bitte neu suchen"))
}

// ---------- Datei-Server ----------

struct Server {
    port: u16,
    files: Mutex<HashMap<String, PathBuf>>,
    covers: Mutex<HashMap<String, Vec<u8>>>,
}

fn server() -> Result<&'static Server> {
    static S: OnceLock<Server> = OnceLock::new();
    if let Some(s) = S.get() {
        return Ok(s);
    }
    let http = tiny_http::Server::http("0.0.0.0:0").map_err(|e| anyhow!("Server: {e}"))?;
    let port = http.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    let _ = S.set(Server { port, files: Mutex::new(HashMap::new()), covers: Mutex::new(HashMap::new()) });
    let srv = S.get().unwrap();
    std::thread::spawn(move || {
        for req in http.incoming_requests() {
            std::thread::spawn(move || {
                let _ = serve(srv, req);
            });
        }
    });
    Ok(srv)
}

pub fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("m4a" | "mp4" | "aac") => "audio/mp4",
        Some("ogg" | "oga") => "audio/ogg",
        Some("opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("aif" | "aiff") => "audio/aiff",
        _ => "application/octet-stream",
    }
}

fn serve(srv: &Server, req: tiny_http::Request) -> std::io::Result<()> {
    let url = req.url().to_string();
    let key = url.trim_start_matches('/').split(['.', '?']).next().unwrap_or("").to_string();
    let header = |n: &str, v: &str| tiny_http::Header::from_bytes(n.as_bytes(), v.as_bytes()).unwrap();
    if let Some(c) = srv.covers.lock().unwrap().get(&key).cloned() {
        return req.respond(tiny_http::Response::from_data(c).with_header(header("Content-Type", "image/jpeg")));
    }
    let Some(path) = srv.files.lock().unwrap().get(&key).cloned() else {
        return req.respond(tiny_http::Response::empty(404));
    };
    let mut f = std::fs::File::open(&path)?;
    let len = f.metadata()?.len();
    let range = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Range"))
        .and_then(|h| h.value.as_str().strip_prefix("bytes=").map(str::to_string));
    let (start, end, status) = match range.and_then(|r| {
        let (a, b) = r.split_once('-')?;
        let a: u64 = a.trim().parse().ok()?;
        let b: u64 = b.trim().parse().unwrap_or(len.saturating_sub(1));
        Some((a, b.min(len.saturating_sub(1))))
    }) {
        Some((a, b)) if a <= b && a < len => (a, b, 206),
        _ => (0, len.saturating_sub(1), 200),
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    f.seek(SeekFrom::Start(start))?;
    let mut headers = vec![
        header("Content-Type", mime(&path)),
        header("Accept-Ranges", "bytes"),
        header("transferMode.dlna.org", "Streaming"),
        header("contentFeatures.dlna.org", "DLNA.ORG_OP=01;DLNA.ORG_FLAGS=01700000000000000000000000000000"),
    ];
    if status == 206 {
        headers.push(header("Content-Range", &format!("bytes {start}-{end}/{len}")));
    }
    let is_head = *req.method() == tiny_http::Method::Head;
    let body: Box<dyn Read + Send> = if is_head { Box::new(std::io::empty()) } else { Box::new(f.take(n)) };
    req.respond(tiny_http::Response::new(tiny_http::StatusCode(status), headers, body, Some(n as usize), None))
}

fn token() -> String {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    format!("{:016x}", h.finish())
}

// ---------- Steuerung ----------

/// SOAP-Aufruf mit Wiederholung (Sonos lehnt kurz nach Stop/URI-Wechsel gern mit 701 „Transition“ ab)
fn soap_retry(control: &str, service: &str, action: &str, args: &[(&str, String)], tries: usize) -> Result<String> {
    let mut last = None;
    for i in 0..tries {
        match soap(control, service, action, args) {
            Ok(r) => return Ok(r),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(300 + 300 * i as u64));
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("{action} fehlgeschlagen")))
}

pub fn play(device_id: &str, path: &Path, meta: Meta, start: f64) -> Result<()> {
    // Schnelles Weiterklicken: Aufträge nacheinander abarbeiten, überholte überspringen.
    // (Sonst liest ein zweiter Start „stumm“ als Ausgangszustand und der Lautsprecher bleibt stumm.)
    static LOCK: Mutex<()> = Mutex::new(());
    static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let my = GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if GEN.load(std::sync::atomic::Ordering::SeqCst) != my {
        return Ok(()); // inzwischen wurde ein anderer Titel gewählt
    }
    let d = device(device_id)?;
    let srv = server()?;
    let key = token();
    srv.files.lock().unwrap().insert(key.clone(), path.to_path_buf());
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let url = format!("http://{}:{}/{key}.{ext}", d.local_ip, srv.port);
    let art = meta.cover.map(|c| {
        let ck = format!("c{key}");
        srv.covers.lock().unwrap().insert(ck.clone(), c);
        format!("http://{}:{}/{ck}.jpg", d.local_ip, srv.port)
    });
    let mime = mime(path);
    let didl = format!(
        r#"<DIDL-Lite xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:upnp="urn:schemas-upnp-org:metadata-1-0/upnp/" xmlns:r="urn:schemas-rinconnetworks-com:metadata-1-0/" xmlns="urn:schemas-upnp-org:metadata-1-0/DIDL-Lite/"><item id="1" parentID="0" restricted="1"><dc:title>{}</dc:title><dc:creator>{}</dc:creator><upnp:artist>{}</upnp:artist><upnp:album>{}</upnp:album>{}<upnp:class>object.item.audioItem.musicTrack</upnp:class><res protocolInfo="http-get:*:{mime}:*" duration="{}">{}</res></item></DIDL-Lite>"#,
        xml_escape(meta.title),
        xml_escape(meta.artist),
        xml_escape(meta.artist),
        xml_escape(meta.album),
        art.map(|a| format!("<upnp:albumArtURI>{}</upnp:albumArtURI>", xml_escape(&a))).unwrap_or_default(),
        hms(meta.duration),
        xml_escape(&url),
    );
    let sonos = d.kind == "sonos";
    // Nur beim Einstieg mitten im Titel stumm schalten (sonst hört man kurz den Anfang).
    // Beim normalen Start nicht: jedes Stumm/Laut kostet Zeit und kann selbst knacken.
    let mute_now = start > 1.0 && !get_mute(&d).unwrap_or(true);
    if mute_now {
        let _ = set_mute(&d, true);
    }
    let result = (|| -> Result<()> {
        // Sonos nimmt eine neue Adresse auch während der Wiedergabe an – ein Stop vorher kostet nur Zeit
        // und lässt den Puffer leer anlaufen. Andere Geräte (DLNA) brauchen ihn oft.
        if !sonos {
            let _ = soap(&d.av_control, AVT, "Stop", &[("InstanceID", "0".into())]);
        }
        soap_retry(&d.av_control, AVT, "SetAVTransportURI", &[("InstanceID", "0".into()), ("CurrentURI", url), ("CurrentURIMetaData", didl)], 3)?;
        let play = || soap_retry(&d.av_control, AVT, "Play", &[("InstanceID", "0".into()), ("Speed", "1".into())], 4);
        play()?;
        if !wait_state(&d, "PLAYING", Duration::from_secs(6)) {
            // hängt in TRANSITIONING/STOPPED → noch einmal anstoßen
            play()?;
            if !wait_state(&d, "PLAYING", Duration::from_secs(6)) {
                bail!("{} startet die Wiedergabe nicht – ist der Lautsprecher erreichbar und die Firewall offen?", d.name);
            }
        }
        if start > 1.0 {
            seek_dev(&d, start)?;
            // erst laut schalten, wenn das Gerät an der neuen Stelle wirklich weiterläuft
            wait_state(&d, "PLAYING", Duration::from_secs(4));
            wait_position(&d, start + 0.5, Duration::from_millis(2500));
        }
        Ok(())
    })();
    if mute_now {
        let _ = set_mute(&d, false);
    }
    result?;
    *active().lock().unwrap() = Some(device_id.to_string());
    Ok(())
}

fn active() -> &'static Mutex<Option<String>> {
    static A: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(None))
}

/// Gerät, an das gerade gestreamt wird (zum Stoppen beim Beenden)
pub fn active_device() -> Option<String> {
    active().lock().unwrap().clone()
}

fn transport_state(d: &Device) -> Option<String> {
    soap(&d.av_control, AVT, "GetTransportInfo", &[("InstanceID", "0".into())]).ok().and_then(|x| tag(&x, "CurrentTransportState"))
}

/// Warten, bis die Abspielposition eine Stelle erreicht hat (Puffer gefüllt, läuft sauber)
fn wait_position(d: &Device, at: f64, max: Duration) -> bool {
    let end = Instant::now() + max;
    while Instant::now() < end {
        let pos = soap(&d.av_control, AVT, "GetPositionInfo", &[("InstanceID", "0".into())])
            .ok()
            .and_then(|x| tag(&x, "RelTime"))
            .map(|t| parse_hms(&t))
            .unwrap_or(0.0);
        if pos >= at {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn wait_state(d: &Device, want: &str, max: Duration) -> bool {
    let end = Instant::now() + max;
    while Instant::now() < end {
        if transport_state(d).as_deref() == Some(want) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    false
}

fn get_mute(d: &Device) -> Result<bool> {
    let rc = d.rc_control.as_ref().ok_or_else(|| anyhow!("keine Lautstärke"))?;
    let r = if d.group_volume {
        soap(rc, GRC, "GetGroupMute", &[("InstanceID", "0".into())])?
    } else {
        soap(rc, RC, "GetMute", &[("InstanceID", "0".into()), ("Channel", "Master".into())])?
    };
    Ok(tag(&r, "CurrentMute").as_deref() == Some("1"))
}

fn set_mute(d: &Device, mute: bool) -> Result<()> {
    let rc = d.rc_control.as_ref().ok_or_else(|| anyhow!("keine Lautstärke"))?;
    let v = if mute { "1" } else { "0" }.to_string();
    if d.group_volume {
        soap(rc, GRC, "SetGroupMute", &[("InstanceID", "0".into()), ("DesiredMute", v)])?;
    } else {
        soap(rc, RC, "SetMute", &[("InstanceID", "0".into()), ("Channel", "Master".into()), ("DesiredMute", v)])?;
    }
    Ok(())
}

/// Springen – Geräte lehnen Seek ab, solange sie noch puffern ("Transition not available") → mehrfach versuchen
fn seek_dev(d: &Device, sec: f64) -> Result<()> {
    let mut last = anyhow!("Springen nicht möglich");
    for _ in 0..12 {
        match soap(&d.av_control, AVT, "Seek", &[("InstanceID", "0".into()), ("Unit", "REL_TIME".into()), ("Target", hms(sec))]) {
            Ok(_) => return Ok(()),
            Err(e) => {
                last = e;
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }
    Err(last)
}

pub fn pause(device_id: &str) -> Result<()> {
    let d = device(device_id)?;
    soap(&d.av_control, AVT, "Pause", &[("InstanceID", "0".into())]).map(|_| ())
}

pub fn resume(device_id: &str) -> Result<()> {
    let d = device(device_id)?;
    soap(&d.av_control, AVT, "Play", &[("InstanceID", "0".into()), ("Speed", "1".into())]).map(|_| ())
}

pub fn stop(device_id: &str) -> Result<()> {
    let d = device(device_id)?;
    let mut a = active().lock().unwrap();
    if a.as_deref() == Some(device_id) {
        *a = None;
    }
    drop(a);
    soap(&d.av_control, AVT, "Stop", &[("InstanceID", "0".into())]).map(|_| ())
}

pub fn seek(device_id: &str, sec: f64) -> Result<()> {
    let d = device(device_id)?;
    seek_dev(&d, sec)
}

pub fn set_volume(device_id: &str, vol: u8) -> Result<()> {
    let d = device(device_id)?;
    let rc = d.rc_control.clone().ok_or_else(|| anyhow!("Gerät hat keine Lautstärkeregelung"))?;
    if d.group_volume {
        soap(&rc, GRC, "SetGroupVolume", &[("InstanceID", "0".into()), ("DesiredVolume", vol.min(100).to_string())])?;
    } else {
        soap(&rc, RC, "SetVolume", &[("InstanceID", "0".into()), ("Channel", "Master".into()), ("DesiredVolume", vol.min(100).to_string())])?;
    }
    Ok(())
}

pub fn status(device_id: &str) -> Result<Status> {
    let d = device(device_id)?;
    let ti = soap(&d.av_control, AVT, "GetTransportInfo", &[("InstanceID", "0".into())])?;
    let pi = soap(&d.av_control, AVT, "GetPositionInfo", &[("InstanceID", "0".into())])?;
    let volume = match &d.rc_control {
        Some(rc) if d.group_volume => soap(rc, GRC, "GetGroupVolume", &[("InstanceID", "0".into())])
            .ok()
            .and_then(|x| tag(&x, "CurrentVolume"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(-1),
        Some(rc) => soap(rc, RC, "GetVolume", &[("InstanceID", "0".into()), ("Channel", "Master".into())])
            .ok()
            .and_then(|x| tag(&x, "CurrentVolume"))
            .and_then(|v| v.parse().ok())
            .unwrap_or(-1),
        None => -1,
    };
    Ok(Status {
        state: tag(&ti, "CurrentTransportState").unwrap_or_default(),
        position: tag(&pi, "RelTime").map(|t| parse_hms(&t)).unwrap_or(0.0),
        duration: tag(&pi, "TrackDuration").map(|t| parse_hms(&t)).unwrap_or(0.0),
        volume,
    })
}

// ---------- Klang (Sonos: Bass, Höhen, Loudness) ----------

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sound {
    pub supported: bool,
    /// -10 … 10
    pub bass: i32,
    pub treble: i32,
    pub loudness: bool,
}

fn rc_of(base: &str) -> String {
    format!("{base}/MediaRenderer/RenderingControl/Control")
}

pub fn sound_get(device_id: &str) -> Result<Sound> {
    let d = device(device_id)?;
    let Some(base) = d.members.first() else {
        return Ok(Sound { supported: false, bass: 0, treble: 0, loudness: false });
    };
    let rc = rc_of(base);
    let num = |action: &str, field: &str| -> i32 {
        soap(&rc, RC, action, &[("InstanceID", "0".into())]).ok().and_then(|x| tag(&x, field)).and_then(|v| v.parse().ok()).unwrap_or(0)
    };
    let loud = soap(&rc, RC, "GetLoudness", &[("InstanceID", "0".into()), ("Channel", "Master".into())])
        .ok()
        .and_then(|x| tag(&x, "CurrentLoudness"))
        .map(|v| v == "1")
        .unwrap_or(false);
    Ok(Sound { supported: true, bass: num("GetBass", "CurrentBass"), treble: num("GetTreble", "CurrentTreble"), loudness: loud })
}

/// Gilt für alle Lautsprecher der Gruppe
pub fn sound_set(device_id: &str, s: &Sound) -> Result<()> {
    let d = device(device_id)?;
    if d.members.is_empty() {
        bail!("Dieses Gerät hat keine Klangregelung");
    }
    for base in &d.members {
        let rc = rc_of(base);
        soap(&rc, RC, "SetBass", &[("InstanceID", "0".into()), ("DesiredBass", s.bass.clamp(-10, 10).to_string())])?;
        soap(&rc, RC, "SetTreble", &[("InstanceID", "0".into()), ("DesiredTreble", s.treble.clamp(-10, 10).to_string())])?;
        let _ = soap(&rc, RC, "SetLoudness", &[("InstanceID", "0".into()), ("Channel", "Master".into()), ("DesiredLoudness", if s.loudness { "1" } else { "0" }.into())]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn helpers() {
        assert_eq!(hms(3725.4), "1:02:05");
        assert_eq!(parse_hms("0:03:30.123"), 210.0);
        assert_eq!(base_of("http://192.168.1.5:1400/xml/device_description.xml"), "http://192.168.1.5:1400");
        assert_eq!(host_ip("http://192.168.1.5:1400/x"), Some(Ipv4Addr::new(192, 168, 1, 5)));
        let xml = r#"<s:Envelope xmlns:s="x"><s:Body><u:R xmlns:u="y"><CurrentTransportState>PLAYING</CurrentTransportState></u:R></s:Body></s:Envelope>"#;
        assert_eq!(tag(xml, "CurrentTransportState").as_deref(), Some("PLAYING"));
    }

    /// Datei-Server mit Range-Anfragen
    #[test]
    fn serves_ranges() {
        let dir = std::env::temp_dir().join(format!("mlcast{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("a.mp3");
        std::fs::write(&f, (0..100u8).collect::<Vec<_>>()).unwrap();
        let srv = server().unwrap();
        srv.files.lock().unwrap().insert("k1".into(), f);
        let mut r = agent()
            .get(&format!("http://127.0.0.1:{}/k1.mp3", srv.port))
            .header("Range", "bytes=10-19")
            .call()
            .unwrap();
        assert_eq!(r.status().as_u16(), 206);
        let body = r.body_mut().read_to_vec().unwrap();
        assert_eq!(body, (10..20u8).collect::<Vec<_>>());
        let r = agent().get(&format!("http://127.0.0.1:{}/nope.mp3", srv.port)).call().unwrap();
        assert_eq!(r.status().as_u16(), 404);
    }
}

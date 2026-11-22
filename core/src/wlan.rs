//! Handy im WLAN als Sync-Ziel: Minimal Player Connected („Von Minimal Library empfangen“).
//! Finden per UDP-Broadcast, einmal mit 6-stelligem Code koppeln, dann Dateien per HTTP übertragen.

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::io::Read;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const UDP_PORT: u16 = 47820;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Phone {
    pub id: String,
    pub name: String,
    pub addr: String,
    /// Auf dem Handy ist ein Empfangsordner gewählt
    pub ready: bool,
    pub free: Option<u64>,
}

/// Einmal suchen: Broadcast an alle Netze, Antworten `wait` lang sammeln
pub fn discover(wait: Duration) -> Vec<Phone> {
    let Ok(sock) = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)) else {
        return vec![];
    };
    let _ = sock.set_broadcast(true);
    let mut targets: Vec<Ipv4Addr> = vec![Ipv4Addr::BROADCAST];
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        if let if_addrs::IfAddr::V4(v) = i.addr
            && !v.ip.is_loopback()
            && !v.ip.is_link_local()
        {
            let b = v
                .broadcast
                .unwrap_or_else(|| Ipv4Addr::from(u32::from(v.ip) | !u32::from(v.netmask)));
            if !targets.contains(&b) {
                targets.push(b);
            }
        }
    }
    for t in &targets {
        let _ = sock.send_to(b"MLDISCOVER1", (*t, UDP_PORT));
    }
    let end = Instant::now() + wait;
    let mut out: Vec<Phone> = Vec::new();
    let mut buf = [0u8; 2048];
    let mut resent = false;
    while let Some(left) = end.checked_duration_since(Instant::now()) {
        if left.is_zero() {
            break;
        }
        // nach der halben Zeit noch einmal fragen (UDP kann verloren gehen)
        if !resent && left < wait / 2 {
            resent = true;
            for t in &targets {
                let _ = sock.send_to(b"MLDISCOVER1", (*t, UDP_PORT));
            }
        }
        let _ = sock.set_read_timeout(Some(left.min(Duration::from_millis(200))));
        let Ok((n, from)) = sock.recv_from(&mut buf) else {
            continue;
        };
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&buf[..n]) else {
            continue;
        };
        let (Some(id), Some(port)) = (v["id"].as_str(), v["port"].as_u64()) else {
            continue;
        };
        if out.iter().any(|p| p.id == id) {
            continue;
        }
        out.push(Phone {
            id: id.to_string(),
            name: v["name"].as_str().unwrap_or("Handy").to_string(),
            addr: SocketAddr::new(from.ip(), port as u16).to_string(),
            ready: v["ready"].as_bool().unwrap_or(false),
            free: v["free"].as_i64().filter(|f| *f >= 0).map(|f| f as u64),
        });
    }
    out
}

// ---- Hintergrund-Suche: hält „wer ist gerade da“ aktuell, solange jemand fragt ----

struct Watch {
    seen: HashMap<String, (Instant, Phone)>,
    asked: Instant,
    running: bool,
}

static WATCH: Mutex<Option<Watch>> = Mutex::new(None);

fn touch() {
    let mut g = WATCH.lock().unwrap();
    let w = g.get_or_insert_with(|| Watch {
        seen: HashMap::new(),
        asked: Instant::now(),
        running: false,
    });
    w.asked = Instant::now();
    if w.running {
        return;
    }
    w.running = true;
    std::thread::spawn(|| {
        loop {
            let found = discover(Duration::from_millis(1500));
            let mut g = WATCH.lock().unwrap();
            let Some(w) = g.as_mut() else { break };
            for p in found {
                w.seen.insert(p.id.clone(), (Instant::now(), p));
            }
            w.seen
                .retain(|_, (t, _)| t.elapsed() < Duration::from_secs(12));
            // eine Minute niemand gefragt → schlafen legen
            if w.asked.elapsed() > Duration::from_secs(60) {
                w.running = false;
                break;
            }
            drop(g);
            std::thread::sleep(Duration::from_millis(1500));
        }
    });
}

/// Zuletzt gesehenes Handy mit dieser Kennung (startet bei Bedarf die Suche im Hintergrund)
pub fn seen(id: &str) -> Option<Phone> {
    touch();
    WATCH
        .lock()
        .unwrap()
        .as_ref()?
        .seen
        .get(id)
        .filter(|(t, _)| t.elapsed() < Duration::from_secs(12))
        .map(|(_, p)| p.clone())
}

/// Alle gerade sichtbaren Handys (für „Ziel hinzufügen“)
pub fn phones() -> Vec<Phone> {
    let mut list = discover(Duration::from_millis(1500));
    touch();
    if let Some(w) = WATCH.lock().unwrap().as_mut() {
        for p in &list {
            w.seen.insert(p.id.clone(), (Instant::now(), p.clone()));
        }
    }
    list.sort_by(|a, b| a.name.cmp(&b.name));
    list
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_recv_response(Some(Duration::from_secs(180)))
        .timeout_per_read(Some(Duration::from_secs(60)))
        .timeout_per_write(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .proxy(None) // Heimnetz: nie über einen Proxy
        .build()
        .into()
}

fn check(mut r: ureq::http::Response<ureq::Body>) -> Result<String> {
    let st = r.status().as_u16();
    let text = r.body_mut().read_to_string().unwrap_or_default();
    match st {
        200..=299 => Ok(text),
        401 => Err(anyhow!(
            "Das Handy kennt diesen Computer nicht mehr – Ziel bearbeiten und neu koppeln"
        )),
        403 => Err(anyhow!("Code falsch")),
        409 => Err(anyhow!("{text}")),
        _ => Err(anyhow!("Handy meldet Fehler {st}: {text}")),
    }
}

/// Mit dem Code vom Handy-Bildschirm koppeln → Schlüssel
pub fn pair(addr: &str, code: &str, client: &str) -> Result<String> {
    let r = agent()
        .post(&format!("http://{addr}/pair"))
        .header("X-Client", client)
        .send(code.trim().replace(' ', "").as_bytes())
        .map_err(|e| anyhow!("Handy nicht erreichbar: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(&check(r)?)?;
    v["token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow!("Keine Antwort vom Handy"))
}

/// Name dieses Computers (zeigt das Handy bei „Gekoppelte Computer“ an)
pub fn computer_name() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .filter(|s| !s.is_empty())
        })
        .unwrap_or_else(|| "Computer".into())
}

/// Pfad für die URL: alles außer A–Z, 0–9, „-._~/“ prozentkodieren
fn enc(rel: &str) -> String {
    let mut s = String::new();
    for b in rel.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

pub struct Client {
    base: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(addr: &str, token: &str) -> Client {
        Client {
            base: format!("http://{addr}"),
            token: token.to_string(),
            agent: agent(),
        }
    }

    pub fn info(&self) -> Result<serde_json::Value> {
        let r = self
            .agent
            .get(&format!("{}/info", self.base))
            .header("X-Token", &self.token)
            .call()?;
        Ok(serde_json::from_str(&check(r)?)?)
    }

    /// Alle Dateien im Empfangsordner: relativer Pfad (klein) → (Größe)
    pub fn list(&self) -> Result<HashMap<String, u64>> {
        let r = self
            .agent
            .get(&format!("{}/list", self.base))
            .header("X-Token", &self.token)
            .call()?;
        let text = check(r)?;
        Ok(text
            .lines()
            .filter_map(|l| {
                let (size, rel) = l.split_once('\t')?;
                Some((rel.to_lowercase(), size.parse().ok()?))
            })
            .collect())
    }

    pub fn begin(&self, total: usize) -> Result<()> {
        let r = self
            .agent
            .post(&format!("{}/begin", self.base))
            .header("X-Token", &self.token)
            .send(total.to_string().as_bytes())?;
        check(r).map(|_| ())
    }

    pub fn put(&self, rel: &str, data: &mut dyn Read, cancel: &AtomicBool) -> Result<()> {
        let mut r = Cancelable {
            inner: data,
            cancel,
        };
        let resp = self
            .agent
            .put(&format!("{}/f/{}", self.base, enc(rel)))
            .header("X-Token", &self.token)
            .send(ureq::SendBody::from_reader(&mut r))
            .map_err(|e| {
                if cancel.load(Ordering::Relaxed) {
                    anyhow!("Abgebrochen")
                } else {
                    anyhow!("{e}")
                }
            })?;
        check(resp).map(|_| ())
    }

    pub fn delete(&self, rel: &str) -> Result<bool> {
        let r = self
            .agent
            .delete(&format!("{}/f/{}", self.base, enc(rel)))
            .header("X-Token", &self.token)
            .call()?;
        if r.status().as_u16() == 404 {
            return Ok(false);
        }
        check(r).map(|_| true)
    }

    pub fn done(&self, json: &str) -> Result<()> {
        let r = self
            .agent
            .post(&format!("{}/done", self.base))
            .header("X-Token", &self.token)
            .send(json.as_bytes())?;
        check(r).map(|_| ())
    }
}

struct Cancelable<'a> {
    inner: &'a mut dyn Read,
    cancel: &'a AtomicBool,
}

impl Read for Cancelable<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("Abgebrochen"));
        }
        self.inner.read(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn encodes_paths() {
        assert_eq!(
            enc("Auto/01 - A & B+C.mp3"),
            "Auto/01%20-%20A%20%26%20B%2BC.mp3"
        );
        assert_eq!(enc("Ä/ö.mp3"), "%C3%84/%C3%B6.mp3");
    }
}

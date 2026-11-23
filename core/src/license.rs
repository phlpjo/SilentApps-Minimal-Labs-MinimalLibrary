//! Lizenz: 90 Tage Testphase, danach Schlüssel (Ed25519-signiert, offline geprüft).
//!
//! Schlüssel = "MLIB-" + base64url(JSON) + "." + base64url(Signatur über genau diese JSON-Bytes)
//! JSON: {"p":"minimal-library","v":1,"id":"…","n":"Name","e":"E-Mail","k":"buy"|"free","t":Unixzeit}

use anyhow::{Result, anyhow};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};

/// Öffentlicher Schlüssel (der private liegt nur beim Verkäufer / auf dem Server)
const PUBLIC_KEY: &str = "bmjzIHuCprUW3Gp4uF/qkj0tmceIl/TTxEHxi48oAvQ=";
/// Testphase in Tagen
pub const TRIAL_DAYS: i64 = 90;
/// Liste gesperrter Schlüssel (IDs)
pub const REVOKED_URL: &str = "https://silentapps-minimal-labs.com/api/revoked.json";

/// Vollversion freigeschaltet (Schlüssel gültig oder Testphase läuft) – für Teile, die selbst nachsehen
static UNLOCKED: AtomicBool = AtomicBool::new(true);

pub fn unlocked() -> bool {
    UNLOCKED.load(Ordering::Relaxed)
}

pub fn set_unlocked(v: bool) {
    UNLOCKED.store(v, Ordering::Relaxed);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Payload {
    pub p: String,
    pub v: u32,
    pub id: String,
    #[serde(default)]
    pub n: String,
    #[serde(default)]
    pub e: String,
    #[serde(default)]
    pub k: String,
    #[serde(default)]
    pub t: i64,
}

/// Schlüssel prüfen (nur offline: Signatur, Produkt, Version)
pub fn verify(key: &str) -> Result<Payload> {
    let key: String = key.chars().filter(|c| !c.is_whitespace()).collect();
    let body = key
        .strip_prefix("MLIB-")
        .ok_or_else(|| anyhow!("Kein gültiger Lizenzschlüssel"))?;
    let (data, sig) = body
        .split_once('.')
        .ok_or_else(|| anyhow!("Kein gültiger Lizenzschlüssel"))?;
    let data = URL_SAFE_NO_PAD
        .decode(data)
        .map_err(|_| anyhow!("Kein gültiger Lizenzschlüssel"))?;
    let sig = URL_SAFE_NO_PAD
        .decode(sig)
        .map_err(|_| anyhow!("Kein gültiger Lizenzschlüssel"))?;
    let pk: [u8; 32] = STANDARD
        .decode(PUBLIC_KEY)?
        .try_into()
        .map_err(|_| anyhow!("Prüfschlüssel defekt"))?;
    let vk = VerifyingKey::from_bytes(&pk)?;
    let sig = Signature::from_slice(&sig).map_err(|_| anyhow!("Kein gültiger Lizenzschlüssel"))?;
    vk.verify(&data, &sig)
        .map_err(|_| anyhow!("Lizenzschlüssel ist nicht echt"))?;
    let p: Payload =
        serde_json::from_slice(&data).map_err(|_| anyhow!("Kein gültiger Lizenzschlüssel"))?;
    if p.p != "minimal-library" || p.v != 1 {
        return Err(anyhow!("Schlüssel gehört nicht zu Minimal Library"));
    }
    Ok(p)
}

/// Liste gesperrter Schlüssel-IDs von der Website
pub fn fetch_revoked() -> Result<Vec<String>> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .build()
        .new_agent();
    let mut r = agent.get(REVOKED_URL).call()?;
    Ok(r.body_mut().read_json::<Vec<String>>()?)
}

/// Schlüssel erzeugen (nur mit privatem Schlüssel – für das Admin-Werkzeug)
pub fn sign(private_b64: &str, p: &Payload) -> Result<String> {
    use ed25519_dalek::{Signer, SigningKey};
    let sk: [u8; 32] = STANDARD
        .decode(private_b64.trim())?
        .try_into()
        .map_err(|_| anyhow!("privater Schlüssel: 32 Byte Base64 erwartet"))?;
    let sk = SigningKey::from_bytes(&sk);
    let data = serde_json::to_vec(p)?;
    let sig = sk.sign(&data);
    Ok(format!(
        "MLIB-{}.{}",
        URL_SAFE_NO_PAD.encode(&data),
        URL_SAFE_NO_PAD.encode(sig.to_bytes())
    ))
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "trial" | "licensed" | "expired" | "revoked"
    pub state: String,
    pub days_left: i64,
    pub name: String,
    pub email: String,
    pub kind: String,
    /// Kennung des Schlüssels (für die Update-Abfrage, damit die Website sie der Lizenz zuordnen kann)
    pub id: String,
    pub trial_start: i64,
}

/// Zustand aus Startzeit der Testphase, gespeichertem Schlüssel und Sperrliste
pub fn status(trial_start: i64, key: Option<&str>, revoked: &[String], now: i64) -> Status {
    if let Some(k) = key.filter(|k| !k.trim().is_empty())
        && let Ok(p) = verify(k)
    {
        let state = if revoked.contains(&p.id) {
            "revoked"
        } else {
            "licensed"
        };
        return Status {
            state: state.into(),
            days_left: 0,
            name: p.n,
            email: p.e,
            kind: p.k,
            id: p.id,
            trial_start,
        };
    }
    let left = TRIAL_DAYS - (now - trial_start).max(0) / 86400;
    Status {
        state: if left > 0 { "trial" } else { "expired" }.into(),
        days_left: left.max(0),
        trial_start,
        ..Default::default()
    }
}

impl Status {
    pub fn unlocked(&self) -> bool {
        self.state == "licensed" || self.state == "trial"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_garbage_and_foreign_keys() {
        assert!(verify("hallo").is_err());
        assert!(verify("MLIB-abc.def").is_err());
        // mit fremdem Schlüssel signiert
        let other = STANDARD.encode([7u8; 32]);
        let p = Payload {
            p: "minimal-library".into(),
            v: 1,
            id: "X".into(),
            n: "A".into(),
            e: String::new(),
            k: "free".into(),
            t: 0,
        };
        let k = sign(&other, &p).unwrap();
        assert!(verify(&k).is_err());
    }

    #[test]
    fn trial_counts_down() {
        let day = 86400;
        assert_eq!(status(0, None, &[], 10 * day).state, "trial");
        assert_eq!(status(0, None, &[], 10 * day).days_left, 80);
        assert_eq!(status(0, None, &[], 90 * day).state, "expired");
    }
}

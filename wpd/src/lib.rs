//! Smartphones & Player über die Windows-eigene Schnittstelle für tragbare Geräte (WPD/MTP).
//! Pfade auf dem Gerät: "<Speicher>/<Ordner>/<Ordner>", z. B. "Interner gemeinsamer Speicher/Music".
//! Alle Aufrufe laufen auf dem aufrufenden Thread (COM wird dort bei Bedarf initialisiert).

#![cfg(windows)]
#![allow(clippy::missing_safety_doc)]

use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::io::Read;
use windows::Win32::Devices::PortableDevices::*;
use windows::Win32::System::Com::*;
use windows::core::{GUID, PCWSTR, PWSTR};

#[derive(Debug, Clone)]
pub struct Device {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct Obj {
    pub id: String,
    pub name: String,
    pub folder: bool,
    pub size: u64,
}

fn com() {
    // MTA; ist der Thread schon STA (RPC_E_CHANGED_MODE), funktioniert WPD dort ebenso
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// PWSTR vom System übernehmen und freigeben
unsafe fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { CoTaskMemFree(Some(p.0 as *const _)) };
    s
}

fn manager() -> Result<IPortableDeviceManager> {
    com();
    Ok(unsafe { CoCreateInstance(&PortableDeviceManager, None, CLSCTX_INPROC_SERVER)? })
}

/// Angeschlossene Geräte (Handys, Player, Kameras …)
pub fn devices() -> Result<Vec<Device>> {
    let m = manager()?;
    unsafe {
        let _ = m.RefreshDeviceList();
        let mut n = 0u32;
        m.GetDevices(std::ptr::null_mut(), &mut n)?;
        if n == 0 {
            return Ok(vec![]);
        }
        let mut ids = vec![PWSTR::null(); n as usize];
        m.GetDevices(ids.as_mut_ptr(), &mut n)?;
        let mut out = Vec::new();
        for p in ids.into_iter().take(n as usize) {
            let id = take(p);
            if id.is_empty() {
                continue;
            }
            let w = wide(&id);
            let mut len = 0u32;
            let _ = m.GetDeviceFriendlyName(PCWSTR(w.as_ptr()), PWSTR::null(), &mut len);
            let mut name = String::new();
            if len > 0 {
                let mut buf = vec![0u16; len as usize];
                if m.GetDeviceFriendlyName(PCWSTR(w.as_ptr()), PWSTR(buf.as_mut_ptr()), &mut len)
                    .is_ok()
                {
                    name = String::from_utf16_lossy(
                        &buf[..buf.iter().position(|&c| c == 0).unwrap_or(buf.len())],
                    );
                }
            }
            if name.trim().is_empty() {
                name = "Gerät".into();
            }
            out.push(Device { id, name });
        }
        Ok(out)
    }
}

pub struct Session {
    dev: IPortableDevice,
    content: IPortableDeviceContent,
    props: IPortableDeviceProperties,
    keys: IPortableDeviceKeyCollection,
}

impl Drop for Session {
    fn drop(&mut self) {
        unsafe {
            let _ = self.dev.Close();
        }
    }
}

impl Session {
    pub fn open(id: &str) -> Result<Session> {
        com();
        unsafe {
            let info: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)?;
            let n = wide("Minimal Library");
            info.SetStringValue(&WPD_CLIENT_NAME, PCWSTR(n.as_ptr()))?;
            info.SetUnsignedIntegerValue(&WPD_CLIENT_MAJOR_VERSION, 1)?;
            info.SetUnsignedIntegerValue(&WPD_CLIENT_MINOR_VERSION, 0)?;
            info.SetUnsignedIntegerValue(&WPD_CLIENT_REVISION, 0)?;
            let dev: IPortableDevice =
                CoCreateInstance(&PortableDeviceFTM, None, CLSCTX_INPROC_SERVER)?;
            let w = wide(id);
            dev.Open(PCWSTR(w.as_ptr()), &info).map_err(|e| anyhow!("Gerät lässt sich nicht öffnen ({e}) – ist es entsperrt und auf „Dateiübertragung“ gestellt?"))?;
            let content = dev.Content()?;
            let props = content.Properties()?;
            let keys: IPortableDeviceKeyCollection =
                CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER)?;
            for k in [
                &WPD_OBJECT_NAME,
                &WPD_OBJECT_ORIGINAL_FILE_NAME,
                &WPD_OBJECT_CONTENT_TYPE,
                &WPD_OBJECT_SIZE,
            ] {
                keys.Add(k)?;
            }
            Ok(Session {
                dev,
                content,
                props,
                keys,
            })
        }
    }

    fn info(&self, id: &str) -> Result<Obj> {
        unsafe {
            let w = wide(id);
            let v = self.props.GetValues(PCWSTR(w.as_ptr()), &self.keys)?;
            let mut name = v
                .GetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME)
                .map(|p| take(p))
                .unwrap_or_default();
            if name.is_empty() {
                name = v
                    .GetStringValue(&WPD_OBJECT_NAME)
                    .map(|p| take(p))
                    .unwrap_or_default();
            }
            let ct: GUID = v.GetGuidValue(&WPD_OBJECT_CONTENT_TYPE).unwrap_or_default();
            let folder = ct == WPD_CONTENT_TYPE_FOLDER || ct == WPD_CONTENT_TYPE_FUNCTIONAL_OBJECT;
            let size = v
                .GetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE)
                .unwrap_or(0);
            Ok(Obj {
                id: id.to_string(),
                name,
                folder,
                size,
            })
        }
    }

    /// Direkte Kinder eines Objekts ("DEVICE" = oberste Ebene → Speicher)
    pub fn children(&self, parent: &str) -> Result<Vec<Obj>> {
        unsafe {
            let w = wide(parent);
            let e =
                self.content
                    .EnumObjects(0, PCWSTR(w.as_ptr()), None::<&IPortableDeviceValues>)?;
            let mut out = Vec::new();
            loop {
                let mut buf = [PWSTR::null(); 64];
                let mut got = 0u32;
                let hr = e.Next(&mut buf, &mut got);
                for p in buf.iter().take(got as usize) {
                    let id = take(*p);
                    if let Ok(o) = self.info(&id) {
                        out.push(o);
                    }
                }
                if hr.is_err() || got == 0 || hr.0 == 1 {
                    break;
                }
            }
            Ok(out)
        }
    }

    /// Speicher (oberste Ebene)
    pub fn storages(&self) -> Result<Vec<Obj>> {
        Ok(self
            .children("DEVICE")?
            .into_iter()
            .filter(|o| o.folder)
            .collect())
    }

    /// "Speicher/Ordner/…" → Objekt-ID (None, wenn ein Teil fehlt)
    pub fn find(&self, path: &str) -> Result<Option<String>> {
        let mut cur = "DEVICE".to_string();
        for part in path.split(['/', '\\']).filter(|p| !p.is_empty()) {
            let kids = self.children(&cur)?;
            match kids
                .into_iter()
                .find(|o| o.folder && o.name.eq_ignore_ascii_case(part))
            {
                Some(o) => cur = o.id,
                None => return Ok(None),
            }
        }
        Ok(Some(cur))
    }

    /// Freier Platz des Speichers, auf dem `path` liegt
    pub fn free(&self, path: &str) -> Option<u64> {
        let storage = path.split(['/', '\\']).find(|p| !p.is_empty())?;
        let id = self
            .storages()
            .ok()?
            .into_iter()
            .find(|o| o.name.eq_ignore_ascii_case(storage))?
            .id;
        unsafe {
            let keys: IPortableDeviceKeyCollection =
                CoCreateInstance(&PortableDeviceKeyCollection, None, CLSCTX_INPROC_SERVER).ok()?;
            keys.Add(&WPD_STORAGE_FREE_SPACE_IN_BYTES).ok()?;
            let w = wide(&id);
            let v = self.props.GetValues(PCWSTR(w.as_ptr()), &keys).ok()?;
            v.GetUnsignedLargeIntegerValue(&WPD_STORAGE_FREE_SPACE_IN_BYTES)
                .ok()
        }
    }

    fn base_values(
        &self,
        parent: &str,
        name: &str,
        ct: &GUID,
        fmt: &GUID,
    ) -> Result<IPortableDeviceValues> {
        unsafe {
            let v: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)?;
            let p = wide(parent);
            let n = wide(name);
            v.SetStringValue(&WPD_OBJECT_PARENT_ID, PCWSTR(p.as_ptr()))?;
            v.SetStringValue(&WPD_OBJECT_NAME, PCWSTR(n.as_ptr()))?;
            v.SetStringValue(&WPD_OBJECT_ORIGINAL_FILE_NAME, PCWSTR(n.as_ptr()))?;
            v.SetGuidValue(&WPD_OBJECT_CONTENT_TYPE, ct)?;
            v.SetGuidValue(&WPD_OBJECT_FORMAT, fmt)?;
            Ok(v)
        }
    }

    pub fn mkdir(&self, parent: &str, name: &str) -> Result<String> {
        let v = self.base_values(
            parent,
            name,
            &WPD_CONTENT_TYPE_FOLDER,
            &WPD_OBJECT_FORMAT_PROPERTIES_ONLY,
        )?;
        unsafe {
            let mut id = PWSTR::null();
            self.content
                .CreateObjectWithPropertiesOnly(&v, &mut id)
                .map_err(|e| anyhow!("Ordner „{name}“ anlegen: {e}"))?;
            Ok(take(id))
        }
    }

    /// Datei anlegen. Bricht die Übertragung ab, verwirft das Gerät das halbe Objekt (kein Commit).
    pub fn upload(
        &self,
        parent: &str,
        name: &str,
        data: &mut dyn Read,
        size: u64,
        cancel: &dyn Fn() -> bool,
    ) -> Result<String> {
        let v = self.base_values(
            parent,
            name,
            &WPD_CONTENT_TYPE_GENERIC_FILE,
            &WPD_OBJECT_FORMAT_UNSPECIFIED,
        )?;
        unsafe {
            v.SetUnsignedLargeIntegerValue(&WPD_OBJECT_SIZE, size)?;
            let mut stream: Option<IStream> = None;
            let mut opt = 0u32;
            self.content
                .CreateObjectWithPropertiesAndData(&v, &mut stream, &mut opt, std::ptr::null_mut())
                .map_err(|e| anyhow!("„{name}“ anlegen: {e}"))?;
            let stream = stream.ok_or_else(|| anyhow!("Kein Datenstrom vom Gerät"))?;
            let mut buf = vec![0u8; (opt as usize).clamp(64 * 1024, 4 * 1024 * 1024)];
            loop {
                if cancel() {
                    return Err(anyhow!("Abgebrochen"));
                }
                let n = data.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                let mut off = 0;
                while off < n {
                    let mut w = 0u32;
                    stream
                        .Write(
                            buf[off..].as_ptr() as *const _,
                            (n - off) as u32,
                            Some(&mut w),
                        )
                        .ok()
                        .map_err(|e| anyhow!("Schreiben: {e}"))?;
                    if w == 0 {
                        return Err(anyhow!("Gerät nimmt keine Daten mehr an"));
                    }
                    off += w as usize;
                }
            }
            stream
                .Commit(STGC_DEFAULT)
                .map_err(|e| anyhow!("Abschließen: {e}"))?;
            let id = windows::core::Interface::cast::<IPortableDeviceDataStream>(&stream)
                .ok()
                .and_then(|d| d.GetObjectID().ok())
                .map(|p| take(p))
                .unwrap_or_default();
            Ok(id)
        }
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        unsafe {
            // PROPVARIANT (VT_LPWSTR) über eine Werteliste erzeugen
            let v: IPortableDeviceValues =
                CoCreateInstance(&PortableDeviceValues, None, CLSCTX_INPROC_SERVER)?;
            let w = wide(id);
            v.SetStringValue(&WPD_OBJECT_ID, PCWSTR(w.as_ptr()))?;
            let pv = v.GetValue(&WPD_OBJECT_ID)?;
            let ids: IPortableDevicePropVariantCollection = CoCreateInstance(
                &PortableDevicePropVariantCollection,
                None,
                CLSCTX_INPROC_SERVER,
            )?;
            ids.Add(&pv)?;
            self.content.Delete(
                PORTABLE_DEVICE_DELETE_NO_RECURSION.0 as u32,
                &ids,
                std::ptr::null_mut(),
            )?;
            Ok(())
        }
    }
}

/// Inhalt eines Ordners rekursiv: relativer Pfad (Kleinbuchstaben) → Objekt
#[derive(Default)]
pub struct Tree {
    pub files: HashMap<String, Obj>,
    pub dirs: HashMap<String, String>,
}

impl Session {
    pub fn tree(&self, root_id: &str) -> Result<Tree> {
        let mut t = Tree::default();
        t.dirs.insert(String::new(), root_id.to_string());
        let mut stack = vec![(root_id.to_string(), String::new())];
        while let Some((id, rel)) = stack.pop() {
            for o in self.children(&id)? {
                let r = if rel.is_empty() {
                    o.name.clone()
                } else {
                    format!("{rel}/{}", o.name)
                };
                if o.folder {
                    t.dirs.insert(r.to_lowercase(), o.id.clone());
                    stack.push((o.id.clone(), r));
                } else {
                    t.files.insert(r.to_lowercase(), o);
                }
            }
        }
        Ok(t)
    }
}

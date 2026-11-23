//! Ersatz für die Teile von Tauri, die die Befehle der Desktop-App benutzen.
//! Events gehen an alle verbundenen Browser (Server-Sent Events), Pfade zeigen auf den Datenordner des Pakets.
#![allow(dead_code)]

use serde::Serialize;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub type Wry = ();
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error(pub String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}

pub struct Inner {
    states: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    pub bus: crate::web::Bus,
    /// Datenordner (Datenbank, Einstellungen, Lizenz)
    pub data: PathBuf,
    /// Standard-Musikordner
    pub music: PathBuf,
    pub version: String,
    /// Ordner/Dateien, die der Browser abspielen bzw. laden darf
    pub allowed: Mutex<Vec<(PathBuf, bool)>>,
}

#[derive(Clone)]
pub struct AppHandle(pub Arc<Inner>);

impl AppHandle {
    pub fn new(data: PathBuf, music: PathBuf, version: &str, bus: crate::web::Bus) -> Self {
        AppHandle(Arc::new(Inner {
            states: Mutex::new(HashMap::new()),
            bus,
            data,
            music,
            version: version.into(),
            allowed: Mutex::new(Vec::new()),
        }))
    }
    pub fn state<T: Send + Sync + 'static>(&self) -> State<'_, T> {
        let any = self
            .0
            .states
            .lock()
            .unwrap()
            .get(&TypeId::of::<T>())
            .cloned()
            .expect("Zustand nicht angelegt");
        State(any.downcast::<T>().ok().expect("Zustand: falscher Typ"), PhantomData)
    }
    pub fn try_state<T: Send + Sync + 'static>(&self) -> Option<State<'_, T>> {
        let any = self.0.states.lock().unwrap().get(&TypeId::of::<T>()).cloned()?;
        any.downcast::<T>().ok().map(|a| State(a, PhantomData))
    }
    pub fn manage<T: Send + Sync + 'static>(&self, t: T) -> bool {
        self.0
            .states
            .lock()
            .unwrap()
            .insert(TypeId::of::<T>(), Arc::new(t))
            .is_none()
    }
    pub fn path(&self) -> PathResolver {
        PathResolver(self.clone())
    }
    pub fn asset_protocol_scope(&self) -> Scope {
        Scope(self.clone())
    }
    pub fn get_webview_window(&self, label: &str) -> Option<WebviewWindow> {
        (label == "main").then(|| WebviewWindow::main(self))
    }
    pub fn package_info(&self) -> PackageInfo {
        PackageInfo { version: self.0.version.clone() }
    }
    pub fn exit(&self, code: i32) {
        std::process::exit(code)
    }
    pub fn app_handle(&self) -> &AppHandle {
        self
    }
    pub fn handle(&self) -> &AppHandle {
        self
    }
    /// Darf der Browser diese Datei laden (Musikordner, freigegebene Dateien)?
    pub fn may_serve(&self, p: &Path) -> bool {
        let Ok(p) = p.canonicalize() else { return false };
        self.0.allowed.lock().unwrap().iter().any(|(a, dir)| {
            let a = a.canonicalize().unwrap_or_else(|_| a.clone());
            if *dir { p.starts_with(&a) } else { p == a }
        })
    }
}

pub struct State<'a, T: Send + Sync + 'static>(Arc<T>, PhantomData<&'a ()>);
impl<T: Send + Sync + 'static> std::ops::Deref for State<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: Send + Sync + 'static> State<'_, T> {
    pub fn inner(&self) -> &T {
        &self.0
    }
}

pub struct PackageInfo {
    pub version: String,
}

pub struct PathResolver(AppHandle);
impl PathResolver {
    pub fn app_data_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.data.clone())
    }
    pub fn app_config_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.data.clone())
    }
    pub fn app_cache_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.data.join("cache"))
    }
    pub fn app_log_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.data.join("logs"))
    }
    pub fn home_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.data.clone())
    }
    pub fn audio_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.music.clone())
    }
    pub fn download_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.music.clone())
    }
    pub fn document_dir(&self) -> Result<PathBuf> {
        Ok(self.0.0.data.clone())
    }
    pub fn temp_dir(&self) -> Result<PathBuf> {
        Ok(std::env::temp_dir())
    }
}

pub struct Scope(AppHandle);
impl Scope {
    pub fn allow_directory<P: AsRef<Path>>(&self, p: P, _recursive: bool) -> Result<()> {
        self.0.0.allowed.lock().unwrap().push((p.as_ref().to_path_buf(), true));
        Ok(())
    }
    pub fn allow_file<P: AsRef<Path>>(&self, p: P) -> Result<()> {
        self.0.0.allowed.lock().unwrap().push((p.as_ref().to_path_buf(), false));
        Ok(())
    }
}

/// Im Browser gibt es nur ein „Fenster“: die Haupt-Bibliothek
#[derive(Clone)]
pub struct WebviewWindow(AppHandle);
impl WebviewWindow {
    pub fn main(app: &AppHandle) -> Self {
        WebviewWindow(app.clone())
    }
    pub fn label(&self) -> &str {
        "main"
    }
    pub fn app_handle(&self) -> &AppHandle {
        &self.0
    }
}

pub trait Manager {}
impl Manager for AppHandle {}
impl Manager for WebviewWindow {}

pub trait Emitter {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()>;
    fn emit_to<S: Serialize + Clone>(&self, target: &str, event: &str, payload: S) -> Result<()>;
}
impl Emitter for AppHandle {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        self.0.bus.send(event, serde_json::to_value(payload).unwrap_or_default());
        Ok(())
    }
    fn emit_to<S: Serialize + Clone>(&self, _target: &str, event: &str, payload: S) -> Result<()> {
        self.emit(event, payload)
    }
}
impl Emitter for WebviewWindow {
    fn emit<S: Serialize + Clone>(&self, event: &str, payload: S) -> Result<()> {
        self.0.emit(event, payload)
    }
    fn emit_to<S: Serialize + Clone>(&self, target: &str, event: &str, payload: S) -> Result<()> {
        self.0.emit_to(target, event, payload)
    }
}

pub mod async_runtime {
    pub use tokio::task::spawn_blocking;
    pub fn spawn<F>(f: F) -> tokio::task::JoinHandle<F::Output>
    where
        F: std::future::Future + Send + 'static,
        F::Output: Send + 'static,
    {
        tokio::spawn(f)
    }
}

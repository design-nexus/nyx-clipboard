//! What the service holds: the store, the settings (read again when the file changes),
//! the watcher's health, secrets kept for a moment in memory, and who's calling.

use nyx_clipboard_core::model::Transient;
use nyx_clipboard_core::peer::{self, Peer, Scope};
use nyx_clipboard_core::prefs::Settings;
use nyx_clipboard_core::{now, paths};
use nyx_clipboard_dbus::{CLIPBOARD, PATH};
use nyx_clipboard_store::Store;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;
use zbus::fdo;
use zbus::message::Header;
use zeroize::Zeroizing;

/// A probable secret that was copied: pasteable from the popup for a little while,
/// never stored.
pub struct Secret {
    pub info: Transient,
    pub value: Zeroizing<String>,
}

/// How long a copied secret stays pasteable from the popup.
pub const SECRET_SECONDS: f64 = 120.0;

pub struct State {
    pub store: Mutex<Store>,
    settings: Mutex<(Settings, Option<SystemTime>)>,
    settings_path: PathBuf,
    /// The clipboard is being watched, or why not.
    pub watching: Mutex<(bool, Option<String>)>,
    pub secrets: Mutex<Vec<Secret>>,
    pub codes: crate::codes::Codes,
    /// Actions Nyx apps registered while running: (manifest, app).
    pub registered: Mutex<Vec<(nyx_clipboard_core::actions::Manifest, String)>>,
    pub conn: OnceLock<zbus::blocking::Connection>,
    peers: Mutex<HashMap<String, Peer>>,
    allowed_dir: PathBuf,
    /// Wakes the maintenance thread (a secret or code was added).
    pub wake: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

pub fn err(e: impl std::fmt::Display) -> fdo::Error {
    fdo::Error::Failed(e.to_string())
}

impl State {
    pub fn new(store: Store) -> Arc<State> {
        let settings_path = paths::settings_file();
        let settings = Settings::load_from(&settings_path);
        let mtime = std::fs::metadata(&settings_path).and_then(|m| m.modified()).ok();
        Arc::new(State {
            store: Mutex::new(store),
            settings: Mutex::new((settings, mtime)),
            settings_path,
            watching: Mutex::new((false, None)),
            secrets: Mutex::new(Vec::new()),
            codes: crate::codes::Codes::default(),
            registered: Mutex::new(Vec::new()),
            conn: OnceLock::new(),
            peers: Mutex::new(HashMap::new()),
            allowed_dir: peer::allowed_dir().unwrap_or_else(|| PathBuf::from("/nonexistent")),
            wake: Mutex::new(None),
        })
    }

    /// The settings, read again if the file changed since.
    pub fn settings(&self) -> Settings {
        let mtime = std::fs::metadata(&self.settings_path).and_then(|m| m.modified()).ok();
        let mut guard = self.settings.lock().unwrap_or_else(|p| p.into_inner());
        if guard.1 != mtime {
            *guard = (Settings::load_from(&self.settings_path), mtime);
        }
        guard.0.clone()
    }

    /// Change and save the settings.
    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Settings {
        let mut s = self.settings();
        f(&mut s);
        if let Err(e) = s.save_to(&self.settings_path) {
            eprintln!("nyx-clipboardd: can't save settings: {e}");
        }
        let mtime = std::fs::metadata(&self.settings_path).and_then(|m| m.modified()).ok();
        *self.settings.lock().unwrap_or_else(|p| p.into_inner()) = (s.clone(), mtime);
        s
    }

    pub fn reload(&self) {
        self.settings.lock().unwrap_or_else(|p| p.into_inner()).1 = None;
    }

    pub fn with<T>(&self, f: impl FnOnce(&mut Store) -> nyx_clipboard_store::Result<T>) -> fdo::Result<T> {
        let mut s = self.store.lock().map_err(|_| err("store lock poisoned"))?;
        f(&mut s).map_err(err)
    }

    /// Tell listeners (the popup) to read again.
    pub fn changed(&self) {
        if let Some(c) = self.conn.get() {
            let _ = c.emit_signal(None::<&str>, PATH, CLIPBOARD, "Changed", &());
        }
    }

    pub fn wake(&self) {
        if let Some(tx) = self.wake.lock().ok().and_then(|g| g.clone()) {
            let _ = tx.send(());
        }
    }

    /// Who sent this call, from the bus's credentials for its connection.
    pub async fn peer(&self, conn: &zbus::Connection, hdr: &Header<'_>) -> fdo::Result<Peer> {
        let sender = hdr.sender().ok_or_else(|| fdo::Error::AccessDenied("no sender".into()))?.to_string();
        if let Some(p) = self.peers.lock().ok().and_then(|m| m.get(&sender).cloned()) {
            return Ok(p);
        }
        let dbus = fdo::DBusProxy::new(conn).await?;
        let name = zbus::names::BusName::try_from(sender.as_str()).map_err(err)?;
        let creds = dbus.get_connection_credentials(name).await?;
        use std::os::fd::AsFd;
        let pidfd = creds.process_fd().map(|fd| fd.as_fd());
        let found = peer::identify(creds.unix_user_id(), creds.process_id(), pidfd, &self.allowed_dir);
        let Some(p) = found else {
            return Err(fdo::Error::AccessDenied("Nyx Clipboard only answers Nyx apps (docs/API.md)".into()));
        };
        if let Ok(mut m) = self.peers.lock() {
            // Unique names are never reused on a bus, so a cached answer stays right.
            if m.len() > 256 {
                m.clear();
            }
            m.insert(sender, p.clone());
        }
        Ok(p)
    }

    /// The caller, if its scope is one of `allowed`.
    pub async fn require(&self, conn: &zbus::Connection, hdr: &Header<'_>, allowed: &[Scope]) -> fdo::Result<Peer> {
        let p = self.peer(conn, hdr).await?;
        if allowed.contains(&p.scope) {
            Ok(p)
        } else {
            Err(fdo::Error::AccessDenied(format!("{} may not do that", p.app)))
        }
    }

    /// Keep a copied secret in memory for the popup.
    pub fn add_secret(&self, label: &str, value: String) {
        let t = now();
        let id = format!("s{}", (t * 1000.0) as u64);
        let mut v = self.secrets.lock().unwrap_or_else(|p| p.into_inner());
        v.retain(|s| *s.value != value);
        v.insert(0, Secret {
            info: Transient { id, label: label.to_string(), created: t, expires: t + SECRET_SECONDS },
            value: Zeroizing::new(value),
        });
        v.truncate(5);
        drop(v);
        self.wake();
        self.changed();
    }

    /// Forget expired secrets; the time the next one expires.
    pub fn expire_secrets(&self) -> Option<f64> {
        let t = now();
        let mut v = self.secrets.lock().unwrap_or_else(|p| p.into_inner());
        let before = v.len();
        v.retain(|s| s.info.expires > t);
        let changed = v.len() != before;
        let next = v.iter().map(|s| s.info.expires).reduce(f64::min);
        drop(v);
        if changed {
            self.changed();
        }
        next
    }
}

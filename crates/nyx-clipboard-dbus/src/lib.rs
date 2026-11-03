//! Nyx Clipboard's D-Bus names (docs/API.md), and a blocking client for programs
//! without a GLib main loop (the Settings helper, tests).

use nyx_clipboard_core::model::{Entry, Status};
use nyx_clipboard_core::query::Search;
use serde::de::DeserializeOwned;

pub const BUS: &str = "io.github.design_nexus.nyx.Clipboard1";
pub const PATH: &str = "/io/github/design_nexus/nyx/Clipboard1";
/// History, snippets, pasting.
pub const CLIPBOARD: &str = "io.github.design_nexus.nyx.Clipboard1";
/// Contextual actions.
pub const ACTIONS: &str = "io.github.design_nexus.nyx.Clipboard.Actions1";
/// Verification codes (restricted callers only).
pub const CODES: &str = "io.github.design_nexus.nyx.Clipboard.Otp1";

/// The error callers get when they aren't allowed.
pub const ACCESS_DENIED: &str = "org.freedesktop.DBus.Error.AccessDenied";

pub struct Client {
    conn: zbus::blocking::Connection,
}

impl Client {
    pub fn connect() -> zbus::Result<Client> {
        Ok(Client { conn: zbus::blocking::Connection::session()? })
    }

    /// Call `method` on `iface` with `body`, returning the reply's single value.
    pub fn call<B, R>(&self, iface: &str, method: &str, body: &B) -> zbus::Result<R>
    where
        B: serde::Serialize + zbus::zvariant::DynamicType,
        R: DeserializeOwned + zbus::zvariant::Type,
    {
        let reply = self.conn.call_method(Some(BUS), PATH, Some(iface), method, body)?;
        reply.body().deserialize::<R>()
    }

    fn json<T: DeserializeOwned, B: serde::Serialize + zbus::zvariant::DynamicType>(
        &self,
        iface: &str,
        method: &str,
        body: &B,
    ) -> zbus::Result<T> {
        let s: String = self.call(iface, method, body)?;
        serde_json::from_str(&s).map_err(|e| zbus::Error::Failure(e.to_string()))
    }

    pub fn status(&self) -> zbus::Result<Status> {
        self.json(CLIPBOARD, "Status", &())
    }

    pub fn search(&self, s: &Search) -> zbus::Result<Vec<Entry>> {
        self.json(CLIPBOARD, "Search", &(serde_json::to_string(s).unwrap_or_default(),))
    }

    pub fn set_private(&self, on: bool) -> zbus::Result<bool> {
        self.call(CLIPBOARD, "SetPrivate", &(on,))
    }

    /// Delete the history; pinned entries too when `pinned`.
    pub fn clear(&self, pinned: bool) -> zbus::Result<u32> {
        self.call(CLIPBOARD, "Clear", &(pinned,))
    }

    /// Ask the daemon to read settings.toml again now.
    pub fn reload(&self) -> zbus::Result<()> {
        self.call::<_, ()>(CLIPBOARD, "Reload", &())
    }
}

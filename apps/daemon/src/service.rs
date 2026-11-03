//! `io.github.design_nexus.nyx.Clipboard1`: the history, snippets and pasting, for the
//! popup, Glimpse and Settings (JSON in and out, a `Changed` signal). Every method checks
//! its caller; verification codes are never part of this interface.

use crate::clip;
use crate::state::{State, err};
use nyx_clipboard_core::classify::TEXT;
use nyx_clipboard_core::model::{Rep, Status};
use nyx_clipboard_core::peer::Scope;
use nyx_clipboard_core::query::Search;
use nyx_clipboard_core::snippet::{self, Snippet};
use nyx_clipboard_core::now;
use std::sync::Arc;
use std::time::Duration;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

/// How long the popup gets to close before the paste keys are sent.
pub const PASTE_DELAY: Duration = Duration::from_millis(140);

const READERS: &[Scope] = &[Scope::Popup, Scope::Glimpse];
const STATUS: &[Scope] = &[Scope::Popup, Scope::Glimpse, Scope::Settings];

pub struct Service {
    pub state: Arc<State>,
}

fn json<T: serde::Serialize>(v: &T) -> fdo::Result<String> {
    serde_json::to_string(v).map_err(err)
}

/// Put entries on the clipboard: one keeps every representation (or only its text when
/// `plain`); several are joined as text.
pub fn put(st: &State, ids: &[i64], plain: bool) -> fdo::Result<()> {
    let settings = st.settings();
    let store = st.store.lock().map_err(|_| err("store lock"))?;
    if ids.len() == 1 {
        let id = ids[0];
        let mut reps = store.reps(id).map_err(err)?;
        if reps.is_empty() {
            return Err(fdo::Error::InvalidArgs("no such entry".into()));
        }
        if plain {
            reps.retain(|r| r.mime == TEXT);
            if reps.is_empty() {
                return Err(fdo::Error::InvalidArgs("that entry has no text".into()));
            }
        }
        let _ = store.touch(id, now());
        drop(store);
        return clip::set(&reps, Some(id), false).map_err(err);
    }
    let mut parts = Vec::new();
    for id in ids {
        if let Some(t) = store.text(*id).map_err(err)?.filter(|t| !t.is_empty()) {
            parts.push(t);
        }
    }
    drop(store);
    if parts.is_empty() {
        return Err(fdo::Error::InvalidArgs("nothing to paste".into()));
    }
    let joined = parts.join(&settings.separator);
    clip::set(&[Rep { mime: TEXT.into(), data: joined.into_bytes() }], None, false).map_err(err)
}

#[interface(name = "io.github.design_nexus.nyx.Clipboard1")]
impl Service {
    /// `{entries, pinned, snippets, bytes, private, watching, problem, version}`.
    async fn status(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<String> {
        self.state.require(conn, &hdr, STATUS).await?;
        let stats = self.state.with(|s| s.stats())?;
        let (watching, problem) = self.state.watching.lock().map(|w| w.clone()).unwrap_or((false, None));
        json(&Status {
            entries: stats.entries,
            pinned: stats.pinned,
            snippets: stats.snippets,
            bytes: stats.bytes,
            private: self.state.settings().private,
            watching,
            problem,
            version: env!("CARGO_PKG_VERSION").into(),
        })
    }

    /// Entries matching `query` (JSON `{text, view, kind, offset, limit}`; `""` for the
    /// newest). Returns a JSON list of entries.
    async fn search(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, query: &str) -> fdo::Result<String> {
        self.state.require(conn, &hdr, READERS).await?;
        let q: Search = if query.trim().is_empty() { Search::default() } else { serde_json::from_str(query).map_err(err)? };
        json(&self.state.with(|s| s.search(&q))?)
    }

    /// One entry with its text (JSON), or `""`.
    async fn get(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: i64) -> fdo::Result<String> {
        self.state.require(conn, &hdr, READERS).await?;
        match self.state.with(|s| s.detail(id))? {
            Some(d) => json(&d),
            None => Ok(String::new()),
        }
    }

    /// Put an entry back on the clipboard (only its text when `plain`).
    async fn copy(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: i64,
        plain: bool,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, READERS).await?;
        put(&self.state, &[id], plain)?;
        let _ = Self::changed(&e).await;
        Ok(true)
    }

    /// Put entries on the clipboard and paste into the focused window (call it after
    /// the caller's own window has closed). Several entries are joined with the
    /// separator from Settings.
    async fn paste(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        ids: Vec<i64>,
        plain: bool,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, READERS).await?;
        if ids.is_empty() || ids.len() > 100 {
            return Err(fdo::Error::InvalidArgs("paste 1 to 100 entries".into()));
        }
        put(&self.state, &ids, plain)?;
        clip::paste_later(PASTE_DELAY);
        let _ = Self::changed(&e).await;
        Ok(true)
    }

    /// Copy some text (an action's result), optionally pasting it. The watcher records
    /// it like any other copy (unless private mode is on).
    async fn copy_text(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        text: &str,
        paste: bool,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, READERS).await?;
        if text.is_empty() || text.len() > 16 * 1024 * 1024 {
            return Err(fdo::Error::InvalidArgs("no text".into()));
        }
        clip::set_text(text, false).map_err(err)?;
        if paste {
            clip::paste_later(PASTE_DELAY);
        }
        Ok(true)
    }

    async fn set_pinned(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: i64,
        pinned: bool,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, READERS).await?;
        let c = self.state.with(|s| s.set_pinned(id, pinned, now()))?;
        let _ = Self::changed(&e).await;
        Ok(c)
    }

    async fn set_favorite(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: i64,
        favorite: bool,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, READERS).await?;
        let c = self.state.with(|s| s.set_favorite(id, favorite))?;
        let _ = Self::changed(&e).await;
        Ok(c)
    }

    /// Delete entries for good. Returns how many went.
    async fn delete(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        ids: Vec<i64>,
    ) -> fdo::Result<u32> {
        self.state.require(conn, &hdr, READERS).await?;
        let n = self.state.with(|s| s.delete(&ids))?;
        let _ = Self::changed(&e).await;
        Ok(n as u32)
    }

    /// Delete the history (pinned entries too when `pinned`), and forget secrets.
    async fn clear(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        pinned: bool,
    ) -> fdo::Result<u32> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Settings]).await?;
        let n = self.state.with(|s| s.clear(pinned))?;
        if let Ok(mut v) = self.state.secrets.lock() {
            v.clear();
        }
        let _ = Self::changed(&e).await;
        Ok(n as u32)
    }

    // ---- Snippets

    /// Snippets matching `query` (JSON list), by name.
    async fn snippets(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, query: &str) -> fdo::Result<String> {
        self.state.require(conn, &hdr, READERS).await?;
        json(&self.state.with(|s| s.snippets(query))?)
    }

    /// Save a snippet (JSON `{id, name, body}`; id 0 for a new one). Returns its id.
    async fn save_snippet(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        snippet: &str,
    ) -> fdo::Result<i64> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        let s: Snippet = serde_json::from_str(snippet).map_err(err)?;
        if s.body.is_empty() || s.body.len() > 1024 * 1024 {
            return Err(fdo::Error::InvalidArgs("a snippet needs text (at most 1 MiB)".into()));
        }
        let id = self.state.with(|st| st.save_snippet(&s, now()))?;
        let _ = Self::changed(&e).await;
        Ok(id)
    }

    async fn delete_snippet(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: i64,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        let c = self.state.with(|s| s.delete_snippet(id))?;
        let _ = Self::changed(&e).await;
        Ok(c)
    }

    /// Copy a snippet with its placeholders filled in, and paste it when `paste`.
    async fn use_snippet(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: i64, paste: bool) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, READERS).await?;
        let (snip, latest) = self.state.with(|s| Ok((s.snippet(id)?, s.latest_text())))?;
        let Some(snip) = snip else { return Err(fdo::Error::InvalidArgs("no such snippet".into())) };
        let latest = latest.unwrap_or_default();
        let text = snippet::expand(&snip.body, &snippet::Context { clipboard: &latest, now: now() });
        clip::set_text(&text, false).map_err(err)?;
        if paste {
            clip::paste_later(PASTE_DELAY);
        }
        Ok(true)
    }

    // ---- Secrets kept for a moment (never stored)

    /// Copied secrets still pasteable (JSON list of `{id, label, created, expires}`).
    async fn transient(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        self.state.expire_secrets();
        let v: Vec<_> = self.state.secrets.lock().map(|v| v.iter().map(|s| s.info.clone()).collect()).unwrap_or_default();
        json(&v)
    }

    /// Copy (and paste) a kept secret, marked sensitive.
    async fn use_transient(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str, paste: bool) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        let value = self.state.secrets.lock().ok().and_then(|v| v.iter().find(|s| s.info.id == id).map(|s| s.value.clone()));
        let Some(value) = value else { return Ok(false) };
        clip::set_text(&value, true).map_err(err)?;
        if paste {
            clip::paste_later(PASTE_DELAY);
        }
        Ok(true)
    }

    async fn forget_transient(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        let gone = self.state.secrets.lock().map(|mut v| {
            let n = v.len();
            v.retain(|s| s.info.id != id);
            v.len() != n
        });
        let gone = gone.unwrap_or(false);
        if gone {
            self.state.changed();
        }
        Ok(gone)
    }

    // ---- Settings

    async fn private(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, STATUS).await?;
        Ok(self.state.settings().private)
    }

    /// Stop (or resume) recording. Returns the new state.
    async fn set_private(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        on: bool,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Settings]).await?;
        let s = self.state.update_settings(|s| s.private = on);
        let _ = Self::changed(&e).await;
        Ok(s.private)
    }

    /// Read settings.toml again now.
    async fn reload(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<()> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Settings]).await?;
        self.state.reload();
        let s = self.state.settings();
        let _ = self.state.with(|st| st.prune(&s, now()));
        self.state.changed();
        Ok(())
    }

    /// Something changed: read again. No payload.
    #[zbus(signal)]
    pub async fn changed(e: &SignalEmitter<'_>) -> zbus::Result<()>;
}

//! Verification codes from notifications: held in locked, zeroed memory for a few
//! minutes, never written anywhere, never put on the clipboard unless the user copies
//! one, and handed to the Vault's browser extension only for a site the notification
//! names (or after the user confirms).
//!
//! `io.github.design_nexus.nyx.Clipboard.Otp1`; every method checks its caller.

use crate::clip;
use crate::state::{State, err};
use nyx_clipboard_core::model::CodeInfo;
use nyx_clipboard_core::otp::{self, Confidence};
use nyx_clipboard_core::peer::Scope;
use nyx_clipboard_core::secret::Code;
use nyx_clipboard_core::now;
use serde::Serialize;
use std::sync::{Arc, Mutex};
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

struct Held {
    id: String,
    code: Code,
    app: String,
    sites: Vec<String>,
    words: Vec<String>,
    received: f64,
    expires: f64,
}

#[derive(Default)]
pub struct Codes {
    held: Mutex<Vec<Held>>,
    serial: Mutex<u64>,
}

/// A code the Vault may ask for, without the code itself.
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    pub app: String,
    pub confidence: &'static str,
    pub received: f64,
    pub expires: f64,
}

impl Codes {
    /// Hold the code in a notification, if it has one. Returns its id.
    pub fn offer(&self, summary: &str, body: &str, app: &str, seconds: u32) -> Option<String> {
        let d = otp::detect(summary, body)?;
        let code = Code::new(&d.code)?;
        let t = now();
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        // The same code again (a notification shown twice) keeps its first arrival.
        if let Some(h) = held.iter_mut().find(|h| h.code.same_as(d.code.as_bytes())) {
            h.expires = t + f64::from(seconds);
            return Some(h.id.clone());
        }
        let mut serial = self.serial.lock().unwrap_or_else(|p| p.into_inner());
        *serial += 1;
        let id = format!("c{}", *serial);
        held.insert(0, Held {
            id: id.clone(),
            code,
            app: app.chars().take(64).collect(),
            sites: d.sites,
            words: d.words,
            received: t,
            expires: t + f64::from(seconds),
        });
        held.truncate(5);
        Some(id)
    }

    /// For the popup: the codes, newest first.
    pub fn current(&self) -> Vec<CodeInfo> {
        let t = now();
        self.held
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter(|h| h.expires > t)
            .map(|h| CodeInfo {
                id: h.id.clone(),
                code: h.code.expose().to_string(),
                app: h.app.clone(),
                sites: h.sites.clone(),
                received: h.received,
                expires: h.expires,
            })
            .collect()
    }

    /// For the Vault: codes that may go to `origin`, best first, without the codes.
    pub fn candidates(&self, origin: &str) -> Vec<Candidate> {
        let t = now();
        let Some(host) = otp::host_of(origin) else { return Vec::new() };
        let held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<(Confidence, Candidate)> = held
            .iter()
            .filter(|h| h.expires > t)
            .filter_map(|h| {
                let c = otp::confidence(&h.sites, &h.words, &h.app, &host);
                (c != Confidence::None).then(|| {
                    (c, Candidate { id: h.id.clone(), app: h.app.clone(), confidence: c.as_str(), received: h.received, expires: h.expires })
                })
            })
            .collect();
        out.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.received.total_cmp(&a.1.received)));
        out.into_iter().map(|(_, c)| c).collect()
    }

    /// For the Vault: the code `id` for `origin`. A sure match is given; an unsure one
    /// only once the user `confirmed` it. A code is given once, then forgotten.
    pub fn take(&self, id: &str, origin: &str, confirmed: bool) -> Result<(zeroize::Zeroizing<String>, Confidence), &'static str> {
        let host = otp::host_of(origin).ok_or("not a website")?;
        let t = now();
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let i = held.iter().position(|h| h.id == id && h.expires > t).ok_or("that code has expired")?;
        let h = &held[i];
        let c = otp::confidence(&h.sites, &h.words, &h.app, &host);
        match c {
            Confidence::High => {}
            Confidence::Low if confirmed => {}
            Confidence::Low => return Err("the user hasn't confirmed this code is for this site"),
            Confidence::None => return Err("that code is for another site"),
        }
        let code = zeroize::Zeroizing::new(h.code.expose().to_string());
        held.remove(i);
        Ok((code, c))
    }

    pub fn value(&self, id: &str) -> Option<zeroize::Zeroizing<String>> {
        let t = now();
        let held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        held.iter().find(|h| h.id == id && h.expires > t).map(|h| zeroize::Zeroizing::new(h.code.expose().to_string()))
    }

    pub fn dismiss(&self, id: &str) -> bool {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let before = held.len();
        held.retain(|h| h.id != id);
        held.len() != before
    }

    /// Forget expired codes. Returns whether any went, and when the next one expires.
    pub fn expire(&self) -> (bool, Option<f64>) {
        let t = now();
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let before = held.len();
        held.retain(|h| h.expires > t);
        (held.len() != before, held.iter().map(|h| h.expires).reduce(f64::min))
    }

    /// Forget every code (lock, sleep, exit).
    pub fn clear(&self) -> bool {
        let mut held = self.held.lock().unwrap_or_else(|p| p.into_inner());
        let any = !held.is_empty();
        held.clear();
        any
    }
}

/// Copy a code with the sensitive hint, and clear it after the configured time if the
/// clipboard still holds it.
pub fn copy_code(st: &Arc<State>, code: zeroize::Zeroizing<String>, paste: bool) -> fdo::Result<bool> {
    clip::set_text(&code, true).map_err(err)?;
    if paste {
        clip::paste_later(crate::service::PASTE_DELAY);
    }
    let after = std::time::Duration::from_secs(u64::from(st.settings().code_clear_seconds));
    std::thread::spawn(move || {
        std::thread::sleep(after);
        clip::clear_if_holds(code.as_bytes());
    });
    Ok(true)
}

pub struct Otp {
    pub state: Arc<State>,
}

impl Otp {
    async fn announce(&self, e: &SignalEmitter<'_>) {
        let _ = Self::changed(e).await;
        self.state.changed();
    }
}

#[interface(name = "io.github.design_nexus.nyx.Clipboard.Otp1")]
impl Otp {
    /// A notification arrived (nyx-notifyd). Holds its code if it has one; returns
    /// whether it did. Nothing about the notification is kept beyond that.
    async fn offer(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        summary: &str,
        body: &str,
        app: &str,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Notifications]).await?;
        let s = self.state.settings();
        if !s.codes {
            return Ok(false);
        }
        let held = self.state.codes.offer(summary, body, app, s.code_seconds).is_some();
        if held {
            self.state.wake();
            self.announce(&e).await;
        }
        Ok(held)
    }

    /// The popup's codes (JSON list of `{id, code, app, sites, received, expires}`).
    async fn current(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        serde_json::to_string(&self.state.codes.current()).map_err(err)
    }

    /// Copy a held code (the popup), marked sensitive and cleared again later; paste it
    /// into the focused window when `paste`.
    async fn copy(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str, paste: bool) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup]).await?;
        let Some(code) = self.state.codes.value(id) else { return Ok(false) };
        copy_code(&self.state, code, paste)
    }

    /// Copy a code the caller shows (the Notification Center), the same safe way.
    async fn copy_code(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, code: &str) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::NotificationCenter]).await?;
        if code.is_empty() || code.len() > 32 || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(fdo::Error::InvalidArgs("not a verification code".into()));
        }
        copy_code(&self.state, zeroize::Zeroizing::new(code.to_string()), false)
    }

    async fn dismiss(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: &str,
    ) -> fdo::Result<bool> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Notifications]).await?;
        let gone = self.state.codes.dismiss(id);
        if gone {
            self.announce(&e).await;
        }
        Ok(gone)
    }

    /// For the Vault: codes that may be for `url` (JSON list of `{id, app, confidence,
    /// received, expires}`; confidence `high` or `low`). No code values.
    async fn r#match(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, url: &str) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Vault]).await?;
        serde_json::to_string(&self.state.codes.candidates(url)).map_err(err)
    }

    /// For the Vault: the code `id` for `url` (`{code, confidence}`), given once. An
    /// unsure match needs `confirmed` (the user picked it in the extension).
    async fn take(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: &str,
        url: &str,
        confirmed: bool,
    ) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Vault]).await?;
        let (code, c) = self.state.codes.take(id, url, confirmed).map_err(|m| fdo::Error::AccessDenied(m.into()))?;
        let reply = zeroize::Zeroizing::new(serde_json::json!({ "code": code.as_str(), "confidence": c.as_str() }).to_string());
        self.announce(&e).await;
        Ok(reply.to_string())
    }

    /// The codes changed (one arrived, was used or expired). No payload.
    #[zbus(signal)]
    pub async fn changed(e: &SignalEmitter<'_>) -> zbus::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn holds_matches_and_gives_once() {
        let c = Codes::default();
        let id = c.offer("GitHub", "Your GitHub launch code is 483920. https://github.com/sessions", "Thunderbird", 180).unwrap();
        assert!(c.offer("Weather", "Sunny, 21 degrees", "Weather", 180).is_none());
        assert_eq!(c.current()[0].code, "483920");
        let m = c.candidates("https://github.com/sessions/two-factor");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].confidence, "high");
        assert!(c.candidates("https://evil.example/login").is_empty());
        assert!(c.take(&id, "https://evil.example/login", true).is_err());
        let (code, conf) = c.take(&id, "https://github.com/login", false).unwrap();
        assert_eq!((code.as_str(), conf), ("483920", Confidence::High));
        assert!(c.take(&id, "https://github.com/login", false).is_err(), "a code is given once");
    }

    #[test]
    fn unsure_matches_need_confirmation_and_codes_expire() {
        let c = Codes::default();
        let id = c.offer("Messages", "Your verification code is 552210", "Messages", 180).unwrap();
        assert_eq!(c.candidates("https://bank.example")[0].confidence, "low");
        assert!(c.take(&id, "https://bank.example", false).is_err());
        assert!(c.take(&id, "https://bank.example", true).is_ok());
        let id = c.offer("Login", "Your login code is 771234", "Mail", 0).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5));
        assert!(c.value(&id).is_none());
        let (gone, _) = c.expire();
        assert!(gone);
        assert!(c.current().is_empty());
    }
}

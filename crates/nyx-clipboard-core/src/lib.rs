//! Nyx Clipboard without GTK or SQLite.
//!
//! - [`model`]: entries, what the daemon captured, what search returns.
//! - [`classify`]: which representations of a copy are kept, and what kind it is.
//! - [`sensitive`]: copies that must never be stored.
//! - [`transform`]: the built-in text actions (case, trim, formats, URL cleanup).
//! - [`otp`]: verification codes in notifications, and matching them to a website.
//! - [`secret`]: locked, zeroed memory for those codes.
//! - [`fuzzy`] and [`query`]: search.
//! - [`snippet`]: snippet templates.
//! - [`actions`]: the action registry's types and the built-in actions.
//! - [`prefs`]: the settings file in `~/.config/nyx-clipboard`.
//! - [`peer`]: who is calling, from the connection.
//! - [`paths`]: where everything lives.
//! - [`omarchy`]: reading Omarchy's clipboard history, once.

pub mod actions;
pub mod classify;
pub mod fuzzy;
pub mod model;
pub mod omarchy;
pub mod otp;
pub mod paths;
pub mod peer;
pub mod prefs;
pub mod query;
pub mod secret;
pub mod sensitive;
pub mod snippet;
pub mod transform;

/// Seconds since the epoch.
pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `bytes` as lowercase hex.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

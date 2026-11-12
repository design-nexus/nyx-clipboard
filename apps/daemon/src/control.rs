//! Clipboard on the Nyx control interface (API 2, nyx-core's nyx-actions contract) for
//! `nyx clipboard …`, Glimpse's commands and Nyx Assistant. Only what reveals nothing:
//! how much is kept, private mode on (turning it off stays with the popup and
//! Settings), clearing the history (the person's own frontends, asking first), and the
//! popup. The history itself stays behind `Clipboard1` and its scopes.

use crate::service::Service;
use crate::state::State;
use nyx_clipboard_core::peer::Scope;
use nyx_clipboard_dbus::{BUS, PATH};
use serde_json::{Value, json};
use std::sync::Arc;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

/// Callers that see these actions at all: any Nyx program beside the daemon.
const ANY: &[Scope] = &[Scope::Popup, Scope::Settings, Scope::Glimpse, Scope::Command, Scope::App];
/// Callers that may clear the history: the popup, Settings, and the person's own
/// command frontends (the `nyx` command, Glimpse), never other apps or agents.
const CLEARERS: &[Scope] = &[Scope::Popup, Scope::Settings, Scope::Glimpse, Scope::Command];

pub fn description() -> Value {
    let none = json!({"type": "object", "properties": {}, "additionalProperties": false});
    json!({
        "tools": [
            {"name": "clipboard.status", "description": "How much the clipboard history keeps, and whether private mode is on. Nothing from the history itself.",
             "inputSchema": none, "annotations": {"title": "Clipboard status", "readOnlyHint": true},
             "x-nyx": {"risk": "read_only", "result": "object"}},
            {"name": "clipboard.private", "description": "Turn private mode on: nothing copied is kept until it's turned off in the popup or Settings.",
             "inputSchema": none, "annotations": {"title": "Private mode on"},
             "x-nyx": {"risk": "writes", "confirmation": "never", "aliases": ["pause"], "result": "message"}},
            {"name": "clipboard.clear", "description": "Delete the clipboard history (pinned entries too when asked) and forget copied secrets.",
             "inputSchema": {"type": "object", "properties": {"pinned": {"type": "boolean"}}, "additionalProperties": false},
             "annotations": {"title": "Clear the history"},
             "x-nyx": {"risk": "destructive", "result": "message"}},
            {"name": "clipboard.open", "description": "Open the clipboard popup.",
             "inputSchema": none, "annotations": {"title": "Open Clipboard"},
             "x-nyx": {"risk": "writes", "confirmation": "never", "result": "none"}}
        ],
        "nyx": {"api": 2, "app": "io.github.design_nexus.nyx.Clipboard", "name": "Clipboard", "icon": "io.github.design_nexus.nyx.Clipboard",
                "namespaces": ["clipboard"], "bus_name": BUS, "object_path": PATH, "activation": "running", "open": {"action": "clipboard.open"}}
    })
}

fn error(kind: &str, message: impl Into<String>) -> String {
    json!({"error": {"kind": kind, "message": message.into()}}).to_string()
}

pub struct Control {
    pub state: Arc<State>,
}

#[interface(name = "io.github.design_nexus.nyx.Control")]
impl Control {
    async fn describe(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<String> {
        self.state.require(conn, &hdr, ANY).await?;
        Ok(description().to_string())
    }

    async fn invoke(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        action: &str,
        arguments: &str,
        options: &str,
    ) -> fdo::Result<String> {
        let peer = match self.state.require(conn, &hdr, ANY).await {
            Ok(p) => p,
            Err(e) => return Ok(error("permission_denied", e.to_string())),
        };
        let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
        let confirmed = serde_json::from_str::<Value>(options).ok().and_then(|o| o["confirmed"].as_bool()).unwrap_or(false);
        let emitter = SignalEmitter::new(conn, PATH)?;
        Ok(match action {
            "clipboard.status" => match self.state.with(|s| s.stats()) {
                Ok(stats) => json!({"ok": {"entries": stats.entries, "pinned": stats.pinned, "snippets": stats.snippets, "private": self.state.settings().private}}).to_string(),
                Err(e) => error("failed", e.to_string()),
            },
            "clipboard.private" => {
                self.state.update_settings(|s| s.private = true);
                let _ = Service::changed(&emitter).await;
                json!({"ok": {"message": "Private mode is on: nothing copied is kept until you turn it off in Clipboard"}}).to_string()
            }
            "clipboard.clear" if !CLEARERS.contains(&peer.scope) => error("permission_denied", format!("{} may not clear the clipboard history", peer.app)),
            "clipboard.clear" if !confirmed => error("confirmation_required", "clipboard.clear needs your go-ahead first (deletes the history)"),
            "clipboard.clear" => {
                let pinned = args["pinned"].as_bool().unwrap_or(false);
                match self.state.with(|s| s.clear(pinned)) {
                    Ok(n) => {
                        if let Ok(mut v) = self.state.secrets.lock() {
                            v.clear();
                        }
                        let _ = Service::changed(&emitter).await;
                        json!({"ok": {"message": format!("Cleared {n} entries")}}).to_string()
                    }
                    Err(e) => error("failed", e.to_string()),
                }
            }
            "clipboard.open" => {
                let popup = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.join("nyx-clipboard")));
                match popup.map(|p| std::process::Command::new(p).stdin(std::process::Stdio::null()).spawn()) {
                    Some(Ok(_)) => json!({"ok": null}).to_string(),
                    _ => error("failed", "the clipboard popup isn't installed"),
                }
            }
            other => error("unknown_action", format!("Clipboard has no action {other}")),
        })
    }

    async fn state(&self, topic: &str) -> String {
        error("unknown_action", format!("Clipboard has no topic {topic}"))
    }

    async fn watch(&self, topic: &str) -> fdo::Result<u32> {
        Err(fdo::Error::InvalidArgs(format!("Clipboard has no topic {topic}")))
    }

    async fn unwatch(&self, _topic: &str) {}
}

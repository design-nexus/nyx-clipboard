//! Talking to nyx-clipboardd over the session bus, asynchronously (JSON in and out).

use gtk::prelude::*;
use gtk::{gio, glib};
use nyx_clipboard_core::actions::{Action, Outcome};
use nyx_clipboard_core::model::{CodeInfo, Detail, Entry, Status, Transient};
use nyx_clipboard_core::query::Search;
use nyx_clipboard_core::snippet::Snippet;
use nyx_clipboard_dbus::{ACTIONS, BUS, CLIPBOARD, CODES, PATH};
use serde::de::DeserializeOwned;

fn bus() -> Result<gio::DBusConnection, String> {
    gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).map_err(|e| e.to_string())
}

async fn call(iface: &str, method: &str, args: Option<glib::Variant>) -> Result<glib::Variant, String> {
    bus()?.call_future(Some(BUS), PATH, iface, method, args.as_ref(), None, gio::DBusCallFlags::NONE, 130_000).await.map_err(|mut e| {
        if e.matches(gio::DBusError::ServiceUnknown) || e.matches(gio::DBusError::NameHasNoOwner) {
            "The clipboard service (nyx-clipboardd) isn't running".to_string()
        } else {
            gio::DBusError::strip_remote_error(&mut e);
            e.message().to_string()
        }
    })
}

async fn get<T: DeserializeOwned>(iface: &str, method: &str, args: Option<glib::Variant>) -> Result<T, String> {
    let v = call(iface, method, args).await?;
    let text = v.child_value(0).str().map(str::to_string).ok_or("unexpected reply")?;
    if text.is_empty() {
        return Err("not found".into());
    }
    serde_json::from_str(&text).map_err(|e| e.to_string())
}

async fn flag(iface: &str, method: &str, args: Option<glib::Variant>) -> Result<bool, String> {
    let v = call(iface, method, args).await?;
    Ok(v.child_value(0).get::<bool>().unwrap_or(false))
}

pub async fn status() -> Result<Status, String> {
    get(CLIPBOARD, "Status", None).await
}

pub async fn search(s: &Search) -> Result<Vec<Entry>, String> {
    get(CLIPBOARD, "Search", Some((serde_json::to_string(s).unwrap_or_default(),).to_variant())).await
}

pub async fn detail(id: i64) -> Result<Detail, String> {
    get(CLIPBOARD, "Get", Some((id,).to_variant())).await
}

pub async fn copy(id: i64, plain: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "Copy", Some((id, plain).to_variant())).await
}

pub async fn paste(ids: Vec<i64>, plain: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "Paste", Some((ids, plain).to_variant())).await
}

pub async fn copy_text(text: &str, paste: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "CopyText", Some((text, paste).to_variant())).await
}

pub async fn set_pinned(id: i64, on: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "SetPinned", Some((id, on).to_variant())).await
}

pub async fn set_favorite(id: i64, on: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "SetFavorite", Some((id, on).to_variant())).await
}

pub async fn delete(ids: Vec<i64>) -> Result<(), String> {
    call(CLIPBOARD, "Delete", Some((ids,).to_variant())).await.map(|_| ())
}

pub async fn snippets(query: &str) -> Result<Vec<Snippet>, String> {
    get(CLIPBOARD, "Snippets", Some((query,).to_variant())).await
}

pub async fn save_snippet(s: &Snippet) -> Result<i64, String> {
    let v = call(CLIPBOARD, "SaveSnippet", Some((serde_json::to_string(s).unwrap_or_default(),).to_variant())).await?;
    Ok(v.child_value(0).get::<i64>().unwrap_or(0))
}

pub async fn delete_snippet(id: i64) -> Result<bool, String> {
    flag(CLIPBOARD, "DeleteSnippet", Some((id,).to_variant())).await
}

pub async fn use_snippet(id: i64, paste: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "UseSnippet", Some((id, paste).to_variant())).await
}

pub async fn transient() -> Result<Vec<Transient>, String> {
    get(CLIPBOARD, "Transient", None).await
}

pub async fn use_transient(id: &str, paste: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "UseTransient", Some((id, paste).to_variant())).await
}

pub async fn forget_transient(id: &str) -> Result<bool, String> {
    flag(CLIPBOARD, "ForgetTransient", Some((id,).to_variant())).await
}

pub async fn set_private(on: bool) -> Result<bool, String> {
    flag(CLIPBOARD, "SetPrivate", Some((on,).to_variant())).await
}

pub async fn actions(id: i64) -> Result<Vec<Action>, String> {
    get(ACTIONS, "List", Some((id,).to_variant())).await
}

pub async fn run(id: i64, action: &str, args: serde_json::Value) -> Result<Outcome, String> {
    get(ACTIONS, "Run", Some((id, action, args.to_string()).to_variant())).await
}

pub async fn codes() -> Result<Vec<CodeInfo>, String> {
    get(CODES, "Current", None).await
}

pub async fn copy_code(id: &str, paste: bool) -> Result<bool, String> {
    flag(CODES, "Copy", Some((id, paste).to_variant())).await
}

pub async fn dismiss_code(id: &str) -> Result<bool, String> {
    flag(CODES, "Dismiss", Some((id,).to_variant())).await
}

/// Call `f` on every `Changed` (history or codes) until the subscriptions are dropped.
pub fn on_changed(f: std::rc::Rc<dyn Fn()>) -> Vec<gio::SignalSubscription> {
    let Ok(bus) = bus() else { return Vec::new() };
    [CLIPBOARD, CODES]
        .iter()
        .map(|iface| {
            let f = f.clone();
            bus.subscribe_to_signal(Some(BUS), Some(iface), Some("Changed"), Some(PATH), None, gio::DBusSignalFlags::NONE, move |_| f())
        })
        .collect()
}

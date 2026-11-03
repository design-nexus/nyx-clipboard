//! `io.github.design_nexus.nyx.Clipboard.Actions1`: the actions that fit an entry, and
//! running them. Built-in actions come first; the ones that hand content to another Nyx
//! app are listed only when that app is installed (`apps::available`). Nyx apps can add
//! actions while they run (`Register`); manifests in the actions folder are used once
//! the user approved them in Settings.

use crate::apps;
use crate::clip;
use crate::service::{PASTE_DELAY, put};
use crate::state::{State, err};
use nyx_clipboard_core::actions::{self, Action, Manifest, Outcome, Output, Subject};
use nyx_clipboard_core::classify::{self, HTML, TEXT};
use nyx_clipboard_core::model::{Kind, Rep};
use nyx_clipboard_core::peer::Scope;
use nyx_clipboard_core::snippet::Snippet;
use nyx_clipboard_core::transform::{self, Case};
use nyx_clipboard_core::{now, paths};
use serde::Deserialize;
use std::sync::Arc;
use zbus::message::Header;
use zbus::object_server::SignalEmitter;
use zbus::{fdo, interface};

pub struct Actions {
    pub state: Arc<State>,
}

/// Options for `Run` (JSON); all optional.
#[derive(Deserialize, Default)]
#[serde(default)]
pub struct RunArgs {
    /// Paste the result instead of only copying it.
    pub paste: bool,
    /// A note's name (Add to a note), a folder (Save), a language (Translate).
    pub target: String,
}

/// Approved manifests from the actions folder.
fn manifests(approved: &[String]) -> Vec<Manifest> {
    let Ok(dir) = std::fs::read_dir(paths::actions_dir()) else { return Vec::new() };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| toml::from_str::<Manifest>(&t).ok())
        .filter(|m| m.validate().is_ok() && approved.contains(&m.id))
        .collect()
}

/// Every manifest in the folder, approved or not (for Settings).
pub fn all_manifests() -> Vec<Manifest> {
    let Ok(dir) = std::fs::read_dir(paths::actions_dir()) else { return Vec::new() };
    dir.flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| toml::from_str::<Manifest>(&t).ok())
        .filter(|m| m.validate().is_ok())
        .collect()
}

struct Item {
    kind: Kind,
    text: String,
    html: Option<String>,
    pinned: bool,
    favorite: bool,
}

fn item(st: &State, id: i64) -> fdo::Result<Item> {
    let store = st.store.lock().map_err(|_| err("store lock"))?;
    let e = store.entry(id).map_err(err)?.ok_or_else(|| fdo::Error::InvalidArgs("no such entry".into()))?;
    let text = store.text(id).map_err(err)?.unwrap_or_default();
    let html = store.rep(id, HTML).map_err(err)?.map(|d| String::from_utf8_lossy(&d).into_owned());
    Ok(Item { kind: e.kind, text, html, pinned: e.pinned, favorite: e.favorite })
}

pub fn list(st: &State, id: i64) -> fdo::Result<Vec<Action>> {
    let it = item(st, id)?;
    let subject = Subject { kind: it.kind, text: &it.text, formatted: it.html.is_some(), pinned: it.pinned, favorite: it.favorite };
    let mut out: Vec<Action> = actions::builtin(&subject).into_iter().filter(|a| apps::available(&a.id)).collect();
    let settings = st.settings();
    for m in manifests(&settings.approved_actions) {
        if m.applies_to(it.kind) {
            out.push(m.action("manifest"));
        }
    }
    if let Ok(reg) = st.registered.lock() {
        for (m, app) in reg.iter() {
            if m.applies_to(it.kind) {
                out.push(m.action(app));
            }
        }
    }
    Ok(out)
}

/// Copy a transformed text (and paste it when asked): it becomes a new entry.
fn give_text(text: String, html: Option<String>, args: &RunArgs, label: &str) -> fdo::Result<Outcome> {
    if text.is_empty() {
        return Ok(Outcome { ok: false, message: "Nothing left after that".into(), ..Default::default() });
    }
    let mut reps = vec![Rep { mime: TEXT.into(), data: text.clone().into_bytes() }];
    if let Some(h) = html {
        reps.push(Rep { mime: HTML.into(), data: h.into_bytes() });
    }
    clip::set(&reps, None, false).map_err(err)?;
    if args.paste {
        clip::paste_later(PASTE_DELAY);
    }
    Ok(Outcome { ok: true, message: format!("{label}: copied"), text: Some(text), copied: None, close: args.paste })
}

pub fn run(st: &Arc<State>, id: i64, action: &str, args: &RunArgs) -> fdo::Result<Outcome> {
    let done = |m: &str| Ok(Outcome { ok: true, message: m.into(), ..Default::default() });
    match action {
        "paste" | "paste-plain" => {
            put(st, &[id], action == "paste-plain")?;
            clip::paste_later(PASTE_DELAY);
            return Ok(Outcome { ok: true, close: true, ..Default::default() });
        }
        "copy" => {
            put(st, &[id], false)?;
            return done("Copied");
        }
        "pin" | "unpin" => {
            st.with(|s| s.set_pinned(id, action == "pin", now()))?;
            return done(if action == "pin" { "Pinned" } else { "Unpinned" });
        }
        "favorite" | "unfavorite" => {
            st.with(|s| s.set_favorite(id, action == "favorite"))?;
            return done(if action == "favorite" { "Added to favorites" } else { "Removed from favorites" });
        }
        "delete" => {
            st.with(|s| s.delete(&[id]))?;
            return done("Deleted");
        }
        _ => {}
    }
    let it = item(st, id)?;
    let case = |c: Case| transform::change_case(&it.text, c);
    match action {
        "snippet" => {
            let name = classify::preview(&it.text).chars().take(48).collect();
            st.with(|s| s.save_snippet(&Snippet { name, body: it.text.clone(), ..Default::default() }, now()))?;
            done("Saved as a snippet")
        }
        "case-upper" => give_text(case(Case::Upper), None, args, "UPPERCASE"),
        "case-lower" => give_text(case(Case::Lower), None, args, "lowercase"),
        "case-title" => give_text(case(Case::Title), None, args, "Title Case"),
        "case-sentence" => give_text(case(Case::Sentence), None, args, "Sentence case"),
        "case-snake" => give_text(case(Case::Snake), None, args, "snake_case"),
        "case-kebab" => give_text(case(Case::Kebab), None, args, "kebab-case"),
        "case-camel" => give_text(case(Case::Camel), None, args, "camelCase"),
        "trim" => give_text(transform::trim_whitespace(&it.text), None, args, "Trimmed"),
        "join-lines" => give_text(transform::join_lines(&it.text), None, args, "Joined"),
        "clean-url" => give_text(transform::clean_urls_in(&it.text), None, args, "Tracking removed"),
        "remove-formatting" => {
            let text = it.html.as_deref().map(transform::html_to_text).filter(|t| !t.is_empty()).unwrap_or(it.text.clone());
            give_text(text, None, args, "Plain text")
        }
        "html-to-text" => give_text(transform::html_to_text(it.html.as_deref().unwrap_or(&it.text)), None, args, "Plain text"),
        "html-to-markdown" => give_text(transform::html_to_markdown(it.html.as_deref().unwrap_or(&it.text)), None, args, "Markdown"),
        "markdown-to-html" => {
            let html = transform::markdown_to_html(&it.text);
            // Rich text where it can be pasted as such, the markup everywhere else.
            give_text(html.clone(), Some(html), args, "HTML")
        }
        "json-pretty" => give_text(transform::json_pretty(&it.text).unwrap_or_default(), None, args, "Formatted"),
        "json-compact" => give_text(transform::json_compact(&it.text).unwrap_or_default(), None, args, "Compacted"),
        "url-encode" => give_text(transform::url_encode(&it.text), None, args, "Encoded"),
        "url-decode" => give_text(transform::url_decode(&it.text), None, args, "Decoded"),
        "copy-path" => give_text(it.text.clone(), None, args, "Path"),
        "copy-uri" => {
            let paths: Vec<String> = it.text.lines().map(str::to_string).collect();
            give_text(classify::uri_list(&paths).trim_end().replace("\r\n", "\n"), None, args, "File link")
        }
        other if other.starts_with("ext:") => run_external(st, &other[4..], &it.text, it.kind, args),
        other => apps::run(st, id, other, &it.text, it.kind, args),
    }
}

fn run_external(st: &State, id: &str, text: &str, kind: Kind, args: &RunArgs) -> fdo::Result<Outcome> {
    let settings = st.settings();
    let registered = st.registered.lock().map(|r| r.iter().map(|(m, _)| m.clone()).collect::<Vec<_>>()).unwrap_or_default();
    let m = manifests(&settings.approved_actions)
        .into_iter()
        .chain(registered)
        .find(|m| m.id == id && m.applies_to(kind))
        .ok_or_else(|| fdo::Error::InvalidArgs("that action isn't available".into()))?;
    let result = if let Some(d) = &m.dbus {
        apps::control_call(&d.bus, &d.path, &d.tool, &serde_json::json!({ "text": text }))
            .map(|v| v.get("text").and_then(|t| t.as_str()).map(str::to_string).unwrap_or_else(|| v.to_string()))
    } else {
        apps::run_program(&m.exec, text)
    }
    .map_err(err)?;
    match m.output {
        Output::Copy => give_text(result, None, args, &m.label),
        Output::Preview => Ok(Outcome { ok: true, message: m.label.clone(), text: Some(result), ..Default::default() }),
        Output::None => Ok(Outcome { ok: true, message: format!("{}: done", m.label), ..Default::default() }),
    }
}

#[interface(name = "io.github.design_nexus.nyx.Clipboard.Actions1")]
impl Actions {
    /// The actions for entry `id`, in menu order (JSON list of `{id, label, icon, group,
    /// source, destructive, previews}`).
    async fn list(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: i64) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Glimpse]).await?;
        let st = self.state.clone();
        let list = crate::offload(move || list(&st, id)).await?;
        serde_json::to_string(&list).map_err(err)
    }

    /// Run `action` on entry `id` with `args` (JSON `{paste, target}`). Returns
    /// `{ok, message, text, copied, close}`.
    async fn run(
        &self,
        #[zbus(header)] hdr: Header<'_>,
        #[zbus(connection)] conn: &zbus::Connection,
        #[zbus(signal_emitter)] e: SignalEmitter<'_>,
        id: i64,
        action: &str,
        args: &str,
    ) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Glimpse]).await?;
        let a: RunArgs = if args.trim().is_empty() { RunArgs::default() } else { serde_json::from_str(args).map_err(err)? };
        let st = self.state.clone();
        let action = action.to_string();
        let out = crate::offload(move || run(&st, id, &action, &a)).await?;
        let _ = crate::service::Service::changed(&e).await;
        self.state.changed();
        serde_json::to_string(&out).map_err(err)
    }

    /// A running Nyx app adds an action (a manifest in JSON, with a `dbus` target in a
    /// Nyx app). It lasts until the app's connection closes or it calls `Unregister`.
    async fn register(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, manifest: &str) -> fdo::Result<bool> {
        let peer = self.state.require(conn, &hdr, &[Scope::App, Scope::Popup]).await?;
        let m: Manifest = serde_json::from_str(manifest).map_err(err)?;
        m.validate().map_err(fdo::Error::InvalidArgs)?;
        if m.dbus.is_none() {
            return Err(fdo::Error::InvalidArgs("registered actions call their app over D-Bus (exec is for manifests)".into()));
        }
        let owner = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        let mut reg = self.state.registered.lock().map_err(|_| err("lock"))?;
        reg.retain(|(x, _)| x.id != m.id);
        reg.push((m, format!("{}@{owner}", peer.app)));
        Ok(true)
    }

    async fn unregister(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection, id: &str) -> fdo::Result<bool> {
        let peer = self.state.require(conn, &hdr, &[Scope::App, Scope::Popup]).await?;
        let mut reg = self.state.registered.lock().map_err(|_| err("lock"))?;
        let before = reg.len();
        reg.retain(|(x, app)| !(x.id == id && app.starts_with(&format!("{}@", peer.app))));
        Ok(reg.len() != before)
    }

    /// Third-party manifests in the actions folder, and which are approved (for
    /// Settings): JSON list of `{id, label, exec, approved}`.
    async fn manifests(&self, #[zbus(header)] hdr: Header<'_>, #[zbus(connection)] conn: &zbus::Connection) -> fdo::Result<String> {
        self.state.require(conn, &hdr, &[Scope::Popup, Scope::Settings]).await?;
        let approved = self.state.settings().approved_actions;
        let v: Vec<serde_json::Value> = all_manifests()
            .into_iter()
            .map(|m| {
                serde_json::json!({
                    "id": m.id, "label": m.label, "exec": m.exec, "dbus": m.dbus.as_ref().map(|d| &d.bus),
                    "approved": approved.contains(&m.id),
                })
            })
            .collect();
        serde_json::to_string(&v).map_err(err)
    }
}

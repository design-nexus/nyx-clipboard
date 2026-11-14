//! Clipboard's page in Nyx Settings (Settings → Desktop → Clipboard): `pages`,
//! `describe PAGE`, `set PAGE KEY VALUE` (see nyx-settings extensions/README.md). It
//! edits `~/.config/nyx-clipboard/settings.toml`, the file the popup and the daemon
//! read, and asks the daemon for its state. The daemon lets it see the status, switch
//! private mode and clear the history, nothing more.

use nyx_clipboard_core::actions::Manifest;
use nyx_clipboard_core::model::Status;
use nyx_clipboard_core::paths;
use nyx_clipboard_core::prefs::{Position, Settings};
use nyx_clipboard_dbus::Client;
use serde_json::{Value, json};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["pages"] => Ok(pages()),
        ["describe", _] => Ok(describe()),
        ["set", _, key, value] => set(key, value),
        ["theme-changed"] => Ok(Value::Null),
        _ => Err("usage: nyx-clipboard-settings pages | describe PAGE | set PAGE KEY VALUE".to_string()),
    };
    match result {
        Ok(Value::Null) => {}
        Ok(v) => println!("{v}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
}

fn pages() -> Value {
    json!([{
        "id": "clipboard", "title": "Clipboard", "icon": "edit-paste-symbolic",
        "description": "History, privacy, snippets, verification codes and the shortcut",
        "keywords": "clipboard history paste copy snippets private exclude apps retention verification code otp 2fa shortcut super v",
    }])
}

fn opts(pairs: &[(u32, &str)]) -> Value {
    Value::Array(pairs.iter().map(|(v, l)| json!([v.to_string(), l])).collect())
}

fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn manifests() -> Vec<Manifest> {
    let Ok(dir) = std::fs::read_dir(paths::actions_dir()) else { return Vec::new() };
    let mut v: Vec<Manifest> = dir
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
        .filter_map(|e| std::fs::read_to_string(e.path()).ok())
        .filter_map(|t| toml::from_str::<Manifest>(&t).ok())
        .filter(|m| m.validate().is_ok())
        .collect();
    v.sort_by(|a, b| a.label.cmp(&b.label));
    v
}

fn separator_id(s: &str) -> &'static str {
    match s {
        "\n" => "newline",
        "\n\n" => "blank",
        " " => "space",
        ", " => "comma",
        "\t" => "tab",
        _ => "newline",
    }
}

/// 1284 → "1,284".
fn count(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// What's stored, first on the page, with a way into the popup.
fn stored() -> Value {
    match Client::connect().and_then(|c| c.status()) {
        Ok(st) => {
            let items = if st.entries == 1 { "1 item".to_string() } else { format!("{} items", count(st.entries)) };
            let snippets = if st.snippets == 1 { "1 snippet".to_string() } else { format!("{} snippets", count(st.snippets)) };
            let watching = if !st.watching {
                format!("not watching the clipboard{}", st.problem.as_deref().map(|p| format!(": {p}")).unwrap_or_default())
            } else if st.private {
                "private mode: nothing new is saved".to_string()
            } else {
                "watching the clipboard".to_string()
            };
            json!({"kind": "button", "key": "open", "title": format!("{items} · {} pinned · {snippets}", count(st.pinned)),
                   "desc": format!("{} on this computer · {watching}", mb(st.bytes)), "label": "Open Clipboard",
                   "keywords": "history stored items size"})
        }
        Err(_) => json!({"kind": "info", "key": "status", "title": "The clipboard service isn't running",
                         "value": "systemctl --user start nyx-clipboardd"}),
    }
}

const ENTRIES: &[(u32, &str)] = &[(100, "100"), (500, "500"), (1000, "1,000"), (5000, "5,000"), (20000, "20,000"), (0, "No limit")];
const DAYS: &[(u32, &str)] = &[(1, "1 day"), (7, "1 week"), (30, "30 days"), (90, "90 days"), (365, "1 year"), (0, "Until the item limit")];
const ITEM_MB: &[(u32, &str)] = &[(1, "1 MB"), (5, "5 MB"), (10, "10 MB"), (50, "50 MB"), (100, "100 MB")];
const TOTAL_MB: &[(u32, &str)] = &[(128, "128 MB"), (512, "512 MB"), (1024, "1 GB"), (4096, "4 GB")];
const CODE_SECS: &[(u32, &str)] = &[(60, "1 minute"), (120, "2 minutes"), (180, "3 minutes"), (300, "5 minutes")];
const CLEAR_SECS: &[(u32, &str)] = &[(10, "After 10 seconds"), (30, "After 30 seconds"), (60, "After 1 minute"), (120, "After 2 minutes")];

fn separators() -> Value {
    json!([["newline", "A new line"], ["blank", "A blank line"], ["space", "A space"], ["comma", "A comma"], ["tab", "A tab"]])
}

fn banners(s: &Settings) -> Vec<Value> {
    if s.private { vec![json!({"text": "Private mode is on: nothing new is saved.", "warning": false})] } else { Vec::new() }
}

/// One switch per installed action, on once approved.
fn action_rows(s: &Settings) -> Vec<Value> {
    manifests()
        .into_iter()
        .map(|m| {
            let what = match (&m.dbus, m.exec.first()) {
                (Some(d), _) => format!("Calls {}", d.bus),
                (None, Some(p)) => format!("Runs {p}, which gets the item's text"),
                _ => String::new(),
            };
            json!({"kind": "switch", "key": format!("approve:{}", m.id), "title": m.label, "desc": what, "value": s.approved_actions.contains(&m.id)})
        })
        .collect()
}

/// Settings draws a preview pane beside the groups.
fn preview_supported() -> bool {
    std::env::var("SETTINGS_KINDS").is_ok_and(|k| k.split(' ').any(|x| x == "preview"))
}

/// `SUPER + CTRL + V` → "Super Ctrl V".
fn keys(shortcut: &str) -> String {
    shortcut
        .split('+')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| {
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().chain(c.flat_map(char::to_lowercase)).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn describe() -> Value {
    let s = Settings::load();
    if preview_supported() {
        return describe_split(&s, Client::connect().and_then(|c| c.status()).ok());
    }
    let banners = banners(&s);
    let (entries, days, item, total, code_secs, clear_secs) = (ENTRIES, DAYS, ITEM_MB, TOTAL_MB, CODE_SECS, CLEAR_SECS);
    let separators = separators();
    let actions = action_rows(&s);
    let mut groups = vec![
        json!({"title": "", "rows": [stored()]}),
        json!({"title": "", "rows": [
            {"kind": "switch", "key": "private", "title": "Private mode", "desc": "Stop saving what you copy until you turn it off. Also Ctrl Shift P in the pop-up.", "value": s.private, "refresh": true},
        ]}),
        json!({"title": "Opening", "rows": [
            {"kind": "entry", "key": "shortcut", "title": "Shortcut", "desc": "Opens the pop-up; Super V still pastes as Omarchy does. Written as Hyprland keys, like SUPER + CTRL + V.", "value": s.shortcut, "placeholder": "SUPER + CTRL + V"},
            {"kind": "segmented", "key": "position", "title": "Opens", "value": if s.position == Position::Cursor { "cursor" } else { "center" }, "options": [["center", "In the middle"], ["cursor", "At the pointer"]]},
            {"kind": "choice", "key": "separator", "title": "Between items pasted together", "desc": "When you pick several with Ctrl Space.", "value": separator_id(&s.separator), "options": separators},
        ]}),
        json!({"title": "History", "rows": [
            {"kind": "choice", "key": "max_entries", "title": "Keep", "desc": "Older items go first. Pinned items always stay.", "value": s.max_entries.to_string(), "options": opts(entries)},
            {"kind": "choice", "key": "max_age_days", "title": "For", "desc": "Items you haven't used for this long are removed.", "value": s.max_age_days.to_string(), "options": opts(days)},
            {"kind": "switch", "key": "images", "title": "Save images", "desc": format!("Screenshots and copied pictures, up to {} MB each.", s.max_item_mb), "value": s.images},
            {"kind": "button", "key": "clear", "title": "Clear history", "desc": "Everything except pinned items.", "label": "Clear", "confirm": "Click again to clear", "destructive": true, "refresh": true},
            {"kind": "disclosure", "key": "limits", "title": "Size limits, and clearing pinned items too", "rows": [
                {"kind": "choice", "key": "max_item_mb", "title": "Largest item", "desc": "Bigger copies aren't saved.", "value": s.max_item_mb.to_string(), "options": opts(item), "refresh": true},
                {"kind": "choice", "key": "max_total_mb", "title": "Space used", "desc": "The oldest items go when the history grows past this.", "value": s.max_total_mb.to_string(), "options": opts(total)},
                {"kind": "button", "key": "clear_all", "title": "Clear everything", "desc": "Also remove pinned items.", "label": "Clear all", "confirm": "Click again to clear all", "destructive": true, "refresh": true},
            ]},
        ]}),
        json!({"title": "Privacy", "rows": [
            {"kind": "switch", "key": "skip_secrets", "title": "Don't save passwords and keys", "desc": "Kept in memory for two minutes, never written down. Password managers' copies are never saved either way.", "value": s.skip_secrets},
            {"kind": "entry", "key": "excluded_apps", "title": "Don't save copies from", "desc": "Window classes, separated by commas (hyprctl clients shows them).", "value": s.excluded_apps.join(", "), "placeholder": "1password, org.keepassxc.KeePassXC"},
        ]}),
        json!({"title": "Verification codes", "collapsible": true, "open_by_default": false, "note": "Codes in notifications (\u{201c}Your code is 483920\u{201d}) appear at the top of the pop-up while they're valid, and Passwords' browser extension can fill them in. They're kept only in memory, never saved, and never put on the clipboard unless you copy one.", "rows": [
            {"kind": "switch", "key": "codes", "title": "Find codes in notifications", "value": s.codes},
            {"kind": "choice", "key": "code_seconds", "title": "Keep a code for", "value": s.code_seconds.to_string(), "options": opts(code_secs)},
            {"kind": "choice", "key": "code_clear_seconds", "title": "Clear a copied code", "desc": "Empty the clipboard this long after copying a code, if it still holds it.", "value": s.code_clear_seconds.to_string(), "options": opts(clear_secs)},
        ]}),
        json!({"title": "Assistant", "collapsible": true, "open_by_default": false, "rows": [
            {"kind": "entry", "key": "translate_to", "title": "Translate into", "desc": "The language the Translate action uses. Assistant actions run on this computer, only when you pick one.", "value": s.translate_to},
        ]}),
    ];
    if !actions.is_empty() {
        groups.push(json!({"title": "Actions from other apps", "collapsible": true, "open_by_default": false, "note": "Actions added to the actions folder work only once you turn them on.", "rows": actions}));
    }
    json!({ "poll": 10, "banners": banners, "groups": groups })
}

/// The page with the pop-up pictured beside it: the same settings, shorter, and the
/// stored counts in the pane.
fn describe_split(s: &Settings, status: Option<Status>) -> Value {
    let actions: Vec<Value> = action_rows(s)
        .into_iter()
        .map(|mut r| {
            r["icon"] = json!("application-x-addon-symbolic");
            r
        })
        .collect();
    let mut more = vec![
        json!({"kind": "disclosure", "key": "privacy", "icon": "dialog-password-symbolic", "title": "Privacy", "desc": "Passwords, keys and apps not to save", "rows": [
            {"kind": "switch", "key": "skip_secrets", "icon": "dialog-password-symbolic", "title": "Don't save passwords and keys", "desc": "Kept in memory for two minutes instead", "value": s.skip_secrets},
            {"kind": "entry", "key": "excluded_apps", "icon": "action-unavailable-symbolic", "title": "Don't save copies from", "desc": "Window classes, separated by commas", "value": s.excluded_apps.join(", "), "placeholder": "1password, org.keepassxc.KeePassXC"},
        ]}),
        json!({"kind": "disclosure", "key": "codes", "icon": "security-high-symbolic", "title": "Verification codes", "desc": "Codes from notifications, at the top while valid", "rows": [
            {"kind": "switch", "key": "codes", "icon": "preferences-system-notifications-symbolic", "title": "Find codes in notifications", "desc": "Kept only in memory, never saved", "value": s.codes},
            {"kind": "choice", "key": "code_seconds", "icon": "preferences-system-time-symbolic", "title": "Keep a code for", "desc": "Then it leaves the pop-up", "value": s.code_seconds.to_string(), "options": opts(CODE_SECS)},
            {"kind": "choice", "key": "code_clear_seconds", "icon": "edit-clear-symbolic", "title": "Clear a copied code", "desc": "If the clipboard still holds it", "value": s.code_clear_seconds.to_string(), "options": opts(CLEAR_SECS)},
        ]}),
        json!({"kind": "disclosure", "key": "assistant", "icon": "starred-symbolic", "title": "Assistant", "desc": if s.translate_to.is_empty() { "Actions that run on this computer".to_string() } else { format!("Translate into {}", s.translate_to) }, "rows": [
            {"kind": "entry", "key": "translate_to", "icon": "preferences-desktop-locale-symbolic", "title": "Translate into", "desc": "The language the Translate action uses", "value": s.translate_to},
        ]}),
    ];
    let mut note = "";
    if !actions.is_empty() {
        let on = actions.iter().filter(|r| r["value"] == true).count();
        let n = if actions.len() == 1 { "1 action".to_string() } else { format!("{} actions", actions.len()) };
        more.push(json!({"kind": "disclosure", "key": "actions", "icon": "application-x-addon-symbolic", "title": "Actions from other apps", "desc": format!("{n}, {on} on"), "rows": actions}));
        note = "Actions from other apps work only once you turn them on.";
    }
    let groups = vec![
        json!({"title": "", "rows": [
            {"kind": "switch", "key": "private", "icon": "view-conceal-symbolic", "title": "Private mode", "desc": "Stops saving what you copy. Also Ctrl Shift P", "value": s.private, "refresh": true},
        ]}),
        json!({"title": "Opening", "rows": [
            {"kind": "entry", "key": "shortcut", "icon": "input-keyboard-symbolic", "title": "Shortcut", "desc": "Super V still pastes as Omarchy does", "value": s.shortcut, "placeholder": "SUPER + CTRL + V"},
            {"kind": "segmented", "key": "position", "icon": "focus-windows-symbolic", "title": "Opens", "desc": "Where the pop-up appears", "value": if s.position == Position::Cursor { "cursor" } else { "center" }, "options": [["center", "In the middle"], ["cursor", "At the pointer"]]},
            {"kind": "choice", "key": "separator", "icon": "view-list-bullet-symbolic", "title": "Between items pasted together", "desc": "Several picked with Ctrl Space", "value": separator_id(&s.separator), "options": separators()},
        ]}),
        json!({"title": "History", "rows": [
            {"kind": "choice", "key": "max_entries", "icon": "document-open-recent-symbolic", "title": "Keep", "desc": "Older items go first; pinned stay", "value": s.max_entries.to_string(), "options": opts(ENTRIES)},
            {"kind": "choice", "key": "max_age_days", "icon": "preferences-system-time-symbolic", "title": "For", "desc": "Unused items are removed after", "value": s.max_age_days.to_string(), "options": opts(DAYS)},
            {"kind": "switch", "key": "images", "icon": "image-x-generic-symbolic", "title": "Save images", "desc": format!("Up to {} MB each", s.max_item_mb), "value": s.images},
            {"kind": "button", "key": "clear", "icon": "user-trash-symbolic", "title": "Clear history", "desc": "Everything except pinned items", "label": "Clear", "confirm": "Click again to clear", "destructive": true, "refresh": true},
            {"kind": "disclosure", "key": "limits", "icon": "drive-harddisk-symbolic", "title": "Size limits", "desc": "And clearing pinned items too", "rows": [
                {"kind": "choice", "key": "max_item_mb", "icon": "document-properties-symbolic", "title": "Largest item", "desc": "Bigger copies aren't saved", "value": s.max_item_mb.to_string(), "options": opts(ITEM_MB), "refresh": true},
                {"kind": "choice", "key": "max_total_mb", "icon": "drive-harddisk-symbolic", "title": "Space used", "desc": "The oldest items go past this", "value": s.max_total_mb.to_string(), "options": opts(TOTAL_MB)},
                {"kind": "button", "key": "clear_all", "icon": "edit-clear-all-symbolic", "title": "Clear everything", "desc": "Also remove pinned items", "label": "Clear all", "confirm": "Click again to clear all", "destructive": true, "refresh": true},
            ]},
        ]}),
        json!({"title": "", "note": note, "rows": more}),
    ];
    json!({ "poll": 10, "banners": banners(s), "groups": groups, "preview": preview(s, status.as_ref()) })
}

fn escape(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// The pop-up as the daemon reports it. Settings may only ask for the status, never
/// for the items themselves, so the pane shows counts rather than recent copies.
fn preview(s: &Settings, status: Option<&Status>) -> Value {
    let shortcut = keys(&s.shortcut);
    let line = if shortcut.is_empty() {
        "Newest first, search as you type.".to_string()
    } else {
        format!("What {shortcut} opens: newest first, search as you type.")
    };
    let mut blocks = Vec::new();
    match status {
        Some(st) => {
            if !st.watching {
                let why = st.problem.as_deref().map(|p| format!(": {}", escape(p))).unwrap_or_default();
                blocks.push(json!({"kind": "banner", "text": format!("Not watching the clipboard{why}"), "warning": true}));
            }
            let items = if st.entries == 1 { "1 item".to_string() } else { format!("{} items", count(st.entries)) };
            blocks.push(json!({"kind": "stats", "items": [
                {"label": "History", "value": items, "sub": mb(st.bytes)},
                {"label": "Pinned", "value": count(st.pinned), "sub": "Always kept"},
                {"label": "Private mode", "value": if st.private { "On" } else { "Off" }, "sub": "Ctrl Shift P", "tone": if st.private { "warn" } else { "" }},
            ]}));
            blocks.push(json!({"kind": "rows", "rows": [
                {"kind": "button", "key": "open", "icon": "edit-paste-symbolic", "title": "The pop-up", "desc": if shortcut.is_empty() { String::new() } else { format!("Or press {shortcut}") }, "label": "Open Clipboard"},
            ]}));
        }
        None => blocks.push(json!({"kind": "banner", "text": "The clipboard service isn't running: <tt>systemctl --user start nyx-clipboardd</tt>", "warning": true})),
    }
    let foot = if s.skip_secrets {
        "Passwords and keys are kept in memory for two minutes, never written down."
    } else {
        "Passwords and keys are saved like other copies; password managers' copies never are."
    };
    json!({"title": "The pop-up", "line": line, "blocks": blocks, "foot": foot, "foot_icon": "security-high-symbolic"})
}

fn reload() {
    if let Ok(c) = Client::connect() {
        let _ = c.reload();
    }
}

fn set(key: &str, value: &str) -> Result<Value, String> {
    let mut s = Settings::load();
    let num = || value.parse::<u32>().map_err(|_| "not a number".to_string());
    match key {
        "private" => {
            let on = value == "true";
            let c = Client::connect().map_err(|e| e.to_string())?;
            c.set_private(on).map_err(|e| e.to_string())?;
            return Ok(json!({"refresh": true}));
        }
        "open" => {
            std::process::Command::new("nyx-clipboard").arg("--show").spawn().map_err(|e| format!("can't open Clipboard: {e}"))?;
            return Ok(Value::Null);
        }
        "clear" | "clear_all" => {
            let c = Client::connect().map_err(|e| e.to_string())?;
            let n = c.clear(key == "clear_all").map_err(|e| e.to_string())?;
            return Ok(json!({"toast": format!("Removed {n} items."), "refresh": true}));
        }
        "shortcut" => {
            let key = value.trim().to_uppercase();
            let out = std::process::Command::new("nyx-clipboard").args(["--integrate", &key]).output().map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            return Ok(json!({"toast": format!("{key} opens Clipboard."), "refresh": true}));
        }
        "position" => s.position = if value == "cursor" { Position::Cursor } else { Position::Center },
        "separator" => {
            s.separator = match value {
                "blank" => "\n\n",
                "space" => " ",
                "comma" => ", ",
                "tab" => "\t",
                _ => "\n",
            }
            .into()
        }
        "max_entries" => s.max_entries = num()?,
        "max_age_days" => s.max_age_days = num()?,
        "max_item_mb" => s.max_item_mb = num()?,
        "max_total_mb" => s.max_total_mb = num()?,
        "code_seconds" => s.code_seconds = num()?,
        "code_clear_seconds" => s.code_clear_seconds = num()?,
        "images" => s.images = value == "true",
        "skip_secrets" => s.skip_secrets = value == "true",
        "codes" => s.codes = value == "true",
        "excluded_apps" => {
            s.excluded_apps = value.split(',').map(str::trim).filter(|v| !v.is_empty()).map(str::to_string).take(100).collect()
        }
        "translate_to" if value.trim().len() <= 40 => s.translate_to = value.trim().to_string(),
        k if k.starts_with("approve:") => {
            let id = &k["approve:".len()..];
            s.approved_actions.retain(|a| a != id);
            if value == "true" {
                s.approved_actions.push(id.to_string());
            }
        }
        _ => return Err(format!("unknown setting {key}")),
    }
    s.save().map_err(|e| e.to_string())?;
    reload();
    Ok(Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(v: &Value) -> Vec<&Value> {
        fn walk<'a>(rows: &'a Value, out: &mut Vec<&'a Value>) {
            for r in rows.as_array().into_iter().flatten() {
                out.push(r);
                walk(&r["rows"], out);
            }
        }
        let mut out = Vec::new();
        for g in v["groups"].as_array().unwrap() {
            walk(&g["rows"], &mut out);
        }
        out
    }

    #[test]
    fn shortcut_words() {
        assert_eq!(keys("SUPER + CTRL + V"), "Super Ctrl V");
        assert_eq!(keys("super+shift+F2"), "Super Shift F2");
        assert_eq!(keys(""), "");
    }

    #[test]
    fn split_page_has_icons_and_counts() {
        let st = Status { entries: 1284, pinned: 3, bytes: 3 * 1_048_576, private: true, watching: true, ..Default::default() };
        let page = describe_split(&Settings::default(), Some(st));
        for r in rows(&page) {
            assert!(r["icon"].as_str().is_some_and(|i| i.ends_with("-symbolic")), "{r}");
        }
        let p = &page["preview"];
        assert_eq!(p["title"], "The pop-up");
        let stats = &p["blocks"][0];
        assert_eq!(stats["kind"], "stats");
        assert_eq!(stats["items"][0]["value"], "1,284 items");
        assert_eq!(stats["items"][0]["sub"], "3.0 MB");
        assert_eq!(stats["items"][1]["value"], "3");
        assert_eq!(stats["items"][2]["value"], "On");
        assert!(p["blocks"].as_array().unwrap().iter().all(|b| b["kind"] != "list"));
    }

    #[test]
    fn pane_without_the_service_or_watching() {
        let p = preview(&Settings::default(), None);
        assert_eq!(p["blocks"][0]["kind"], "banner");
        let st = Status { watching: false, problem: Some("no <data-control>".into()), ..Default::default() };
        let p = preview(&Settings::default(), Some(&st));
        assert_eq!(p["blocks"][0]["text"], "Not watching the clipboard: no &lt;data-control&gt;");
    }

    #[test]
    fn foot_follows_skip_secrets() {
        let mut s = Settings { skip_secrets: true, ..Default::default() };
        assert!(preview(&s, None)["foot"].as_str().unwrap().contains("two minutes"));
        s.skip_secrets = false;
        assert!(!preview(&s, None)["foot"].as_str().unwrap().contains("two minutes"));
    }
}

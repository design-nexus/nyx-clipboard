//! Clipboard's page in Nyx Settings (Settings → Desktop → Clipboard): `pages`,
//! `describe PAGE`, `set PAGE KEY VALUE` (see nyx-settings extensions/README.md). It
//! edits `~/.config/nyx-clipboard/settings.toml`, the file the popup and the daemon
//! read, and asks the daemon for its state. The daemon lets it see the status, switch
//! private mode and clear the history, nothing more.

use nyx_clipboard_core::actions::Manifest;
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

fn describe() -> Value {
    let s = Settings::load();
    let status = Client::connect().and_then(|c| c.status());
    let state = match &status {
        Ok(st) if !st.watching => {
            format!("Not watching the clipboard{}", st.problem.as_deref().map(|p| format!(": {p}")).unwrap_or_default())
        }
        Ok(st) => format!("{} items, {} pinned, {} snippets · {}", st.entries, st.pinned, st.snippets, mb(st.bytes)),
        Err(_) => "The clipboard service isn't running (systemctl --user start nyx-clipboardd)".to_string(),
    };
    let mut banners = Vec::new();
    if s.private {
        banners.push(json!({"text": "Private mode is on: nothing new is saved.", "warning": false}));
    }
    let entries = &[(100, "100"), (500, "500"), (1000, "1,000"), (5000, "5,000"), (20000, "20,000"), (0, "No limit")];
    let days = &[(1, "1 day"), (7, "1 week"), (30, "30 days"), (90, "90 days"), (365, "1 year"), (0, "Until the item limit")];
    let item = &[(1, "1 MB"), (5, "5 MB"), (10, "10 MB"), (50, "50 MB"), (100, "100 MB")];
    let total = &[(128, "128 MB"), (512, "512 MB"), (1024, "1 GB"), (4096, "4 GB")];
    let code_secs = &[(60, "1 minute"), (120, "2 minutes"), (180, "3 minutes"), (300, "5 minutes")];
    let clear_secs = &[(10, "After 10 seconds"), (30, "After 30 seconds"), (60, "After 1 minute"), (120, "After 2 minutes")];
    let separators = json!([["newline", "A new line"], ["blank", "A blank line"], ["space", "A space"], ["comma", "A comma"], ["tab", "A tab"]]);
    let actions: Vec<Value> = manifests()
        .into_iter()
        .map(|m| {
            let what = match (&m.dbus, m.exec.first()) {
                (Some(d), _) => format!("Calls {}", d.bus),
                (None, Some(p)) => format!("Runs {p}, which gets the item's text"),
                _ => String::new(),
            };
            json!({"kind": "switch", "key": format!("approve:{}", m.id), "title": m.label, "desc": what, "value": s.approved_actions.contains(&m.id)})
        })
        .collect();
    let mut groups = vec![
        json!({"title": "Clipboard", "rows": [
            {"kind": "info", "key": "status", "title": "History", "value": state},
            {"kind": "switch", "key": "private", "title": "Private mode", "desc": "Stop saving what you copy until you turn this off. Also Ctrl + Shift + P in the popup.", "value": s.private, "refresh": true},
            {"kind": "entry", "key": "shortcut", "title": "Shortcut", "desc": "Opens the popup. Written as Hyprland keys, like SUPER + CTRL + V.", "value": s.shortcut, "placeholder": "SUPER + CTRL + V"},
            {"kind": "segmented", "key": "position", "title": "Open", "desc": "Where the popup appears.", "value": if s.position == Position::Cursor { "cursor" } else { "center" }, "options": [["center", "In the middle"], ["cursor", "At the pointer"]]},
            {"kind": "choice", "key": "separator", "title": "Between items pasted together", "desc": "What goes between several marked items.", "value": separator_id(&s.separator), "options": separators},
        ]}),
        json!({"title": "History", "rows": [
            {"kind": "choice", "key": "max_entries", "title": "Keep", "desc": "Older items go first. Pinned items always stay.", "value": s.max_entries.to_string(), "options": opts(entries)},
            {"kind": "choice", "key": "max_age_days", "title": "For", "desc": "Items you haven't used for this long are removed.", "value": s.max_age_days.to_string(), "options": opts(days)},
            {"kind": "choice", "key": "max_item_mb", "title": "Largest item", "desc": "Bigger copies aren't saved.", "value": s.max_item_mb.to_string(), "options": opts(item)},
            {"kind": "choice", "key": "max_total_mb", "title": "Space used", "desc": "The oldest items go when the history grows past this.", "value": s.max_total_mb.to_string(), "options": opts(total)},
            {"kind": "switch", "key": "images", "title": "Save images", "desc": "Screenshots and copied pictures.", "value": s.images},
            {"kind": "button", "key": "clear", "title": "Clear history", "desc": "Remove everything except pinned items.", "label": "Clear", "confirm": true, "destructive": true, "refresh": true},
            {"kind": "button", "key": "clear_all", "title": "Clear everything", "desc": "Also remove pinned items.", "label": "Clear all", "confirm": true, "destructive": true, "refresh": true},
        ]}),
        json!({"title": "Privacy", "rows": [
            {"kind": "switch", "key": "skip_secrets", "title": "Don't save passwords and keys", "desc": "Copies that look like passwords, access tokens or private keys are kept in memory for two minutes and never saved. Password managers' copies are never saved either way.", "value": s.skip_secrets},
            {"kind": "entry", "key": "excluded_apps", "title": "Don't save copies from", "desc": "Window classes, separated by commas (hyprctl clients shows them).", "value": s.excluded_apps.join(", "), "placeholder": "1password, org.keepassxc.KeePassXC"},
        ]}),
        json!({"title": "Verification codes", "note": "Codes in notifications (\u{201c}Your code is 483920\u{201d}) appear at the top of the popup while they're valid, and Passwords' browser extension can fill them in. They're kept only in memory, never saved, and never put on the clipboard unless you copy one.", "rows": [
            {"kind": "switch", "key": "codes", "title": "Find codes in notifications", "value": s.codes},
            {"kind": "choice", "key": "code_seconds", "title": "Keep a code for", "value": s.code_seconds.to_string(), "options": opts(code_secs)},
            {"kind": "choice", "key": "code_clear_seconds", "title": "Clear a copied code", "desc": "Empty the clipboard this long after copying a code, if it still holds it.", "value": s.code_clear_seconds.to_string(), "options": opts(clear_secs)},
        ]}),
        json!({"title": "Assistant", "rows": [
            {"kind": "entry", "key": "translate_to", "title": "Translate into", "desc": "The language the Translate action uses. Assistant actions run on this computer, only when you pick one.", "value": s.translate_to},
        ]}),
    ];
    if !actions.is_empty() {
        groups.push(json!({"title": "Actions from other apps", "note": "Actions added to the actions folder work only once you turn them on.", "rows": actions}));
    }
    json!({ "poll": 10, "banners": banners, "groups": groups })
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

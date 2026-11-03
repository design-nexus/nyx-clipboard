//! Putting things on the clipboard (served by this process, so they outlive the popup),
//! clearing a secret later if it's still there, and pasting into the focused window.

use nyx_clipboard_core::classify::{self, TEXT};
use nyx_clipboard_core::model::Rep;
use std::io::Read;
use std::time::Duration;
use wl_clipboard_rs::copy::{self, ClipboardType, MimeSource, MimeType, Options, Seat, Source};
use wl_clipboard_rs::paste;

/// Marks copies this service made from a history entry, so the watcher only moves that
/// entry up instead of storing it again. Its value is the entry id.
pub const MARKER: &str = "application/x-nyx-clipboard-entry";

/// Offer `reps` (best first). `entry` adds the marker; `sensitive` the password-manager
/// hint, so clipboard managers (this one included) don't keep it.
pub fn set(reps: &[Rep], entry: Option<i64>, sensitive: bool) -> Result<(), String> {
    if reps.is_empty() {
        return Err("nothing to copy".into());
    }
    let mut sources: Vec<MimeSource> = reps
        .iter()
        .map(|r| MimeSource {
            source: Source::Bytes(r.data.clone().into_boxed_slice()),
            mime_type: if r.mime == TEXT { MimeType::Text } else { MimeType::Specific(r.mime.clone()) },
        })
        .collect();
    // Files: also offer the GNOME type file managers paste from.
    if let Some(list) = reps.iter().find(|r| r.mime == classify::URI_LIST)
        && !reps.iter().any(|r| r.mime == classify::GNOME_FILES)
    {
        let mut g = b"copy\n".to_vec();
        let uris: Vec<&str> = std::str::from_utf8(&list.data).unwrap_or("").lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        g.extend_from_slice(uris.join("\n").as_bytes());
        sources.push(MimeSource { source: Source::Bytes(g.into_boxed_slice()), mime_type: MimeType::Specific(classify::GNOME_FILES.into()) });
    }
    if let Some(id) = entry {
        sources.push(MimeSource {
            source: Source::Bytes(id.to_string().into_bytes().into_boxed_slice()),
            mime_type: MimeType::Specific(MARKER.into()),
        });
    }
    let mut opts = Options::new();
    opts.clipboard(ClipboardType::Regular).seat(Seat::All).sensitive(sensitive);
    opts.copy_multi(sources).map_err(|e| e.to_string())
}

pub fn set_text(text: &str, sensitive: bool) -> Result<(), String> {
    set(&[Rep { mime: TEXT.into(), data: text.as_bytes().to_vec() }], None, sensitive)
}

/// Clears the clipboard if its text is still exactly `value` (something copied since
/// is left alone). Blocking.
pub fn clear_if_holds(value: &[u8]) -> bool {
    let Ok((pipe, _)) = paste::get_contents(paste::ClipboardType::Regular, paste::Seat::Unspecified, paste::MimeType::Text) else {
        return false;
    };
    let mut current = zeroize::Zeroizing::new(Vec::new());
    if pipe.take(4096).read_to_end(&mut current).is_err() {
        return false;
    }
    let same = current.len() == value.len() && current.iter().zip(value).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
    same && copy::clear(ClipboardType::Regular, Seat::All).is_ok()
}

/// Whether the focused window is a terminal (Omarchy tags them), which pastes with
/// Ctrl + Shift + V.
fn focused_is_terminal(hypr: &nyx_desktop::hypr::Instance) -> bool {
    let Ok(w) = hypr.json::<serde_json::Value>("activewindow") else { return false };
    w.get("tags").and_then(|t| t.as_array()).is_some_and(|tags| {
        tags.iter().filter_map(|t| t.as_str()).any(|t| t.trim_end_matches('*') == "terminal")
    })
}

fn send_keys(hypr: &nyx_desktop::hypr::Instance, mods: &str, state: &str) -> bool {
    let lua = format!("hl.dispatch(hl.dsp.send_key_state({{ mods = \"{mods}\", key = \"V\", state = \"{state}\" }}))");
    hypr.request(&format!("eval {lua}")).is_ok_and(|r| r.trim() == "ok")
}

/// Paste into the focused window after `delay` (the popup must be gone first): the
/// focused surface gets Ctrl + V (Ctrl + Shift + V in a terminal) from Hyprland, as
/// Omarchy's universal paste does. Falls back to wtype outside Hyprland.
pub fn paste_later(delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        if let Ok(hypr) = nyx_desktop::hypr::Instance::find() {
            let mods = if focused_is_terminal(&hypr) { "CTRL SHIFT" } else { "CTRL" };
            if send_keys(&hypr, mods, "down") {
                std::thread::sleep(Duration::from_millis(50));
                send_keys(&hypr, mods, "up");
                return;
            }
        }
        let _ = std::process::Command::new("wtype").args(["-M", "ctrl", "-k", "v", "-m", "ctrl"]).status();
    });
}

/// The focused window's class, for app exclusions.
pub fn focused_class() -> Option<String> {
    let hypr = nyx_desktop::hypr::Instance::find().ok()?;
    let w = hypr.json::<serde_json::Value>("activewindow").ok()?;
    w.get("class").and_then(|c| c.as_str()).filter(|c| !c.is_empty()).map(str::to_string)
}

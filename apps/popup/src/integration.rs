//! The shortcut: a block Clipboard owns at the end of `~/.config/hypr/hyprland.lua`,
//! as Glimpse does. Only the exact block written last is ever replaced or removed; a
//! block edited by hand is left alone. Hyprland reloads, and a configuration it rejects
//! is rolled back.

use nyx_clipboard_core::paths;
use std::path::{Path, PathBuf};
use std::process::Command;

const START: &str = "-- BEGIN NYX CLIPBOARD";
const END: &str = "-- END NYX CLIPBOARD";

pub fn block(key: &str) -> String {
    format!(
        "{START}\n-- Owned by Clipboard. Change the key in Settings → Desktop → Clipboard.\n\
         o.rebind(\"{key}\", \"Clipboard\", \"nyx-clipboard --toggle\")\n{END}\n"
    )
}

/// `text` with the owned block replaced by `new` (removed when `None`). `old` is the
/// block as last written; anything else between the markers is refused.
pub fn edit(text: &str, old: Option<&str>, new: Option<&str>) -> Result<String, String> {
    let starts = text.matches(START).count();
    let ends = text.matches(END).count();
    if starts == 0 && ends == 0 {
        return Ok(match new {
            Some(s) => format!("{}{}{}", text, if text.ends_with('\n') || text.is_empty() { "" } else { "\n" }, s),
            None => text.into(),
        });
    }
    if starts != 1 || ends != 1 {
        return Err("hyprland.lua has more than one Clipboard block".into());
    }
    let start = text.find(START).unwrap_or(0);
    let end = text.find(END).map(|e| e + END.len()).unwrap_or(0);
    if start >= end {
        return Err("the Clipboard block in hyprland.lua is malformed".into());
    }
    let end = if text.as_bytes().get(end) == Some(&b'\n') { end + 1 } else { end };
    match old {
        Some(o) if &text[start..end] == o => {}
        _ => return Err("the Clipboard block in hyprland.lua was edited by hand; leaving it alone".into()),
    }
    Ok(format!("{}{}{}", &text[..start], new.unwrap_or(""), &text[end..]))
}

fn hypr_file() -> PathBuf {
    paths::config_home().join("hypr/hyprland.lua")
}

fn owned_file() -> PathBuf {
    paths::state_home().join("nyx-clipboard/hypr-block.lua")
}

fn reload_ok() -> Result<(), String> {
    let _ = Command::new("hyprctl").arg("reload").output();
    let out = Command::new("hyprctl").arg("configerrors").output().map_err(|e| e.to_string())?;
    let s = String::from_utf8_lossy(&out.stdout).to_string();
    if !s.trim().is_empty() && !s.to_lowercase().contains("no errors") {
        return Err(format!("Hyprland reports: {}", s.trim()));
    }
    Ok(())
}

fn write_keeping_mode(file: &Path, text: &str) -> Result<(), String> {
    let mode = std::fs::metadata(file).map_err(|e| e.to_string())?.permissions();
    let tmp = file.with_extension(format!("nyx-clipboard-{}", std::process::id()));
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::set_permissions(&tmp, mode).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, file).map_err(|e| e.to_string())
}

/// Bind `key` (None removes the binding, giving the key back to Omarchy).
pub fn apply(key: Option<&str>) -> Result<(), String> {
    let file = hypr_file();
    let original = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let owned = owned_file();
    let old = std::fs::read_to_string(&owned).ok();
    let new = key.map(block);
    let changed = edit(&original, old.as_deref(), new.as_deref())?;
    if changed == original {
        return Ok(());
    }
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let backup = paths::state_home().join(format!("nyx-clipboard/hyprland-{stamp}.lua"));
    paths::atomic_write(&backup, original.as_bytes()).map_err(|e| e.to_string())?;
    write_keeping_mode(&file, &changed)?;
    if let Err(e) = reload_ok() {
        write_keeping_mode(&file, &original)?;
        let _ = reload_ok();
        return Err(format!("{e} (the change was undone)"));
    }
    match new {
        Some(n) => paths::atomic_write(&owned, n.as_bytes()).map_err(|e| e.to_string())?,
        None => {
            let _ = std::fs::remove_file(owned);
        }
    }
    Ok(())
}

/// A key as Omarchy writes it: `SUPER + CTRL + V`.
pub fn valid_key(key: &str) -> bool {
    let parts: Vec<&str> = key.split('+').map(str::trim).collect();
    !parts.is_empty()
        && parts.len() <= 5
        && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owns_only_its_block() {
        let b = block("SUPER + CTRL + V");
        let added = edit("a\n", None, Some(&b)).unwrap();
        assert_eq!(added, format!("a\n{b}"));
        let changed = edit(&added, Some(&b), Some(&block("SUPER + ALT + V"))).unwrap();
        assert!(changed.contains("SUPER + ALT + V") && !changed.contains("SUPER + CTRL + V"));
        assert_eq!(edit(&added, Some(&b), None).unwrap(), "a\n");
        let hand = added.replace("nyx-clipboard --toggle", "something-else");
        assert!(edit(&hand, Some(&b), None).is_err());
        assert!(valid_key("SUPER + CTRL + V") && !valid_key("SUPER + \"; os.execute"));
    }
}

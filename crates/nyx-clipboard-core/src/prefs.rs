//! `~/.config/nyx-clipboard/settings.toml`. Written by the popup and the Settings page,
//! read by the daemon whenever the file changes.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    /// The middle of the screen with the pointer.
    #[default]
    Center,
    /// At the pointer.
    Cursor,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    /// Unpinned entries kept, newest first (0: no limit).
    pub max_entries: u32,
    /// Days unpinned entries are kept (0: until the entry limit removes them).
    pub max_age_days: u32,
    /// The largest copy kept, in MiB.
    pub max_item_mb: u32,
    /// All stored copies together, in MiB.
    pub max_total_mb: u32,
    /// Nothing new is recorded.
    pub private: bool,
    /// Apps (Hyprland window classes, case-insensitive) whose copies aren't recorded.
    pub excluded_apps: Vec<String>,
    /// Keep probable passwords and tokens out of the history.
    pub skip_secrets: bool,
    /// Record images.
    pub images: bool,
    pub position: Position,
    /// Between items in a multi-item paste.
    pub separator: String,
    /// Watch notifications for verification codes.
    pub codes: bool,
    /// Seconds a verification code stays available.
    pub code_seconds: u32,
    /// Seconds before a copied code is cleared from the clipboard (if it's still there).
    pub code_clear_seconds: u32,
    /// Ids of third-party action manifests the user approved.
    pub approved_actions: Vec<String>,
    /// Language the Assistant translates into.
    pub translate_to: String,
    /// The Hyprland shortcut, as Omarchy writes keys (`SUPER + CTRL + V`).
    pub shortcut: String,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            max_entries: 1000,
            max_age_days: 30,
            max_item_mb: 10,
            max_total_mb: 512,
            private: false,
            excluded_apps: ["1password", "bitwarden", "org.keepassxc.keepassxc", "keepassxc", "io.github.design_nexus.nyx.passwords"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            skip_secrets: true,
            images: true,
            position: Position::Center,
            separator: "\n".into(),
            codes: true,
            code_seconds: 180,
            code_clear_seconds: 30,
            approved_actions: Vec::new(),
            translate_to: "English".into(),
            shortcut: "SUPER + CTRL + V".into(),
        }
    }
}

impl Settings {
    pub fn load_from(path: &Path) -> Settings {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str::<Settings>(&t).ok()).unwrap_or_default().clamped()
    }

    pub fn load() -> Settings {
        Self::load_from(&crate::paths::settings_file())
    }

    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        let text = toml::to_string_pretty(&self.clone().clamped()).map_err(std::io::Error::other)?;
        crate::paths::atomic_write(path, text.as_bytes())
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&crate::paths::settings_file())
    }

    /// Values inside their allowed ranges.
    pub fn clamped(mut self) -> Settings {
        self.max_entries = self.max_entries.min(100_000);
        self.max_age_days = self.max_age_days.min(3650);
        self.max_item_mb = self.max_item_mb.clamp(1, 256);
        self.max_total_mb = self.max_total_mb.clamp(16, 16_384);
        self.code_seconds = self.code_seconds.clamp(30, 300);
        self.code_clear_seconds = self.code_clear_seconds.clamp(5, 120);
        if self.separator.len() > 16 {
            self.separator = "\n".into();
        }
        self
    }

    /// Whether copies from `class` aren't recorded.
    pub fn excludes(&self, class: &str) -> bool {
        let c = class.to_lowercase();
        !c.is_empty() && self.excluded_apps.iter().any(|e| {
            let e = e.to_lowercase();
            c == e || c.ends_with(&format!(".{e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_ranges() {
        let dir = std::env::temp_dir().join(format!("nyx-clipboard-prefs-{}", std::process::id()));
        let file = dir.join("settings.toml");
        let s = Settings { max_entries: 5, code_seconds: 9999, ..Default::default() };
        s.save_to(&file).unwrap();
        let back = Settings::load_from(&file);
        assert_eq!(back.max_entries, 5);
        assert_eq!(back.code_seconds, 300);
        assert!(back.excludes("1Password"));
        assert!(back.excludes("org.keepassxc.KeePassXC"));
        assert!(!back.excludes("firefox"));
        std::fs::write(&file, "max_entries = \"nope\"").unwrap();
        assert_eq!(Settings::load_from(&file), Settings::default());
        let _ = std::fs::remove_dir_all(dir);
    }
}

//! Where Nyx Clipboard keeps things (XDG directories respected).

use std::path::{Path, PathBuf};

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

pub fn config_home() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config")
}

pub fn data_home() -> PathBuf {
    xdg("XDG_DATA_HOME", ".local/share")
}

pub fn state_home() -> PathBuf {
    xdg("XDG_STATE_HOME", ".local/state")
}

pub fn cache_home() -> PathBuf {
    xdg("XDG_CACHE_HOME", ".cache")
}

/// `~/.config/nyx-clipboard`.
pub fn config_dir() -> PathBuf {
    config_home().join("nyx-clipboard")
}

pub fn settings_file() -> PathBuf {
    config_dir().join("settings.toml")
}

/// `~/.local/share/nyx/clipboard.db`, beside the suite's other stores.
pub fn database() -> PathBuf {
    data_home().join("nyx").join("clipboard.db")
}

/// `~/.local/share/nyx-clipboard`: stored images and other large copies.
pub fn data_dir() -> PathBuf {
    data_home().join("nyx-clipboard")
}

pub fn blobs_dir() -> PathBuf {
    data_dir().join("blobs")
}

/// Third-party action manifests.
pub fn actions_dir() -> PathBuf {
    data_dir().join("actions")
}

/// Scaled previews of images, made by the popup.
pub fn thumbs_dir() -> PathBuf {
    cache_home().join("nyx-clipboard").join("thumbs")
}

/// Omarchy's clipboard history (imported once).
pub fn omarchy_history() -> PathBuf {
    state_home().join("omarchy").join("clipboard-history.json")
}

/// A private folder (0700), created if missing.
pub fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    if !dir.exists() {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    }
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Write a file so readers never see half of it; owner-only.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let dir = path.parent().ok_or_else(|| std::io::Error::other("no parent folder"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{}.{}.tmp", path.file_name().and_then(|n| n.to_str()).unwrap_or("file"), std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

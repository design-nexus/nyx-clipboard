//! Clipboard sync with devices paired in Flux (Nyx Connect), when it's turned on in
//! Settings › Devices (`clipboard_sync` in connect.toml).
//!
//! fluxd's own sync is off then, so every copy goes through this service's filters
//! first: private mode, password-manager copies, excluded apps and secrets never leave
//! the computer. What's kept is sent to each connected device whose clipboard access is
//! on. A device's copy is put on the clipboard with the [`clip::REMOTE`] marker: the
//! watcher records where it came from and doesn't send it back.

use crate::clip;
use crate::state::State;
use nyx_clipboard_core::classify::TEXT;
use nyx_clipboard_core::model::Rep;
use nyx_connect::{Access, Client, ClipEntry, Event, Feature, Prefs, Snapshot, Watch};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// What fluxd sends to a device at most.
const MAX_TEXT: usize = 256 << 10;
/// A copy that arrived from a device this recently isn't sent back out.
const ECHO: Duration = Duration::from_secs(30);

#[derive(Default)]
struct Sync {
    /// The latest snapshot, while fluxd runs.
    last: Option<Arc<Snapshot>>,
    /// fluxd's clipboard entries already handled.
    seen: HashSet<String>,
    /// What was last sent to each device, so it isn't taken back when the device echoes it.
    sent: HashMap<String, u64>,
    /// Text that just arrived from a device, and when.
    arrived: Option<(u64, Instant)>,
}

fn sync() -> &'static Mutex<Sync> {
    static SYNC: OnceLock<Mutex<Sync>> = OnceLock::new();
    SYNC.get_or_init(Mutex::default)
}

fn lock() -> std::sync::MutexGuard<'static, Sync> {
    sync().lock().unwrap_or_else(|e| e.into_inner())
}

fn hash(text: &str) -> u64 {
    text.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Follow fluxd for as long as the service runs.
pub fn start(st: Arc<State>) -> Watch {
    let client = Client::new();
    let mut first = true;
    Watch::start(move |event| match event {
        Event::State(snap) => {
            let prefs = Prefs::load();
            nyx_connect::keep_flux_sync_off(&client, &snap, &prefs);
            let new = {
                let mut s = lock();
                s.last = Some(snap.clone());
                let new: Vec<ClipEntry> = snap.clipboard.iter().filter(|e| !s.seen.contains(&e.id)).cloned().collect();
                s.seen = snap.clipboard.iter().map(|e| e.id.clone()).collect();
                new
            };
            // What fluxd already had when this service started stays there.
            if std::mem::take(&mut first) || !prefs.clipboard_sync {
                return;
            }
            // fluxd lists the newest first; only the newest copy goes on the clipboard.
            if let Some(entry) = new.iter().find(|e| e.is_incoming_copy() && snap.allowed(&e.device, Access::Clipboard)) {
                receive(&st, &client, &snap, entry);
            }
        }
        Event::Unavailable(_) => lock().last = None,
        Event::Toast(_) => {}
    })
}

fn receive(st: &State, client: &Client, snap: &Snapshot, entry: &ClipEntry) {
    let name = snap.device(&entry.device).map(|d| d.name.clone()).unwrap_or_else(|| entry.device_name.clone());
    let reps = if entry.image.is_empty() {
        let Ok(text) = client.clip_text(entry) else { return };
        let mut s = lock();
        // The device echoing what was just sent to it.
        if s.sent.get(&entry.device) == Some(&hash(&text)) {
            return;
        }
        s.arrived = Some((hash(&text), Instant::now()));
        drop(s);
        vec![Rep { mime: TEXT.into(), data: text.into_bytes() }]
    } else {
        if !st.settings().images {
            return;
        }
        let Some(rep) = image(&entry.image) else { return };
        vec![rep]
    };
    if let Err(e) = clip::set_remote(&reps, &entry.device, &name) {
        eprintln!("nyx-clipboardd: can't put a copy from a device on the clipboard: {e}");
    }
}

/// An image fluxd saved from a device's clipboard (in its runtime folder).
fn image(path: &str) -> Option<Rep> {
    let path = std::path::Path::new(path);
    let flux_dir = nyx_connect::socket_path().parent()?.to_path_buf();
    if !path.starts_with(&flux_dir) || std::fs::symlink_metadata(path).ok()?.file_type().is_symlink() {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let mime = match data.get(..12)? {
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [b'G', b'I', b'F', b'8', ..] => "image/gif",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P'] => "image/webp",
        _ => return None,
    };
    Some(Rep { mime: mime.into(), data })
}

/// What was copied here, to send.
pub enum Copied {
    Text(String),
    /// fluxd reads the image off the clipboard itself.
    Image,
}

/// A copy this service kept (it passed every filter): send it to the devices that may
/// have it, except `from`, the device it came from. Runs off the watcher's thread.
pub fn copied(what: Copied, from: Option<String>) {
    let Some(snap) = lock().last.clone() else { return };
    if !Prefs::load().clipboard_sync {
        return;
    }
    if let Copied::Text(text) = &what {
        if text.len() > MAX_TEXT || text.trim().is_empty() {
            return;
        }
        // A device's copy that fluxd put on the clipboard itself (one sent by hand).
        let h = hash(text);
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        let s = lock();
        if s.arrived.is_some_and(|(a, at)| a == h && at.elapsed() < ECHO)
            || snap
                .clipboard
                .iter()
                .any(|e| e.dir == "in" && now - e.time < ECHO.as_secs() as i64 && !e.truncated && e.text == *text)
        {
            return;
        }
    }
    let targets: Vec<String> = snap
        .paired()
        .filter(|d| d.online && d.has(Feature::Clipboard) && snap.allowed(&d.id, Access::Clipboard))
        .filter(|d| from.as_deref() != Some(d.id.as_str()))
        .map(|d| d.id.clone())
        .collect();
    if targets.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        let client = Client::new();
        for device in targets {
            let text = match &what {
                Copied::Text(t) => t.as_str(),
                Copied::Image => "",
            };
            if client.send_clipboard(&device, text).is_ok()
                && let Copied::Text(t) = &what
            {
                lock().sent.insert(device, hash(t));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_images_in_fluxds_folder_are_read() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("clip-1.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\n0000").unwrap();
        assert!(image(png.to_str().unwrap()).is_none(), "outside fluxd's runtime folder");
    }

    #[test]
    fn a_remote_marker_names_a_device() {
        assert_eq!(clip::remote_origin(b"a1b2-c3_d4 My Pixel"), Some(("a1b2-c3_d4".to_string(), "My Pixel".to_string())));
        assert_eq!(clip::remote_origin(b"bad/id name"), None);
        assert_eq!(clip::remote_origin(b""), None);
    }
}

//! Watching the clipboard through data-control (ext-data-control-v1, or
//! wlr-data-control): the compositor tells us when the selection changes, so nothing
//! polls. Each new copy is read (bounded in size and time), classified, checked against
//! private mode, excluded apps and secrets, and stored.

use crate::clip;
use crate::state::State;
use nyx_clipboard_core::classify::{self, classify};
use nyx_clipboard_core::model::Rep;
use nyx_clipboard_core::{now, sensitive};
use std::io::Read;
use std::os::fd::AsRawFd;
use std::sync::Arc;
use std::time::{Duration, Instant};
use wl_clipboard_rs::paste::Seat;
use wl_clipboard_rs::watch::{ClipboardEvent, ClipboardType, Watcher};

/// How long a copying app gets to hand over each representation.
const READ_TIMEOUT: Duration = Duration::from_secs(2);

pub fn run(st: Arc<State>) {
    let mut backoff = Duration::from_secs(2);
    loop {
        match Watcher::new(ClipboardType::Regular, Seat::Unspecified) {
            Ok(mut w) => {
                backoff = Duration::from_secs(2);
                set_watching(&st, true, None);
                loop {
                    match w.next_event() {
                        Ok(Some(ClipboardEvent::Changed { mime_types, mut offer, .. })) => {
                            let read = |mime: &str, max: usize| -> Option<Vec<u8>> {
                                let pipe = offer.receive(mime).ok()?;
                                read_bounded(pipe, max)
                            };
                            if let Some(changed) = take(&st, &mime_types, read)
                                && changed {
                                    st.changed();
                                }
                        }
                        Ok(Some(ClipboardEvent::Cleared { .. })) => {}
                        Ok(None) => return,
                        Err(e) => {
                            eprintln!("nyx-clipboardd: clipboard connection lost: {e}");
                            break;
                        }
                    }
                }
            }
            Err(e) => {
                set_watching(&st, false, Some(e.to_string()));
                // Logged once per kind of failure, not on every retry.
                eprintln!("nyx-clipboardd: can't watch the clipboard: {e}");
            }
        }
        set_watching(&st, false, Some("reconnecting to the compositor".into()));
        std::thread::sleep(backoff);
        backoff = (backoff * 2).min(Duration::from_secs(30));
    }
}

fn set_watching(st: &State, on: bool, problem: Option<String>) {
    if let Ok(mut w) = st.watching.lock() {
        *w = (on, problem);
    }
}

/// Read a pipe to its end, giving up past `max` bytes or the timeout.
fn read_bounded(mut pipe: impl Read + AsRawFd, max: usize) -> Option<Vec<u8>> {
    let fd = pipe.as_raw_fd();
    // SAFETY: fcntl on a descriptor we own.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFL);
        libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK);
    }
    let deadline = Instant::now() + READ_TIMEOUT;
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut pfd, 1, left.as_millis().min(i32::MAX as u128) as i32) };
        if r == 0 {
            return None;
        }
        if r < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return None;
        }
        match pipe.read(&mut buf) {
            Ok(0) => return Some(out),
            Ok(n) => {
                if out.len() + n > max {
                    return None;
                }
                out.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
}

/// Handle one new selection, reading representations through `read(mime, max)`.
/// Returns whether listeners should be told (None: nothing happened).
pub fn take(st: &State, offered: &[String], mut read: impl FnMut(&str, usize) -> Option<Vec<u8>>) -> Option<bool> {
    let settings = st.settings();
    if settings.private || offered.is_empty() {
        return None;
    }
    // A password manager said "don't keep this": not even read.
    if classify::has_password_hint(offered) {
        return None;
    }
    // One of ours, from a history entry: just move it up.
    if offered.iter().any(|m| m == clip::MARKER) {
        let id = read(clip::MARKER, 32).and_then(|d| String::from_utf8(d).ok()).and_then(|s| s.trim().parse::<i64>().ok());
        if let Some(id) = id {
            return st.store.lock().ok().map(|s| s.touch(id, now()).unwrap_or(false));
        }
    }
    let app = clip::focused_class();
    if app.as_deref().is_some_and(|a| settings.excludes(a)) {
        return None;
    }
    let max = settings.max_item_mb as usize * 1024 * 1024;
    let mut reps = Vec::new();
    for (offered_type, stored_type) in classify::plan(offered) {
        if stored_type.starts_with("image/") && !settings.images {
            continue;
        }
        // Text first: a secret is caught before anything else is read.
        let limit = if stored_type.starts_with("image/") { max } else { max.min(16 * 1024 * 1024) };
        if let Some(data) = read(&offered_type, limit) {
            if stored_type == classify::TEXT && settings.skip_secrets {
                let text = classify::decode_text(&data);
                if let Some(label) = sensitive::detect(&text) {
                    st.add_secret(label, text);
                    return Some(false);
                }
            }
            reps.push(Rep { mime: stored_type, data });
        }
    }
    let captured = classify(reps)?;
    if captured.size() > max {
        return None;
    }
    let mut store = st.store.lock().ok()?;
    let inserted = match store.insert(&captured, app.as_deref(), now()) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("nyx-clipboardd: can't store a copy: {e}");
            return None;
        }
    };
    if inserted.new {
        let _ = store.prune(&settings, now());
    }
    Some(true)
}

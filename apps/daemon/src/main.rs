//! nyx-clipboardd: the Nyx Clipboard service.
//!
//! - Watches the Wayland clipboard (data-control; no polling) and keeps the history in
//!   SQLite, skipping private mode, excluded apps, password-manager copies and secrets.
//! - Syncs what it keeps with devices paired in Flux, when that's on (connect.rs).
//! - Serves `io.github.design_nexus.nyx.Clipboard1` (history, snippets, paste),
//!   `…Clipboard.Actions1` (contextual actions) and `…Clipboard.Otp1` (verification
//!   codes, held in locked memory and never written anywhere) to callers it can
//!   identify (docs/API.md).
//! - Prunes the history by the retention settings, forgets codes when they expire, the
//!   session locks or the computer sleeps, and wipes them on exit.

mod actions;
mod apps;
mod clip;
mod codes;
mod connect;
mod control;
mod service;
mod state;
mod watch;

use nyx_clipboard_core::classify::{self, classify};
use nyx_clipboard_core::model::Rep;
use nyx_clipboard_core::{now, omarchy, paths, secret, sensitive};
use nyx_clipboard_dbus::{BUS, PATH};
use nyx_clipboard_store::Store;
use state::State;
use std::sync::Arc;
use std::time::Duration;
use zbus::fdo;

/// Run blocking work (SQLite, the Assistant, Capture) off the bus's executor.
pub async fn offload<T: Send + 'static>(f: impl FnOnce() -> fdo::Result<T> + Send + 'static) -> fdo::Result<T> {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx.recv().await.unwrap_or_else(|_| Err(fdo::Error::Failed("the action stopped unexpectedly".into())))
}

/// Bring in Omarchy's clipboard history once, oldest first so the order is kept.
fn import_omarchy(st: &State) {
    let Ok(store) = st.store.lock() else { return };
    if store.meta("omarchy_imported").is_some() {
        return;
    }
    drop(store);
    let settings = st.settings();
    let items = omarchy::read(&paths::omarchy_history());
    let t = now();
    let mut n = 0;
    let count = items.len();
    for (i, item) in items.into_iter().rev().enumerate() {
        let reps = match item {
            omarchy::Item::Text(text) => {
                if settings.skip_secrets && sensitive::detect(&text).is_some() {
                    continue;
                }
                vec![Rep { mime: classify::TEXT.into(), data: text.into_bytes() }]
            }
            omarchy::Item::Image { path, mime } => {
                let ok = std::fs::metadata(&path).is_ok_and(|m| m.len() <= u64::from(settings.max_item_mb) * 1024 * 1024);
                match std::fs::read(&path) {
                    Ok(data) if ok && settings.images => vec![Rep { mime, data }],
                    _ => continue,
                }
            }
        };
        let Some(c) = classify(reps) else { continue };
        // Spread over the seconds before now, in Omarchy's order.
        let when = t - (count - i) as f64;
        if let Ok(mut s) = st.store.lock()
            && s.insert(&c, None, when).is_ok_and(|r| r.new)
        {
            n += 1;
        }
    }
    if let Ok(mut s) = st.store.lock() {
        let _ = s.set_meta("omarchy_imported", &format!("{t}"));
        let _ = s.prune(&settings, t);
    }
    if n > 0 {
        eprintln!("nyx-clipboardd: brought in {n} entries from Omarchy's clipboard history");
        st.changed();
    }
}

/// Hourly retention, and forgetting secrets and codes as they expire.
fn maintain(st: Arc<State>, wake: std::sync::mpsc::Receiver<()>) {
    let mut last_prune = 0.0;
    loop {
        let t = now();
        if t - last_prune > 3600.0 {
            last_prune = t;
            let s = st.settings();
            if st.store.lock().map(|mut store| store.prune(&s, t).unwrap_or(0)).unwrap_or(0) > 0 {
                st.changed();
            }
        }
        let next_secret = st.expire_secrets();
        let (gone, next_code) = st.codes.expire();
        if gone {
            st.changed();
            if let Some(c) = st.conn.get() {
                let _ = c.emit_signal(None::<&str>, PATH, nyx_clipboard_dbus::CODES, "Changed", &());
            }
        }
        let next = [next_secret, next_code].into_iter().flatten().reduce(f64::min);
        let wait = next.map(|n| (n - now()).max(0.05)).unwrap_or(3600.0).min(3600.0);
        let _ = wake.recv_timeout(Duration::from_secs_f64(wait));
    }
}

/// Codes go when the session locks or the computer sleeps (logind).
fn forget_on_lock(st: Arc<State>) {
    let Ok(conn) = zbus::blocking::Connection::system() else { return };
    let rules = [
        "type='signal',interface='org.freedesktop.login1.Session',member='Lock'",
        "type='signal',interface='org.freedesktop.login1.Manager',member='PrepareForSleep'",
    ];
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    for r in rules {
        if let Ok(rule) = zbus::MatchRule::try_from(r) {
            let _ = dbus.add_match_rule(rule);
        }
    }
    let iter = zbus::blocking::MessageIterator::from(&conn);
    for msg in iter.flatten() {
        let h = msg.header();
        let member = h.member().map(|m| m.as_str().to_string()).unwrap_or_default();
        let going = match member.as_str() {
            "Lock" => true,
            "PrepareForSleep" => msg.body().deserialize::<bool>().unwrap_or(false),
            _ => false,
        };
        if going {
            let any = st.codes.clear();
            if let Ok(mut v) = st.secrets.lock() {
                v.clear();
            }
            if any {
                st.changed();
            }
        }
    }
}

/// Registered actions go when the app that added them leaves the bus.
fn drop_departed(st: Arc<State>, conn: zbus::blocking::Connection) {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    let Ok(changes) = dbus.receive_name_owner_changed() else { return };
    for c in changes {
        let Ok(args) = c.args() else { continue };
        if args.new_owner().is_none() && args.name().as_str().starts_with(':') {
            let gone = format!("@{}", args.name());
            if let Ok(mut reg) = st.registered.lock() {
                reg.retain(|(_, app)| !app.ends_with(&gone));
            }
        }
    }
}

/// SIGTERM and SIGINT wipe codes and secrets, then exit; SIGHUP reads settings again.
fn signals(st: Arc<State>) {
    // SAFETY: plain signal-set manipulation; the set was blocked in main before any thread.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        loop {
            let mut sig = 0;
            if libc::sigwait(&set, &mut sig) != 0 {
                continue;
            }
            if sig == libc::SIGHUP {
                st.reload();
                st.changed();
                continue;
            }
            st.codes.clear();
            if let Ok(mut v) = st.secrets.lock() {
                v.clear();
            }
            std::process::exit(0);
        }
    }
}

fn block_signals() {
    // SAFETY: as in `signals`; threads spawned later inherit the mask.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("nyx-clipboardd {}", env!("CARGO_PKG_VERSION"));
            return std::process::ExitCode::SUCCESS;
        }
        Some("--describe-actions") => {
            println!("{}", control::description());
            return std::process::ExitCode::SUCCESS;
        }
        Some("-h" | "--help") => {
            println!("Usage: nyx-clipboardd\n\nThe Nyx Clipboard service (normally a user service: nyx-clipboardd.service).");
            return std::process::ExitCode::SUCCESS;
        }
        Some(other) => {
            eprintln!("nyx-clipboardd: unknown option {other}");
            return std::process::ExitCode::FAILURE;
        }
        None => {}
    }
    secret::harden_process();
    block_signals();
    let (db, blobs) = Store::default_paths();
    let store = match Store::open(&db, &blobs) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("nyx-clipboardd: can't open {}: {e}", db.display());
            return std::process::ExitCode::FAILURE;
        }
    };
    let st = State::new(store);
    {
        let st = st.clone();
        std::thread::spawn(move || signals(st));
    }
    let built = zbus::blocking::connection::Builder::session()
        .and_then(|b| b.serve_at(PATH, service::Service { state: st.clone() }))
        .and_then(|b| b.serve_at(PATH, actions::Actions { state: st.clone() }))
        .and_then(|b| b.serve_at(PATH, codes::Otp { state: st.clone() }))
        .and_then(|b| b.serve_at(PATH, control::Control { state: st.clone() }))
        .and_then(|b| b.build());
    let conn = match built {
        Ok(c) => c,
        Err(e) => {
            eprintln!("nyx-clipboardd: session bus: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match conn.request_name_with_flags(BUS, zbus::fdo::RequestNameFlags::DoNotQueue.into()) {
        Ok(zbus::fdo::RequestNameReply::PrimaryOwner) => {}
        Ok(_) => {
            eprintln!("nyx-clipboardd: already running");
            return std::process::ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("nyx-clipboardd: {BUS}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    }
    let _ = st.conn.set(conn.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    if let Ok(mut w) = st.wake.lock() {
        *w = Some(tx);
    }
    {
        let st = st.clone();
        std::thread::spawn(move || {
            import_omarchy(&st);
            maintain(st, rx);
        });
    }
    {
        let st = st.clone();
        std::thread::spawn(move || forget_on_lock(st));
    }
    {
        let st = st.clone();
        let conn = conn.clone();
        std::thread::spawn(move || drop_departed(st, conn));
    }
    // Clipboard sync with devices paired in Flux, while it's on in Settings.
    let _devices = connect::start(st.clone());
    watch::run(st);
    std::process::ExitCode::FAILURE
}

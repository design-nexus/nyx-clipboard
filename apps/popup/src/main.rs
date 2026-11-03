//! Nyx Clipboard: the floating clipboard popup.
//!
//! `nyx-clipboard` starts it hidden (normally as a user service);
//! `nyx-clipboard --toggle [recent|pinned|snippets] | --show | --hide` opens or closes
//! the running one. `--only <output>` keeps a separate test copy on one output.
//! `--integrate` / `--unintegrate` set or remove the Hyprland shortcut;
//! `--list [query]` prints matching entries as JSON (no window).

mod client;
mod integration;
mod popup;
mod theme;
mod thumbs;

use gtk::prelude::*;
use gtk::{gio, glib};
use nyx_clipboard_core::prefs::Settings;
use std::cell::RefCell;
use std::rc::Rc;

pub const APP_ID: &str = "io.github.design_nexus.nyx.Clipboard";

const USAGE: &str = "Usage: nyx-clipboard [--toggle [recent|pinned|snippets] | --show | --hide] [--only <output>]
       nyx-clipboard --integrate [KEY] | --unintegrate
       nyx-clipboard --list [QUERY]

With no option, start (hidden). The options open or close the running popup;
--toggle with a view's name opens it on that view. --integrate binds the shortcut
(Settings' key, normally SUPER + CTRL + V) in Hyprland; --unintegrate gives the key
back to Omarchy. --list prints the history as JSON.
";

fn cli() -> Option<std::process::ExitCode> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ok = std::process::ExitCode::SUCCESS;
    let fail = std::process::ExitCode::FAILURE;
    match args.first().map(String::as_str) {
        Some("-h" | "--help") => {
            print!("{USAGE}");
            Some(ok)
        }
        Some("--version") => {
            println!("nyx-clipboard {}", env!("CARGO_PKG_VERSION"));
            Some(ok)
        }
        Some("--integrate") => {
            let mut settings = Settings::load();
            if let Some(k) = args.get(1) {
                if !integration::valid_key(k) {
                    eprintln!("nyx-clipboard: “{k}” isn't a key Hyprland takes (like SUPER + CTRL + V)");
                    return Some(fail);
                }
                settings.shortcut = k.clone();
                let _ = settings.save();
            }
            Some(match integration::apply(Some(&settings.shortcut)) {
                Ok(()) => {
                    println!("{} opens Clipboard", settings.shortcut);
                    ok
                }
                Err(e) => {
                    eprintln!("nyx-clipboard: {e}");
                    fail
                }
            })
        }
        Some("--unintegrate") => Some(match integration::apply(None) {
            Ok(()) => ok,
            Err(e) => {
                eprintln!("nyx-clipboard: {e}");
                fail
            }
        }),
        Some("--list") => {
            let text = args.get(1).cloned().unwrap_or_default();
            let r = nyx_clipboard_dbus::Client::connect()
                .and_then(|c| c.search(&nyx_clipboard_core::query::Search { text, ..Default::default() }));
            Some(match r {
                Ok(v) => {
                    println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
                    ok
                }
                Err(e) => {
                    eprintln!("nyx-clipboard: {e}");
                    fail
                }
            })
        }
        _ => None,
    }
}

fn main() -> std::process::ExitCode {
    if let Some(code) = cli() {
        return code;
    }
    // A test copy (`--only <output>`) is separate: its own id, never the running popup.
    let test = std::env::args().any(|a| a == "--only");
    let base = std::env::var("NYX_CLIPBOARD_APP_ID").unwrap_or_else(|_| APP_ID.to_string());
    let (id, mut flags) = if test {
        (format!("{base}.Test"), gio::ApplicationFlags::NON_UNIQUE)
    } else {
        (base, gio::ApplicationFlags::empty())
    };
    glib::set_prgname(Some(id.as_str()));
    flags |= gio::ApplicationFlags::HANDLES_COMMAND_LINE;
    let app = gtk::Application::builder().application_id(id).flags(flags).build();
    let popup: Rc<RefCell<Option<Rc<popup::Popup>>>> = Rc::default();
    app.connect_startup(|app| {
        theme::install();
        // Resident: closing the popup doesn't quit.
        std::mem::forget(app.hold());
    });
    let p = popup.clone();
    app.connect_command_line(move |app, cmd| {
        let args: Vec<String> = cmd.arguments().iter().skip(1).map(|a| a.to_string_lossy().to_string()).collect();
        let only = args.iter().position(|a| a == "--only").and_then(|i| args.get(i + 1)).cloned();
        if p.borrow().is_none() {
            *p.borrow_mut() = Some(popup::build(app, only));
        }
        let popup = p.borrow().clone().expect("built");
        if let Some(i) = args.iter().position(|a| a == "--toggle") {
            match args.get(i + 1).and_then(|a| popup::Tab::parse(a)) {
                Some(t) => popup.toggle_tab(t),
                None => popup.toggle(),
            }
        } else if args.iter().any(|a| a == "--show") {
            popup.show();
        } else if args.iter().any(|a| a == "--hide") {
            popup.hide();
        }
        glib::ExitCode::SUCCESS
    });
    let code = app.run();
    std::process::ExitCode::from(code.get())
}

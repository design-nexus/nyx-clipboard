//! The popup: a layer-shell overlay over the focused screen holding one solid card,
//! placed at the screen's centre or at the pointer. The history is on the left and the
//! chosen item in full on the right, with its buttons; its actions, an action's
//! result, the snippet editor and the shortcuts open in place of that preview. A click
//! beside the card closes it. Typing searches; arrows move; Enter pastes. It stays in
//! memory while hidden but listens to nothing until it's opened.

use crate::{client, thumbs};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use nyx_clipboard_core::actions::{Action, Group, Outcome};
use nyx_clipboard_core::model::{CodeInfo, Detail, Entry, Kind, Status, Transient};
use nyx_clipboard_core::now;
use nyx_clipboard_core::prefs::{Position, Settings};
use nyx_clipboard_core::query::{Search, View};
use nyx_clipboard_core::snippet::{Snippet, local};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

pub const WIDTH: i32 = 860;
const HEIGHT: i32 = 560;
const LIST_WIDTH: i32 = 380;
const PAGE: u32 = 50;
/// The most of an item's text the preview lays out.
const PREVIEW_CHARS: usize = 20_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Recent,
    Pinned,
    Snippets,
}

impl Tab {
    pub fn parse(s: &str) -> Option<Tab> {
        match s {
            "recent" => Some(Tab::Recent),
            "pinned" => Some(Tab::Pinned),
            "snippets" => Some(Tab::Snippets),
            _ => None,
        }
    }
}

/// The kind chips under the search field.
const KINDS: [(Option<Kind>, &str); 6] = [
    (None, "All"),
    (Some(Kind::Text), "Text"),
    (Some(Kind::Url), "Links"),
    (Some(Kind::Image), "Images"),
    (Some(Kind::Files), "Files"),
    (Some(Kind::Code), "Code"),
];

/// The actions pane's groups, in the order they're laid out (alternating columns).
const HEADINGS: [&str; 6] = ["Change it", "Send to", "Assistant, on this computer", "Keep", "Paste and copy", "From other apps"];

fn heading(g: Group) -> &'static str {
    match g {
        Group::Transform | Group::Convert => HEADINGS[0],
        Group::Send => HEADINGS[1],
        Group::Assistant => HEADINGS[2],
        Group::Organize => HEADINGS[3],
        Group::Paste => HEADINGS[4],
        Group::Apps => HEADINGS[5],
    }
}

/// What a list row stands for.
#[derive(Clone)]
enum Item {
    Code(CodeInfo),
    Secret(Transient),
    Entry(Entry),
    Snippet(Snippet),
}

/// What the result page is showing.
struct Result {
    text: String,
}

/// The chosen item in full, beside the list.
struct Preview {
    icon: gtk::Image,
    meta: gtk::Label,
    scroll: gtk::ScrolledWindow,
    body: gtk::Box,
    picture_box: gtk::ScrolledWindow,
    picture: gtk::Picture,
    facts: gtk::Grid,
    bar: gtk::Box,
    paste_label: gtk::Label,
    plain: gtk::Button,
    plain_label: gtk::Label,
    pin: gtk::Button,
    favorite: gtk::Button,
    edit: gtk::Button,
    actions: gtk::Button,
    actions_label: gtk::Label,
    /// A code's or a secret's countdown, and when it ends.
    left: RefCell<Option<(gtk::Label, f64)>>,
}

/// The chosen item's actions, in place of the preview: grouped, and filtered as you type.
struct Pane {
    field: gtk::SearchEntry,
    about: gtk::Label,
    scroll: gtk::ScrolledWindow,
    columns: [gtk::Box; 2],
    /// In keyboard order: the left column, then the right.
    items: RefCell<Vec<(Action, gtk::Button)>>,
    selected: Cell<Option<usize>>,
    subject: RefCell<Option<Item>>,
}

pub struct Popup {
    window: gtk::Window,
    card: gtk::Box,
    search: gtk::SearchEntry,
    tabs: Vec<(Tab, gtk::ToggleButton, gtk::Label)>,
    private: gtk::ToggleButton,
    kinds_row: gtk::Box,
    kinds: Vec<(Option<Kind>, gtk::ToggleButton)>,
    banner: gtk::Label,
    list_side: gtk::Box,
    side: gtk::Stack,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    hint: gtk::Label,
    toast: gtk::Label,
    preview: Preview,
    pane: Pane,
    result_title: gtk::Label,
    result_text: gtk::TextView,
    result_buttons: gtk::Box,
    editor_name: gtk::Entry,
    editor_body: gtk::TextView,
    target_title: gtk::Label,
    target: gtk::Entry,
    tab: Cell<Tab>,
    kind: Cell<Option<Kind>>,
    items: RefCell<Vec<Item>>,
    marked: RefCell<Vec<i64>>,
    details: RefCell<HashMap<i64, Detail>>,
    offset: Cell<u32>,
    exhausted: Cell<bool>,
    loading: Cell<bool>,
    generation: Cell<u64>,
    only: Option<String>,
    narrow: Cell<bool>,
    listening: RefCell<Vec<gio::SignalSubscription>>,
    ticker: RefCell<Option<glib::SourceId>>,
    toast_timer: RefCell<Option<glib::SourceId>>,
    pending_refresh: Cell<bool>,
    result: RefCell<Option<Result>>,
    editing: Cell<i64>,
    target_for: RefCell<Option<(i64, String)>>,
    armed: RefCell<Option<(String, glib::SourceId)>>,
    status: RefCell<Status>,
    me: RefCell<Weak<Popup>>,
}

fn act(fut: impl std::future::Future<Output = ()> + 'static) {
    glib::MainContext::default().spawn_local(fut);
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.add_css_class(class);
    l.set_xalign(0.0);
    l.set_ellipsize(gtk::pango::EllipsizeMode::End);
    l.set_max_width_chars(1);
    l.set_hexpand(true);
    l
}

fn icon(name: &str, px: i32) -> gtk::Image {
    let i = gtk::Image::from_icon_name(name);
    i.set_pixel_size(px);
    i
}

/// A button with an icon before its label.
fn icon_button(icon_name: &str, text: &str) -> (gtk::Button, gtk::Label) {
    let b = gtk::Button::new();
    let bx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bx.append(&icon(icon_name, 14));
    let l = gtk::Label::new(Some(text));
    bx.append(&l);
    b.set_child(Some(&bx));
    b.set_focus_on_click(false);
    (b, l)
}

fn clear_grid(g: &gtk::Grid) {
    while let Some(c) = g.first_child() {
        g.remove(&c);
    }
}

fn clear_box(b: &gtk::Box) {
    while let Some(c) = b.first_child() {
        b.remove(&c);
    }
}

fn ago(ts: f64) -> String {
    let s = (now() - ts).max(0.0);
    if s < 60.0 {
        "now".into()
    } else if s < 3600.0 {
        format!("{} min", (s / 60.0) as i64)
    } else if s < 86_400.0 {
        format!("{} h", (s / 3600.0) as i64)
    } else {
        local(ts, "%b %-d")
    }
}

/// "4 min ago", "just now", "on Nov 2".
fn since(ts: f64) -> String {
    match ago(ts) {
        a if a == "now" => "just now".into(),
        a if (now() - ts) < 86_400.0 => format!("{a} ago"),
        a => format!("on {a}"),
    }
}

/// "Today", "Yesterday", a weekday this week, else the date.
fn day(ts: f64) -> String {
    let d = local(ts, "%Y-%m-%d");
    let t = now();
    if d == local(t, "%Y-%m-%d") {
        "Today".into()
    } else if d == local(t - 86_400.0, "%Y-%m-%d") {
        "Yesterday".into()
    } else if t - ts < 6.0 * 86_400.0 {
        local(ts, "%A")
    } else {
        local(ts, "%B %-d")
    }
}

fn when(ts: f64) -> String {
    let time = local(ts, "%H:%M");
    match day(ts).as_str() {
        "Today" => format!("today at {time}"),
        "Yesterday" => format!("yesterday at {time}"),
        d => format!("{d} at {time}"),
    }
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A window class as a name: `org.mozilla.firefox` → Firefox.
fn app_name(class: &str) -> String {
    capitalized(&class.rsplit('.').next().unwrap_or(class).replace(['-', '_'], " "))
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

fn left(expires: f64) -> String {
    let s = (expires - now()).max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn size(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1024 * 1024 => format!("{:.1} KB", b as f64 / 1024.0),
        b => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

/// A code in readable groups: 483920 → "483 920".
fn spaced(code: &str) -> String {
    let n = code.chars().count();
    if !code.chars().all(|c| c.is_ascii_digit()) || !(6..=8).contains(&n) {
        return code.to_string();
    }
    let half = n.div_ceil(2);
    format!("{} {}", &code[..half], &code[half..])
}

fn code_from(c: &CodeInfo) -> String {
    if c.sites.is_empty() { c.app.clone() } else { format!("{} · {}", c.app, c.sites.join(", ")) }
}

/// `name` if the icon theme has it, else `fallback`.
fn themed(name: &str, fallback: &str) -> String {
    let has = gdk::Display::default().is_some_and(|d| gtk::IconTheme::for_display(&d).has_icon(name));
    if has { name } else { fallback }.to_string()
}

fn kind_icon(kind: Kind) -> String {
    let name = match kind {
        Kind::Text | Kind::Markdown => "format-justify-left-symbolic",
        Kind::Url => "insert-link-symbolic",
        Kind::Code => "utilities-terminal-symbolic",
        Kind::Files => "folder-documents-symbolic",
        Kind::Rich | Kind::Html => "format-text-rich-symbolic",
        Kind::Image => "image-x-generic-symbolic",
    };
    themed(name, kind.icon())
}

/// For an action whose own icon the theme lacks.
fn group_icon(g: Group) -> &'static str {
    match g {
        Group::Transform | Group::Convert => "format-justify-left-symbolic",
        Group::Send => "send-to-symbolic",
        Group::Assistant => "applications-science-symbolic",
        Group::Organize => "view-pin-symbolic",
        Group::Paste => "edit-copy-symbolic",
        Group::Apps => "application-x-addon-symbolic",
    }
}

/// The glyph colour for a kind (a theme swatch), none for plain text.
fn tint(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Url => Some("swatch-blue"),
        Kind::Code => Some("swatch-violet"),
        Kind::Files => Some("swatch-amber"),
        Kind::Rich | Kind::Html | Kind::Markdown => Some("swatch-teal"),
        _ => None,
    }
}

/// The line over the preview: "Text · copied from Signal · 4 min ago · 92 characters".
fn about(e: &Entry) -> String {
    let mut parts = vec![e.kind.label().to_string()];
    if let Some(a) = &e.app {
        parts.push(format!("copied from {}", app_name(a)));
    }
    parts.push(since(e.last_used));
    match e.kind {
        Kind::Image => {
            if let Some((w, h)) = e.dims {
                parts.push(format!("{w} × {h}"));
            }
            parts.push(size(e.size));
        }
        Kind::Files => {}
        _ => parts.push(format!("{} characters", count(e.chars))),
    }
    parts.join(" · ")
}

fn copied(e: &Entry) -> String {
    match e.uses {
        0 | 1 => capitalized(&when(e.created)),
        2 => format!("Twice, last {}", when(e.last_used)),
        n => format!("{n} times, last {}", when(e.last_used)),
    }
}

fn pastes_as(e: &Entry) -> String {
    let formatted = e.mimes.iter().any(|m| m == "text/html" || m == "text/rtf");
    match e.kind {
        Kind::Image => {
            let ty = e.mimes.iter().find_map(|m| m.strip_prefix("image/")).unwrap_or("png").to_uppercase();
            format!("An image ({ty})")
        }
        Kind::Files => "The files, or their paths as text".into(),
        Kind::Rich | Kind::Html => "Formatted text, or plain text".into(),
        _ if formatted => "Formatted text, or plain text".into(),
        _ => "Plain text".into(),
    }
}

/// Whether an action's label matches what was typed: a word starting with it.
fn matches(label: &str, q: &str) -> bool {
    if q.is_empty() {
        return true;
    }
    let l = label.to_lowercase();
    l.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(q)) || (q.chars().count() >= 3 && l.contains(q))
}

/// A countdown ring for a code: how much of its time is left.
fn ring(received: f64, expires: f64) -> gtk::DrawingArea {
    let d = gtk::DrawingArea::new();
    d.set_content_width(26);
    d.set_content_height(26);
    d.set_valign(gtk::Align::Center);
    d.set_widget_name("ring");
    d.add_css_class("cb-ring");
    d.set_draw_func(move |area, cr, w, h| {
        let span = (expires - received).max(1.0);
        let frac = ((expires - now()) / span).clamp(0.0, 1.0);
        let c = area.color();
        let (r, g, b) = (f64::from(c.red()), f64::from(c.green()), f64::from(c.blue()));
        let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
        let radius = cx.min(cy) - 3.0;
        cr.set_line_width(4.0);
        cr.set_source_rgba(r, g, b, 0.18);
        cr.arc(cx, cy, radius, 0.0, std::f64::consts::TAU);
        let _ = cr.stroke();
        cr.set_source_rgba(r, g, b, 1.0);
        let start = -std::f64::consts::FRAC_PI_2;
        cr.arc(cx, cy, radius, start, start + std::f64::consts::TAU * frac);
        let _ = cr.stroke();
    });
    d
}

fn focused_monitor() -> Option<gdk::Monitor> {
    let hypr = nyx_desktop::hypr::Instance::find().ok()?;
    let mons: Vec<serde_json::Value> = hypr.json("monitors").ok()?;
    let name = mons.iter().find(|m| m["focused"] == true)?.get("name")?.as_str()?.to_string();
    monitor_named(&name)
}

fn monitor_named(name: &str) -> Option<gdk::Monitor> {
    let display = gdk::Display::default()?;
    display.monitors().iter::<gdk::Monitor>().flatten().find(|m| m.connector().as_deref() == Some(name))
}

/// The pointer, relative to its monitor (logical pixels), and that monitor's size.
fn pointer() -> Option<((i32, i32), (i32, i32))> {
    let hypr = nyx_desktop::hypr::Instance::find().ok()?;
    let c: serde_json::Value = hypr.json("cursorpos").ok()?;
    let (cx, cy) = (c["x"].as_f64()? as i32, c["y"].as_f64()? as i32);
    let mons: Vec<serde_json::Value> = hypr.json("monitors").ok()?;
    let m = mons.iter().find(|m| m["focused"] == true)?;
    let scale = m["scale"].as_f64().unwrap_or(1.0).max(0.1);
    let (mx, my) = (m["x"].as_i64()? as i32, m["y"].as_i64()? as i32);
    let (mw, mh) = ((m["width"].as_f64()? / scale) as i32, (m["height"].as_f64()? / scale) as i32);
    Some(((cx - mx, cy - my), (mw, mh)))
}

const SHORTCUTS: &[(&str, &str)] = &[
    ("Paste", "Enter"),
    ("Paste as plain text", "Shift + Enter"),
    ("Paste the first nine", "Alt + 1 … 9"),
    ("Pick several to paste together", "Ctrl + Space"),
    ("Copy without pasting", "Ctrl + C"),
    ("Actions", "Ctrl + Enter · Menu · Shift + F10 · right-click"),
    ("Pin or unpin", "Ctrl + P"),
    ("Add to favorites", "Ctrl + S"),
    ("Delete", "Ctrl + Delete"),
    ("Recent · Pinned · Snippets", "Tab · Ctrl + 1 2 3"),
    ("New snippet · Edit snippet", "Ctrl + N · Ctrl + E"),
    ("Private mode", "Ctrl + Shift + P"),
    ("Clipboard's settings", "Ctrl + ,"),
    ("Filter by kind", "img: url: file: code: text: fav:"),
    ("Close", "Esc"),
];

impl Popup {
    pub fn is_open(&self) -> bool {
        self.window.is_visible()
    }

    fn weak(&self) -> Weak<Popup> {
        self.me.borrow().clone()
    }

    fn with(&self, f: impl Fn(&Popup) + 'static) -> impl Fn() + 'static {
        let w = self.weak();
        move || {
            if let Some(p) = w.upgrade() {
                f(&p);
            }
        }
    }

    pub fn toggle(&self) {
        if self.is_open() { self.hide() } else { self.show() }
    }

    pub fn toggle_tab(&self, tab: Tab) {
        if self.is_open() && self.tab.get() == tab {
            self.hide();
        } else {
            self.show();
            if self.is_open() {
                self.set_tab(tab);
            }
        }
    }

    /// Put the card on its screen, sized to fit. A test copy (`--only`) whose output
    /// isn't there doesn't open at all: the compositor would pick a screen itself.
    fn place(&self) -> bool {
        let settings = Settings::load();
        let monitor = match &self.only {
            Some(name) => match monitor_named(name) {
                Some(m) => Some(m),
                None => {
                    eprintln!("nyx-clipboard: there's no output named {name}");
                    return false;
                }
            },
            None => focused_monitor(),
        };
        self.window.set_monitor(monitor.as_ref());
        let (mw, mh) = monitor.as_ref().map(|m| (m.geometry().width(), m.geometry().height())).unwrap_or((1280, 800));
        let width = WIDTH.min(mw - 24).max(360);
        let height = HEIGHT.min(mh - 24).max(320);
        self.card.set_size_request(width, height);
        // On a small screen the list narrows and the preview's buttons get shorter labels.
        let narrow = width < 820;
        self.narrow.set(narrow);
        self.list_side.set_size_request(if narrow { width * 7 / 20 } else { LIST_WIDTH }, -1);
        self.preview.actions_label.set_visible(!narrow);
        let at_pointer = settings.position == Position::Cursor && self.only.is_none();
        let (left, top) = match pointer().filter(|_| at_pointer) {
            Some(((x, y), (w, h))) => ((x - 24).clamp(8, (w - width - 8).max(8)), (y + 12).clamp(8, (h - height - 8).max(8))),
            None => ((mw - width) / 2, (mh - height) / 2),
        };
        // Margins on every side fix the card's size: centred, it would take its content's
        // natural width (a long line in a hidden page) and grow.
        self.card.set_margin_start(left);
        self.card.set_margin_end((mw - left - width).max(0));
        self.card.set_margin_top(top);
        self.card.set_margin_bottom((mh - top - height).max(0));
        true
    }

    pub fn show(&self) {
        if self.is_open() {
            self.focus_search();
            return;
        }
        if !self.place() {
            return;
        }
        self.side.set_visible_child_name("preview");
        self.set_tab_quiet(Tab::Recent);
        self.set_kind_quiet(None);
        self.search.set_text("");
        self.marked.borrow_mut().clear();
        self.details.borrow_mut().clear();
        self.window.set_visible(true);
        self.focus_search();
        let on_change: Rc<dyn Fn()> = Rc::new(self.with(|p| p.queue_refresh()));
        *self.listening.borrow_mut() = client::on_changed(on_change);
        let tick = self.with(|p| p.tick());
        *self.ticker.borrow_mut() = Some(glib::timeout_add_seconds_local(1, move || {
            tick();
            glib::ControlFlow::Continue
        }));
        self.refresh(true);
        // For trying it on a test output: NYX_CLIPBOARD_TEST_VIEW, steps separated by
        // commas, such as `down,actions,type:tr` (pinned, snippets, help, actions,
        // editor, images, down, type:TEXT, search:TEXT).
        if self.only.is_some()
            && let Ok(view) = std::env::var("NYX_CLIPBOARD_TEST_VIEW")
        {
            for (i, step) in view.split(',').map(str::to_string).enumerate() {
                let w = self.weak();
                glib::timeout_add_local_once(Duration::from_millis(700 + 500 * i as u64), move || {
                    let Some(p) = w.upgrade() else { return };
                    match step.as_str() {
                        "pinned" => p.set_tab(Tab::Pinned),
                        "snippets" => p.set_tab(Tab::Snippets),
                        "help" => p.show_help(),
                        "actions" => p.open_actions(),
                        "editor" => p.edit_snippet(None),
                        "images" => p.set_kind(Some(Kind::Image)),
                        "down" => p.move_by(1),
                        s if s.starts_with("type:") => p.pane.field.set_text(&s["type:".len()..]),
                        s if s.starts_with("search:") => p.search.set_text(&s["search:".len()..]),
                        _ => {}
                    }
                });
            }
        }
    }

    pub fn hide(&self) {
        if !self.is_open() {
            return;
        }
        self.window.set_visible(false);
        self.listening.borrow_mut().clear();
        if let Some(id) = self.ticker.borrow_mut().take() {
            id.remove();
        }
        self.disarm();
        self.result.borrow_mut().take();
        self.result_text.buffer().set_text("");
        self.pane.subject.borrow_mut().take();
        self.pane.items.borrow_mut().clear();
        self.items.borrow_mut().clear();
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        self.show_preview();
    }

    fn toast(&self, text: &str, danger: bool) {
        self.toast.set_text(text);
        if danger {
            self.toast.add_css_class("danger");
        } else {
            self.toast.remove_css_class("danger");
        }
        self.toast.set_visible(true);
        self.hint.set_visible(false);
        if let Some(t) = self.toast_timer.borrow_mut().take() {
            t.remove();
        }
        let w = self.weak();
        *self.toast_timer.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(3500), move || {
            if let Some(p) = w.upgrade() {
                p.toast_timer.borrow_mut().take();
                p.toast.set_visible(false);
                p.hint.set_visible(true);
            }
        }));
    }

    fn update_hint(&self) {
        let marked = self.marked.borrow().len();
        let text = if marked > 0 {
            format!("{marked} picked · ↵ pastes them together · Ctrl Space unpicks")
        } else if self.tab.get() == Tab::Snippets {
            "↑ ↓ move · Ctrl N new snippet · Ctrl E edit · F1 every key".to_string()
        } else {
            "↑ ↓ move · Ctrl Space picks several · F1 every key".to_string()
        };
        self.hint.set_text(&text);
    }

    fn set_tab_quiet(&self, tab: Tab) {
        self.tab.set(tab);
        for (t, b, _) in &self.tabs {
            b.set_active(*t == tab);
        }
        self.kinds_row.set_visible(tab != Tab::Snippets);
        self.search.set_placeholder_text(Some(match tab {
            Tab::Snippets => "Search snippets",
            Tab::Pinned => "Search what you pinned",
            Tab::Recent => "Search everything you copied",
        }));
    }

    fn set_tab(&self, tab: Tab) {
        if self.tab.get() == tab {
            return;
        }
        self.set_tab_quiet(tab);
        self.marked.borrow_mut().clear();
        self.side.set_visible_child_name("preview");
        self.refresh(true);
        self.focus_search();
    }

    fn cycle_tab(&self, back: bool) {
        let order = [Tab::Recent, Tab::Pinned, Tab::Snippets];
        let i = order.iter().position(|t| *t == self.tab.get()).unwrap_or(0);
        let next = if back { (i + order.len() - 1) % order.len() } else { (i + 1) % order.len() };
        self.set_tab(order[next]);
    }

    fn set_kind_quiet(&self, kind: Option<Kind>) {
        self.kind.set(kind);
        for (k, b) in &self.kinds {
            b.set_active(*k == kind);
        }
    }

    fn set_kind(&self, kind: Option<Kind>) {
        if self.kind.get() == kind {
            return;
        }
        self.set_kind_quiet(kind);
        self.marked.borrow_mut().clear();
        self.refresh(true);
        self.focus_search();
    }

    fn queue_refresh(&self) {
        if self.pending_refresh.replace(true) {
            return;
        }
        let w = self.weak();
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            if let Some(p) = w.upgrade() {
                p.pending_refresh.set(false);
                if p.is_open() && p.side.visible_child_name().as_deref() == Some("preview") {
                    p.refresh(false);
                }
            }
        });
    }

    fn search_for(&self, offset: u32) -> Search {
        let view = if self.tab.get() == Tab::Pinned { View::Pinned } else { View::Recent };
        Search { text: self.search.text().to_string(), view, offset, limit: PAGE, kind: self.kind.get() }
    }

    /// Read again from the start. `reset` moves the selection to the top.
    fn refresh(&self, reset: bool) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.offset.set(0);
        self.exhausted.set(false);
        let keep = if reset { None } else { self.selected_key() };
        let tab = self.tab.get();
        let query = self.search_for(0);
        let everything = tab == Tab::Recent && query.text.is_empty() && query.kind.is_none();
        let w = self.weak();
        act(async move {
            let status = client::status().await;
            let (codes, secrets) = if everything {
                (client::codes().await.unwrap_or_default(), client::transient().await.unwrap_or_default())
            } else {
                (Vec::new(), Vec::new())
            };
            let rows: std::result::Result<Vec<Item>, String> = match tab {
                Tab::Snippets => client::snippets(&query.text).await.map(|v| v.into_iter().map(Item::Snippet).collect()),
                _ => client::search(&query).await.map(|v| v.into_iter().map(Item::Entry).collect()),
            };
            let Some(p) = w.upgrade() else { return };
            if p.generation.get() != generation {
                return;
            }
            match &status {
                Ok(s) => {
                    *p.status.borrow_mut() = s.clone();
                    p.private.set_active(s.private);
                    p.show_banner(s);
                    for (t, _, n) in &p.tabs {
                        n.set_text(&count(match t {
                            Tab::Recent => s.entries,
                            Tab::Pinned => s.pinned,
                            Tab::Snippets => s.snippets,
                        }));
                    }
                }
                Err(e) => {
                    p.banner.set_text(e);
                    p.banner.add_css_class("danger");
                    p.banner.set_visible(true);
                }
            }
            let mut items: Vec<Item> = codes.into_iter().map(Item::Code).chain(secrets.into_iter().map(Item::Secret)).collect();
            let fetched = match rows {
                Ok(r) => r,
                Err(e) => {
                    p.toast(&e, true);
                    Vec::new()
                }
            };
            p.exhausted.set(tab == Tab::Snippets || (fetched.len() as u32) < PAGE);
            p.offset.set(fetched.len() as u32);
            items.extend(fetched);
            p.fill(items, keep);
        });
    }

    fn show_banner(&self, s: &Status) {
        self.banner.remove_css_class("danger");
        // A test copy's scratch service has no compositor to watch, on purpose.
        let expected = self.only.is_some() && std::env::var_os("NYX_CLIPBOARD_TEST_UNWATCHED").is_some();
        if !s.watching && !expected {
            self.banner.set_text(&format!(
                "The clipboard isn't being watched{}",
                s.problem.as_deref().map(|p| format!(": {p}")).unwrap_or_default()
            ));
            self.banner.add_css_class("danger");
            self.banner.set_visible(true);
        } else if s.private {
            self.banner.set_text("Private mode: nothing new is saved until you turn it off");
            self.banner.set_visible(true);
        } else {
            self.banner.set_visible(false);
        }
    }

    fn more(&self) {
        if self.exhausted.get() || self.loading.get() || self.tab.get() == Tab::Snippets {
            return;
        }
        self.loading.set(true);
        let generation = self.generation.get();
        let offset = self.offset.get();
        let query = self.search_for(offset);
        let w = self.weak();
        act(async move {
            let rows = client::search(&query).await.unwrap_or_default();
            let Some(p) = w.upgrade() else { return };
            p.loading.set(false);
            if p.generation.get() != generation {
                return;
            }
            p.exhausted.set((rows.len() as u32) < PAGE);
            p.offset.set(offset + rows.len() as u32);
            for e in rows {
                let item = Item::Entry(e);
                let row = p.row(&item, 0);
                p.items.borrow_mut().push(item);
                p.list.append(&row);
            }
        });
    }

    fn key_of(item: &Item) -> String {
        match item {
            Item::Code(c) => format!("code:{}", c.id),
            Item::Secret(s) => format!("secret:{}", s.id),
            Item::Entry(e) => format!("entry:{}", e.id),
            Item::Snippet(s) => format!("snippet:{}", s.id),
        }
    }

    fn selected_index(&self) -> Option<usize> {
        self.list.selected_row().map(|r| r.index() as usize)
    }

    fn selected(&self) -> Option<Item> {
        let i = self.selected_index()?;
        self.items.borrow().get(i).cloned()
    }

    fn selected_key(&self) -> Option<String> {
        self.selected().map(|i| Self::key_of(&i))
    }

    fn fill(&self, items: Vec<Item>, keep: Option<String>) {
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
        let select = keep.and_then(|k| items.iter().position(|i| Self::key_of(i) == k)).unwrap_or(0);
        let none = items.is_empty();
        // Set first: the day headers read it as rows are added.
        *self.items.borrow_mut() = items.clone();
        let mut n = 0;
        for item in &items {
            let numbered = matches!(item, Item::Entry(_) | Item::Snippet(_));
            n += usize::from(numbered);
            let row = self.row(item, if numbered { n } else { 0 });
            self.list.append(&row);
        }
        self.empty.set_visible(none);
        self.scroller.set_visible(!none);
        let filtered = !self.search.text().is_empty() || self.kind.get().is_some();
        self.empty.set_text(match (self.tab.get(), filtered) {
            (_, true) => "Nothing matches",
            (Tab::Recent, false) => "Nothing copied yet. Copy something and it shows up here.",
            (Tab::Pinned, false) => "No pinned items. Ctrl + P pins the chosen item, and pinned items always stay.",
            (Tab::Snippets, false) => "No snippets. Ctrl + N adds one; Save as a snippet in an item's actions makes one from it.",
        });
        match self.list.row_at_index(select as i32) {
            Some(r) => self.list.select_row(Some(&r)),
            None => self.show_preview(),
        }
        self.update_hint();
    }

    /// A list row; `n` (1–9) labels it with its Alt + n key.
    fn row(&self, item: &Item, n: usize) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        if let Item::Code(c) = item {
            row.add_css_class("cb-code-card");
            row.set_child(Some(&self.code_card(c)));
            row.update_property(&[gtk::accessible::Property::Label(&format!("Verification code from {}", code_from(c)))]);
            return row;
        }
        row.add_css_class("cb-row");
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let glyph = gtk::Box::new(gtk::Orientation::Vertical, 0);
        glyph.add_css_class("cb-glyph");
        glyph.set_size_request(32, 32);
        glyph.set_valign(gtk::Align::Center);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 2);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let (title, meta, icon_name, tinted) = match item {
            Item::Code(_) => unreachable!(),
            Item::Secret(s) => {
                row.add_css_class("cb-secret");
                (format!("{} (not saved)", s.label), format!("Kept in memory · {}", left(s.expires)), "security-high-symbolic".to_string(), None)
            }
            Item::Entry(e) => {
                let mut parts = vec![e.kind.label().to_string()];
                if let Some(a) = &e.app {
                    parts.push(app_name(a));
                }
                parts.push(ago(e.last_used));
                if e.kind == Kind::Image || e.size > 4096 {
                    parts.push(size(e.size));
                }
                (e.preview.clone(), parts.join(" · "), kind_icon(e.kind), tint(e.kind))
            }
            Item::Snippet(s) => (
                s.name.clone(),
                nyx_clipboard_core::classify::preview(&s.body),
                "insert-text-symbolic".to_string(),
                Some("swatch-green"),
            ),
        };
        if let Item::Entry(e) = item
            && let Some(img) = &e.image
        {
            glyph.add_css_class("image");
            let pic = gtk::Image::new();
            pic.set_pixel_size(32);
            pic.set_vexpand(true);
            pic.add_css_class("cb-thumb");
            thumbs::load_icon(&pic, img, 64);
            glyph.append(&pic);
        } else {
            let i = icon(&icon_name, 16);
            i.set_vexpand(true);
            glyph.append(&i);
            if let Some(t) = tinted {
                glyph.add_css_class("tinted");
                glyph.add_css_class(&format!("section-{t}"));
            }
        }
        b.append(&glyph);
        let t = label(&title, "cb-title");
        if matches!(item, Item::Entry(e) if e.kind == Kind::Code) {
            t.add_css_class("mono");
        }
        text.append(&t);
        let m = label(&meta, "cb-meta");
        m.set_widget_name("meta");
        text.append(&m);
        b.append(&text);
        if let Item::Entry(e) = item {
            if self.marked.borrow().contains(&e.id) {
                row.add_css_class("marked");
                let check = icon("object-select-symbolic", 14);
                check.add_css_class("accent");
                b.append(&check);
            }
            if e.favorite {
                let s = icon("starred-symbolic", 13);
                s.add_css_class("cb-mark");
                s.add_css_class("star");
                s.set_tooltip_text(Some("Favorite"));
                b.append(&s);
            }
            if e.pinned {
                let s = icon("view-pin-symbolic", 13);
                s.add_css_class("cb-mark");
                s.set_tooltip_text(Some("Pinned"));
                b.append(&s);
            }
        }
        if (1..=9).contains(&n) {
            let k = gtk::Label::new(Some(&format!("Alt {n}")));
            k.add_css_class("cb-key");
            k.set_valign(gtk::Align::Center);
            b.append(&k);
        }
        row.set_child(Some(&b));
        row.update_property(&[gtk::accessible::Property::Label(&format!("{title}. {meta}"))]);
        let right = gtk::GestureClick::new();
        right.set_button(3);
        let w = self.weak();
        let r2 = row.clone();
        right.connect_pressed(move |g, _, _, _| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(p) = w.upgrade() {
                p.list.select_row(Some(&r2));
                p.open_actions();
            }
        });
        row.add_controller(right);
        row
    }

    /// A verification code, on top of the list with its countdown and a Copy button.
    fn code_card(&self, c: &CodeInfo) -> gtk::Box {
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let i = icon("dialog-password-symbolic", 20);
        i.add_css_class("accent");
        b.append(&i);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let code = gtk::Label::new(Some(&spaced(&c.code)));
        code.add_css_class("cb-code-text");
        code.set_xalign(0.0);
        text.append(&code);
        text.append(&label(&format!("Verification code · {}", code_from(c)), "cb-code-sub"));
        b.append(&text);
        b.append(&ring(c.received, c.expires));
        let l = gtk::Label::new(Some(&left(c.expires)));
        l.add_css_class("cb-left");
        l.set_widget_name("left");
        b.append(&l);
        let (copy, copy_label) = icon_button("edit-copy-symbolic", "Copy");
        copy_label.set_visible(!self.narrow.get());
        copy.update_property(&[gtk::accessible::Property::Label("Copy")]);
        copy.add_css_class("cb-code-copy");
        copy.set_valign(gtk::Align::Center);
        copy.set_tooltip_text(Some("Copy it without pasting (Ctrl C)"));
        let w = self.weak();
        let key = format!("code:{}", c.id);
        copy.connect_clicked(move |_| {
            let Some(p) = w.upgrade() else { return };
            let i = p.items.borrow().iter().position(|it| Self::key_of(it) == key);
            if let Some(i) = i {
                p.select_index(i as i32);
                p.copy_selected();
            }
        });
        b.append(&copy);
        b
    }

    // ---- Preview

    fn show_preview(&self) {
        let p = &self.preview;
        p.left.borrow_mut().take();
        clear_box(&p.body);
        clear_grid(&p.facts);
        p.picture.set_paintable(None::<&gdk::Paintable>);
        p.picture_box.set_visible(false);
        p.scroll.set_visible(true);
        p.scroll.vadjustment().set_value(0.0);
        let Some(item) = self.selected() else {
            p.icon.set_visible(false);
            p.meta.set_text("");
            p.bar.set_visible(false);
            return;
        };
        p.icon.set_visible(true);
        p.bar.set_visible(true);
        let marked = self.marked.borrow().len();
        p.paste_label.set_text(&if marked > 0 && self.tab.get() != Tab::Snippets {
            format!("Paste {marked} together")
        } else {
            "Paste".to_string()
        });
        let mut facts: Vec<(&str, String)> = Vec::new();
        let mut countdown: Option<f64> = None;
        match &item {
            Item::Entry(e) => {
                p.icon.set_icon_name(Some(&kind_icon(e.kind)));
                p.meta.set_text(&about(e));
                let detail = self.details.borrow().get(&e.id).cloned();
                if detail.is_none() {
                    self.fetch_detail(e.id);
                }
                self.preview_body(e, detail.as_ref());
                if let Some(a) = &e.app {
                    facts.push(("From", app_name(a)));
                }
                facts.push(("Copied", copied(e)));
                facts.push(("Pastes as", pastes_as(e)));
                if e.kind == Kind::Image {
                    facts.push(("Size", size(e.size)));
                }
                p.plain.set_visible(e.kind.has_text());
                p.plain_label.set_text(if self.narrow.get() { "Plain text" } else { "Paste as plain text" });
                p.pin.set_visible(true);
                p.favorite.set_visible(true);
                p.edit.set_visible(false);
                p.actions.set_visible(true);
                let pin = if e.pinned { "Unpin (Ctrl P)" } else { "Pin (Ctrl P)" };
                p.pin.set_tooltip_text(Some(pin));
                p.pin.update_property(&[gtk::accessible::Property::Label(pin)]);
                if e.pinned { p.pin.add_css_class("on") } else { p.pin.remove_css_class("on") }
                let fav = if e.favorite { "Remove from favorites (Ctrl S)" } else { "Favorite (Ctrl S)" };
                p.favorite.set_icon_name(if e.favorite { "starred-symbolic" } else { "non-starred-symbolic" });
                p.favorite.set_tooltip_text(Some(fav));
                p.favorite.update_property(&[gtk::accessible::Property::Label(fav)]);
                if e.favorite { p.favorite.add_css_class("on") } else { p.favorite.remove_css_class("on") }
            }
            Item::Code(c) => {
                p.icon.set_icon_name(Some("dialog-password-symbolic"));
                p.meta.set_text(&format!("Verification code · from {}", c.app));
                let l = gtk::Label::new(Some(&spaced(&c.code)));
                l.add_css_class("cb-pcode");
                l.set_xalign(0.0);
                p.body.append(&l);
                facts.push(("From", c.app.clone()));
                if !c.sites.is_empty() {
                    facts.push(("For", c.sites.join(", ")));
                }
                facts.push(("Kept", "In memory only, never saved".into()));
                countdown = Some(c.expires);
                self.bar_for_copy();
            }
            Item::Secret(s) => {
                p.icon.set_icon_name(Some("security-high-symbolic"));
                p.meta.set_text(&format!("{} · not saved", s.label));
                let l = gtk::Label::new(Some(
                    "This looks like a password or a key, so it isn't written down. It stays here for a moment so you can still paste it.",
                ));
                l.add_css_class("cb-pnote");
                l.set_wrap(true);
                l.set_xalign(0.0);
                p.body.append(&l);
                countdown = Some(s.expires);
                self.bar_for_copy();
            }
            Item::Snippet(s) => {
                p.icon.set_icon_name(Some("insert-text-symbolic"));
                p.meta.set_text(&if s.name.is_empty() { "Snippet".to_string() } else { format!("Snippet · {}", s.name) });
                p.body.append(&self.body_text(&s.body, false));
                let fills: Vec<&str> = ["{date}", "{time}", "{datetime}", "{weekday}", "{clipboard}"]
                    .into_iter()
                    .filter(|f| s.body.contains(f))
                    .collect();
                if !fills.is_empty() {
                    facts.push(("Fills in", fills.join(" ")));
                }
                if s.updated > 0.0 {
                    facts.push(("Changed", capitalized(&when(s.updated))));
                }
                self.bar_for_copy();
                p.edit.set_visible(true);
                p.actions.set_visible(true);
            }
        }
        let mut row = 0;
        for (k, v) in facts {
            self.fact(row, k, &v);
            row += 1;
        }
        if let Some(expires) = countdown {
            let v = self.fact(row, "Expires in", &left(expires));
            *p.left.borrow_mut() = Some((v, expires));
        }
    }

    fn bar_for_copy(&self) {
        let p = &self.preview;
        p.plain.set_visible(true);
        p.plain_label.set_text("Copy");
        p.pin.set_visible(false);
        p.favorite.set_visible(false);
        p.edit.set_visible(false);
        p.actions.set_visible(false);
    }

    fn fact(&self, row: i32, key: &str, value: &str) -> gtk::Label {
        let k = gtk::Label::new(Some(key));
        k.add_css_class("cb-fact-key");
        k.set_xalign(0.0);
        let v = label(value, "cb-fact");
        self.preview.facts.attach(&k, 0, row, 1, 1);
        self.preview.facts.attach(&v, 1, row, 1, 1);
        v
    }

    fn body_text(&self, text: &str, mono: bool) -> gtk::Label {
        let mut t: String = text.chars().take(PREVIEW_CHARS).collect();
        if text.chars().nth(PREVIEW_CHARS).is_some() {
            t.push_str("\n…");
        }
        let l = gtk::Label::new(Some(&t));
        l.add_css_class("cb-pbody");
        if mono {
            l.add_css_class("mono");
        }
        l.set_wrap(true);
        l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        l.set_xalign(0.0);
        l.set_yalign(0.0);
        l.set_selectable(false);
        l
    }

    fn preview_body(&self, e: &Entry, detail: Option<&Detail>) {
        let p = &self.preview;
        if let Some(img) = &e.image {
            p.scroll.set_visible(false);
            p.picture_box.set_visible(true);
            thumbs::load(&p.picture, img, 720);
            return;
        }
        let Some(d) = detail else {
            p.body.append(&self.body_text(&e.preview, e.kind == Kind::Code));
            return;
        };
        if e.kind == Kind::Files && !d.files.is_empty() {
            for f in &d.files {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                row.add_css_class("cb-file");
                row.append(&icon("text-x-generic-symbolic", 14));
                let l = label(f, "cb-file-path");
                l.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                l.set_tooltip_text(Some(f));
                row.append(&l);
                p.body.append(&row);
            }
            return;
        }
        let mono = matches!(e.kind, Kind::Code | Kind::Html);
        p.body.append(&self.body_text(&d.text, mono));
        if d.truncated {
            p.body.append(&label("Only the start is shown; all of it pastes.", "cb-pnote"));
        }
    }

    fn fetch_detail(&self, id: i64) {
        let w = self.weak();
        act(async move {
            let Ok(d) = client::detail(id).await else { return };
            let Some(p) = w.upgrade() else { return };
            p.details.borrow_mut().insert(id, d);
            if p.selected_key().as_deref() == Some(&format!("entry:{id}")) {
                p.show_preview();
            }
        });
    }

    fn tick(&self) {
        let t = now();
        let mut expired = false;
        let items = self.items.borrow().clone();
        for (i, item) in items.iter().enumerate() {
            let expires = match item {
                Item::Code(c) => c.expires,
                Item::Secret(s) => s.expires,
                _ => continue,
            };
            if expires <= t {
                expired = true;
                continue;
            }
            let Some(row) = self.list.row_at_index(i as i32) else { continue };
            match item {
                Item::Code(_) => {
                    if let Some(l) = find_named(row.upcast_ref(), "left").and_then(|w| w.downcast::<gtk::Label>().ok()) {
                        l.set_text(&left(expires));
                    }
                    if let Some(r) = find_named(row.upcast_ref(), "ring") {
                        r.queue_draw();
                    }
                }
                _ => {
                    if let Some(l) = find_named(row.upcast_ref(), "meta").and_then(|w| w.downcast::<gtk::Label>().ok()) {
                        l.set_text(&format!("Kept in memory · {}", left(expires)));
                    }
                }
            }
        }
        if let Some((l, expires)) = self.preview.left.borrow().as_ref() {
            l.set_text(&left(*expires));
        }
        if expired {
            self.refresh(false);
        }
    }

    fn move_by(&self, delta: i32) {
        let n = self.items.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let cur = self.selected_index().map(|i| i as i32).unwrap_or(-1);
        let next = (cur + delta).clamp(0, n - 1);
        if let Some(r) = self.list.row_at_index(next) {
            self.list.select_row(Some(&r));
            self.scroll_to(&r);
        }
        if next >= n - 5 {
            self.more();
        }
    }

    fn select_index(&self, i: i32) {
        if let Some(r) = self.list.row_at_index(i) {
            self.list.select_row(Some(&r));
            self.scroll_to(&r);
        }
    }

    /// Keep the selected row in view without moving the keyboard from the search field.
    fn scroll_to(&self, row: &gtk::ListBoxRow) {
        let Some(b) = row.compute_bounds(&self.list) else { return };
        let adj = self.scroller.vadjustment();
        let (y, h) = (f64::from(b.y()), f64::from(b.height()));
        if y < adj.value() {
            adj.set_value(y);
        } else if y + h > adj.value() + adj.page_size() {
            adj.set_value(y + h - adj.page_size());
        }
    }

    fn focus_search(&self) {
        self.search.grab_focus();
        self.search.set_position(-1);
    }

    /// Enter: paste what's selected (or everything picked).
    fn activate(&self, plain: bool) {
        let marked = self.marked.borrow().clone();
        if !marked.is_empty() && self.tab.get() != Tab::Snippets {
            self.hide();
            act(async move {
                if let Err(e) = client::paste(marked, plain).await {
                    eprintln!("nyx-clipboard: {e}");
                }
            });
            return;
        }
        let Some(item) = self.selected() else { return };
        self.hide();
        act(async move {
            let r = match item {
                Item::Code(c) => client::copy_code(&c.id, true).await,
                Item::Secret(s) => client::use_transient(&s.id, true).await,
                Item::Entry(e) => client::paste(vec![e.id], plain).await,
                Item::Snippet(s) => client::use_snippet(s.id, true).await,
            };
            if let Err(e) = r {
                eprintln!("nyx-clipboard: {e}");
            }
        });
    }

    fn paste_nth(&self, n: usize) {
        let idx = self.items.borrow().iter().enumerate().filter(|(_, i)| matches!(i, Item::Entry(_) | Item::Snippet(_))).nth(n).map(|(i, _)| i);
        if let Some(i) = idx {
            self.select_index(i as i32);
            self.marked.borrow_mut().clear();
            self.activate(false);
        }
    }

    fn copy_selected(&self) {
        let Some(item) = self.selected() else { return };
        self.hide();
        act(async move {
            let _ = match item {
                Item::Code(c) => client::copy_code(&c.id, false).await,
                Item::Secret(s) => client::use_transient(&s.id, false).await,
                Item::Entry(e) => client::copy(e.id, false).await,
                Item::Snippet(s) => client::use_snippet(s.id, false).await,
            };
        });
    }

    fn toggle_mark(&self) {
        let Some(Item::Entry(e)) = self.selected() else { return };
        {
            let mut m = self.marked.borrow_mut();
            if let Some(i) = m.iter().position(|x| *x == e.id) {
                m.remove(i);
            } else {
                m.push(e.id);
            }
        }
        let i = self.selected_index().unwrap_or(0);
        let item = self.items.borrow()[i].clone();
        let n = self.items.borrow()[..=i].iter().filter(|x| matches!(x, Item::Entry(_) | Item::Snippet(_))).count();
        let row = self.row(&item, n);
        if let Some(old) = self.list.row_at_index(i as i32) {
            self.list.remove(&old);
        }
        self.list.insert(&row, i as i32);
        self.list.select_row(Some(&row));
        self.update_hint();
    }

    fn toggle_pin(&self) {
        let Some(Item::Entry(e)) = self.selected() else { return };
        let w = self.weak();
        act(async move {
            let r = client::set_pinned(e.id, !e.pinned).await;
            if let Some(p) = w.upgrade() {
                match r {
                    Ok(_) => p.toast(if e.pinned { "Unpinned" } else { "Pinned: it stays until you unpin it" }, false),
                    Err(err) => p.toast(&err, true),
                }
                p.refresh(false);
            }
        });
    }

    fn toggle_favorite(&self) {
        let Some(Item::Entry(e)) = self.selected() else { return };
        let w = self.weak();
        act(async move {
            let r = client::set_favorite(e.id, !e.favorite).await;
            if let Some(p) = w.upgrade() {
                match r {
                    Ok(_) => p.toast(if e.favorite { "Removed from favorites" } else { "Added to favorites (fav: finds them)" }, false),
                    Err(err) => p.toast(&err, true),
                }
                p.refresh(false);
            }
        });
    }

    fn disarm(&self) {
        if let Some((_, id)) = self.armed.borrow_mut().take() {
            id.remove();
        }
    }

    /// Dangerous actions take two presses within four seconds.
    fn armed(&self, key: &str, message: &str) -> bool {
        if self.armed.borrow().as_ref().is_some_and(|(k, _)| k == key) {
            self.disarm();
            return true;
        }
        self.disarm();
        let w = self.weak();
        let id = glib::timeout_add_local_once(Duration::from_secs(4), move || {
            if let Some(p) = w.upgrade() {
                p.armed.borrow_mut().take();
            }
        });
        *self.armed.borrow_mut() = Some((key.to_string(), id));
        self.toast(message, true);
        false
    }

    fn delete_selected(&self) {
        let Some(item) = self.selected() else { return };
        let key = Self::key_of(&item);
        let w = self.weak();
        match item {
            Item::Entry(e) => {
                if !e.pinned || self.armed(&key, "Pinned: press again to delete it") {
                    act(async move {
                        let r = client::delete(vec![e.id]).await;
                        if let Some(p) = w.upgrade() {
                            if let Err(err) = r {
                                p.toast(&err, true);
                            }
                            p.refresh(false);
                        }
                    });
                }
            }
            Item::Snippet(s) => {
                if self.armed(&key, "Press again to delete this snippet") {
                    self.forget_snippet(s.id);
                }
            }
            Item::Secret(s) => act(async move {
                let _ = client::forget_transient(&s.id).await;
                if let Some(p) = w.upgrade() {
                    p.refresh(false);
                }
            }),
            Item::Code(c) => act(async move {
                let _ = client::dismiss_code(&c.id).await;
                if let Some(p) = w.upgrade() {
                    p.refresh(false);
                }
            }),
        }
    }

    fn forget_snippet(&self, id: i64) {
        let w = self.weak();
        act(async move {
            let _ = client::delete_snippet(id).await;
            if let Some(p) = w.upgrade() {
                p.toast("Snippet deleted", false);
                p.refresh(false);
            }
        });
    }

    fn toggle_private(&self) {
        let on = !self.status.borrow().private;
        let w = self.weak();
        act(async move {
            let r = client::set_private(on).await;
            if let Some(p) = w.upgrade() {
                match r {
                    Ok(now_on) => p.toast(if now_on { "Private mode on: nothing new is saved" } else { "Private mode off" }, false),
                    Err(e) => p.toast(&e, true),
                }
                p.refresh(false);
            }
        });
    }

    /// Settings › Desktop › Clipboard in Nyx Settings.
    fn open_settings(&self) {
        if self.only.is_some() {
            self.toast("A test copy doesn't open Settings", false);
            return;
        }
        self.hide();
        if let Err(e) = std::process::Command::new("nyx-settings").args(["--section", "clipboard.clipboard"]).spawn() {
            eprintln!("nyx-clipboard: can't open Settings: {e}");
        }
    }

    // ---- Snippet editor

    fn edit_snippet(&self, existing: Option<Snippet>) {
        let s = existing.unwrap_or_default();
        self.editing.set(s.id);
        self.editor_name.set_text(&s.name);
        self.editor_body.buffer().set_text(&s.body);
        self.side.set_visible_child_name("editor");
        self.hint.set_text("Ctrl S saves · Esc cancels");
        self.editor_body.grab_focus();
    }

    fn save_snippet(&self) {
        let buf = self.editor_body.buffer();
        let body = buf.text(&buf.start_iter(), &buf.end_iter(), false).to_string();
        if body.trim().is_empty() {
            self.toast("A snippet needs some text", true);
            return;
        }
        let s = Snippet { id: self.editing.get(), name: self.editor_name.text().to_string(), body, ..Default::default() };
        let w = self.weak();
        act(async move {
            let r = client::save_snippet(&s).await;
            if let Some(p) = w.upgrade() {
                match r {
                    Ok(_) => {
                        p.back_to_preview();
                        p.set_tab(Tab::Snippets);
                        p.toast("Snippet saved", false);
                        p.refresh(false);
                    }
                    Err(e) => p.toast(&e, true),
                }
            }
        });
    }

    fn back_to_preview(&self) {
        self.side.set_visible_child_name("preview");
        self.result.borrow_mut().take();
        self.target_for.borrow_mut().take();
        self.focus_search();
        self.update_hint();
    }

    // ---- Actions

    fn open_actions(&self) {
        let Some(item) = self.selected() else { return };
        match item {
            Item::Entry(e) => {
                let w = self.weak();
                act(async move {
                    let r = client::actions(e.id).await;
                    let Some(p) = w.upgrade() else { return };
                    match r {
                        Ok(list) => p.show_actions(Item::Entry(e), list),
                        Err(err) => p.toast(&err, true),
                    }
                });
            }
            Item::Snippet(s) => {
                let a = |id: &str, label: &str, icon: &str, group: Group| Action {
                    id: id.into(),
                    label: label.into(),
                    icon: icon.into(),
                    group,
                    source: "builtin".into(),
                    destructive: id == "delete",
                    previews: false,
                };
                let list = vec![
                    a("paste", "Paste", "edit-paste-symbolic", Group::Paste),
                    a("copy", "Copy", "edit-copy-symbolic", Group::Paste),
                    a("edit", "Edit", "document-edit-symbolic", Group::Organize),
                    a("delete", "Delete", "user-trash-symbolic", Group::Organize),
                ];
                self.show_actions(Item::Snippet(s), list);
            }
            _ => {}
        }
    }

    fn show_actions(&self, subject: Item, list: Vec<Action>) {
        let pane = &self.pane;
        for c in &pane.columns {
            clear_box(c);
        }
        let translate = Settings::load().translate_to;
        let entry = matches!(subject, Item::Entry(_));
        let mut sections: Vec<(&str, Vec<Action>)> = Vec::new();
        for mut a in list {
            // Paste and Paste as plain text are the preview's own buttons.
            if entry && matches!(a.id.as_str(), "paste" | "paste-plain") {
                continue;
            }
            if a.id == "ai-translate" && !translate.trim().is_empty() {
                a.label = format!("Translate into {}", translate.trim());
            }
            let h = heading(a.group);
            match sections.iter_mut().find(|(x, _)| *x == h) {
                Some((_, v)) => v.push(a),
                None => sections.push((h, vec![a])),
            }
        }
        sections.sort_by_key(|(h, _)| HEADINGS.iter().position(|x| x == h));
        let mut order: [Vec<(Action, gtk::Button)>; 2] = [Vec::new(), Vec::new()];
        for (i, (h, acts)) in sections.into_iter().enumerate() {
            let g = gtk::Box::new(gtk::Orientation::Vertical, 0);
            g.add_css_class("cb-ag");
            g.append(&label(h, "cb-ag-head"));
            for a in acts {
                let (b, l) = icon_button(&themed(&a.icon, group_icon(a.group)), &a.label);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
                b.set_tooltip_text(Some(&a.label));
                b.add_css_class("flat");
                b.add_css_class("cb-ai");
                if a.destructive {
                    b.add_css_class("danger");
                }
                if let Some(c) = b.child() {
                    c.set_halign(gtk::Align::Start);
                }
                let w = self.weak();
                let id = a.id.clone();
                b.connect_clicked(move |_| {
                    if let Some(p) = w.upgrade() {
                        p.run_pane_action(&id);
                    }
                });
                g.append(&b);
                order[i % 2].push((a, b));
            }
            pane.columns[i % 2].append(&g);
        }
        let [left_col, right_col] = order;
        *pane.items.borrow_mut() = left_col.into_iter().chain(right_col).collect();
        pane.selected.set(None);
        let title = match &subject {
            Item::Entry(e) => e.preview.clone(),
            Item::Snippet(s) => s.name.clone(),
            _ => String::new(),
        };
        let short: String = title.chars().take(28).collect();
        let more = if title.chars().count() > 28 { "…" } else { "" };
        pane.about.set_text(&format!("Actions for “{short}{more}”"));
        *pane.subject.borrow_mut() = Some(subject);
        pane.field.set_text("");
        self.filter_actions();
        pane.scroll.vadjustment().set_value(0.0);
        self.side.set_visible_child_name("actions");
        pane.field.grab_focus();
    }

    fn pane_query(&self) -> String {
        self.pane.field.text().trim().to_lowercase()
    }

    fn filter_actions(&self) {
        let q = self.pane_query();
        let first = {
            let items = self.pane.items.borrow();
            let mut first = None;
            for (i, (a, b)) in items.iter().enumerate() {
                let hit = matches(&a.label, &q);
                if q.is_empty() {
                    b.remove_css_class("hit");
                    b.remove_css_class("dim");
                } else if hit {
                    b.add_css_class("hit");
                    b.remove_css_class("dim");
                } else {
                    b.add_css_class("dim");
                    b.remove_css_class("hit");
                }
                if hit && first.is_none() {
                    first = Some(i);
                }
            }
            first
        };
        let keep = self.pane.selected.get().filter(|&i| self.pane.items.borrow().get(i).is_some_and(|(a, _)| matches(&a.label, &q)));
        self.select_action(keep.or(first));
    }

    fn select_action(&self, i: Option<usize>) {
        for (j, (_, b)) in self.pane.items.borrow().iter().enumerate() {
            if Some(j) == i {
                b.add_css_class("sel");
                if let Some(grid) = self.pane.columns[0].parent()
                    && let Some(bounds) = b.compute_bounds(&grid)
                {
                    let adj = self.pane.scroll.vadjustment();
                    let (y, h) = (f64::from(bounds.y()), f64::from(bounds.height()));
                    if y < adj.value() {
                        adj.set_value(y);
                    } else if y + h > adj.value() + adj.page_size() {
                        adj.set_value(y + h - adj.page_size());
                    }
                }
            } else {
                b.remove_css_class("sel");
            }
        }
        self.pane.selected.set(i);
    }

    fn move_action(&self, delta: i32) {
        let q = self.pane_query();
        let hits: Vec<usize> =
            self.pane.items.borrow().iter().enumerate().filter(|(_, (a, _))| matches(&a.label, &q)).map(|(i, _)| i).collect();
        if hits.is_empty() {
            return;
        }
        let pos = self.pane.selected.get().and_then(|s| hits.iter().position(|&h| h == s));
        let next = match pos {
            Some(p) => (p as i32 + delta).clamp(0, hits.len() as i32 - 1) as usize,
            None => 0,
        };
        self.select_action(Some(hits[next]));
    }

    fn run_selected_action(&self) {
        let id = self.pane.selected.get().and_then(|i| self.pane.items.borrow().get(i).map(|(a, _)| a.id.clone()));
        if let Some(id) = id {
            self.run_pane_action(&id);
        }
    }

    fn run_pane_action(&self, id: &str) {
        let Some(subject) = self.pane.subject.borrow().clone() else { return };
        let Some(a) = self.pane.items.borrow().iter().find(|(a, _)| a.id == id).map(|(a, _)| a.clone()) else { return };
        if a.destructive && !self.armed(&format!("action:{id}"), "Press again to delete") {
            return;
        }
        match subject {
            Item::Entry(e) => {
                self.back_to_preview();
                self.run_action(&e, &a);
            }
            Item::Snippet(s) => {
                self.back_to_preview();
                match id {
                    "paste" => self.activate(false),
                    "copy" => self.copy_selected(),
                    "edit" => self.edit_snippet(Some(s)),
                    "delete" => self.forget_snippet(s.id),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn run_action(&self, entry: &Entry, a: &Action) {
        let id = entry.id;
        match a.id.as_str() {
            "save-image" | "save-file" => return self.save_dialog(entry.clone()),
            "note-append" => {
                *self.target_for.borrow_mut() = Some((id, a.id.clone()));
                self.target_title.set_text("Add to which note? Type its name or path in the vault.");
                self.target.set_text("");
                self.target.set_placeholder_text(Some("Note name"));
                self.side.set_visible_child_name("target");
                self.target.grab_focus();
                return;
            }
            "paste" | "paste-plain" => {
                let plain = a.id == "paste-plain";
                self.hide();
                act(async move {
                    let _ = client::paste(vec![id], plain).await;
                });
                return;
            }
            "share" | "reveal" => self.hide(),
            _ => {}
        }
        self.run_with(id, a.id.clone(), a.label.clone(), a.previews, serde_json::json!({}));
    }

    fn run_with(&self, id: i64, action: String, label: String, previews: bool, args: serde_json::Value) {
        if previews {
            self.result_title.set_text(&format!("{label}…"));
            self.result_text.buffer().set_text("");
            self.result_buttons.set_visible(false);
            self.side.set_visible_child_name("result");
        }
        let w = self.weak();
        act(async move {
            let r = client::run(id, &action, args).await;
            let Some(p) = w.upgrade() else { return };
            match r {
                Ok(o) => p.outcome(o, &label, previews),
                Err(e) => {
                    if previews {
                        p.back_to_preview();
                    }
                    p.toast(&e, true);
                }
            }
        });
    }

    fn outcome(&self, o: Outcome, label: &str, previews: bool) {
        if !o.ok {
            if previews {
                self.back_to_preview();
            }
            self.toast(if o.message.is_empty() { "That didn't work" } else { &o.message }, true);
            return;
        }
        if o.close {
            self.hide();
            return;
        }
        if previews && let Some(t) = o.text {
            self.result_title.set_text(&if o.message.is_empty() { label.to_string() } else { format!("{label} · {}", o.message) });
            self.result_text.buffer().set_text(&t);
            *self.result.borrow_mut() = Some(Result { text: t });
            self.result_buttons.set_visible(true);
            self.side.set_visible_child_name("result");
            self.hint.set_text("Ctrl C copies · ↵ pastes · Esc goes back");
            return;
        }
        self.back_to_preview();
        if !o.message.is_empty() {
            self.toast(&o.message, false);
        }
        self.refresh(true);
    }

    fn result_use(&self, paste: bool) {
        let Some(text) = self.result.borrow().as_ref().map(|r| r.text.clone()) else { return };
        if paste {
            self.hide();
        }
        let w = self.weak();
        act(async move {
            let r = client::copy_text(&text, paste).await;
            if !paste && let Some(p) = w.upgrade() {
                match r {
                    Ok(_) => {
                        p.back_to_preview();
                        p.toast("Copied", false);
                        p.refresh(true);
                    }
                    Err(e) => p.toast(&e, true),
                }
            }
        });
    }

    fn submit_target(&self) {
        let Some((id, action)) = self.target_for.borrow_mut().take() else { return };
        let target = self.target.text().to_string();
        if target.trim().is_empty() {
            *self.target_for.borrow_mut() = Some((id, action));
            return;
        }
        self.back_to_preview();
        self.run_with(id, action, "Add to a note".into(), false, serde_json::json!({ "target": target }));
    }

    fn save_dialog(&self, entry: Entry) {
        self.hide();
        act(async move {
            let (name, bytes) = if let Some(img) = &entry.image {
                let ext = img.rsplit('.').next().unwrap_or("png").to_string();
                let name = format!("Clipboard {}.{ext}", local(entry.created, "%Y-%m-%d %H.%M.%S"));
                (name, std::fs::read(img).ok())
            } else {
                let d = client::detail(entry.id).await.ok();
                let ext = match entry.kind {
                    Kind::Markdown => "md",
                    Kind::Html => "html",
                    _ => "txt",
                };
                let name = format!("Clipboard {}.{ext}", local(entry.created, "%Y-%m-%d %H.%M"));
                (name, d.map(|d| d.text.into_bytes()))
            };
            let Some(bytes) = bytes else { return };
            let dialog = gtk::FileDialog::builder().title("Save").initial_name(name).modal(true).build();
            if let Ok(file) = dialog.save_future(None::<&gtk::Window>).await
                && let Some(path) = file.path()
                && let Err(e) = std::fs::write(&path, bytes)
            {
                eprintln!("nyx-clipboard: can't save {}: {e}", path.display());
            }
        });
    }

    fn show_help(&self) {
        let mut t = String::new();
        for (what, keys) in SHORTCUTS {
            t.push_str(&format!("{what:<31} {keys}\n"));
        }
        self.result_title.set_text("Every key");
        self.result_text.buffer().set_text(t.trim_end());
        *self.result.borrow_mut() = None;
        self.result_buttons.set_visible(false);
        self.side.set_visible_child_name("result");
        self.hint.set_text("Esc goes back");
    }

    /// Keys for the whole popup. Typing goes to the search field.
    fn key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use glib::Propagation::{Proceed, Stop};
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        let page = self.side.visible_child_name().unwrap_or_default();
        match page.as_str() {
            "actions" => {
                return match key {
                    gdk::Key::Escape => {
                        self.disarm();
                        self.back_to_preview();
                        Stop
                    }
                    gdk::Key::Down => {
                        self.move_action(1);
                        Stop
                    }
                    gdk::Key::Up => {
                        self.move_action(-1);
                        Stop
                    }
                    gdk::Key::Return | gdk::Key::KP_Enter => {
                        self.run_selected_action();
                        Stop
                    }
                    _ => Proceed,
                };
            }
            "result" => {
                return match key {
                    gdk::Key::Escape | gdk::Key::BackSpace => {
                        self.back_to_preview();
                        Stop
                    }
                    gdk::Key::c if ctrl => {
                        self.result_use(false);
                        Stop
                    }
                    gdk::Key::Return | gdk::Key::KP_Enter => {
                        self.result_use(true);
                        Stop
                    }
                    _ => Proceed,
                };
            }
            "editor" => {
                return match key {
                    gdk::Key::Escape => {
                        self.back_to_preview();
                        Stop
                    }
                    gdk::Key::s | gdk::Key::Return if ctrl => {
                        self.save_snippet();
                        Stop
                    }
                    _ => Proceed,
                };
            }
            "target" => {
                return match key {
                    gdk::Key::Escape => {
                        self.back_to_preview();
                        Stop
                    }
                    gdk::Key::Return | gdk::Key::KP_Enter => {
                        self.submit_target();
                        Stop
                    }
                    _ => Proceed,
                };
            }
            _ => {}
        }
        if alt && let Some(n) = key.to_unicode().and_then(|c| c.to_digit(10)).filter(|n| (1..=9).contains(n)) {
            self.paste_nth(n as usize - 1);
            return Stop;
        }
        match key {
            gdk::Key::Escape => {
                if !self.search.text().is_empty() {
                    self.search.set_text("");
                } else {
                    self.hide();
                }
                Stop
            }
            gdk::Key::Down => {
                self.move_by(1);
                Stop
            }
            gdk::Key::Up => {
                self.move_by(-1);
                Stop
            }
            gdk::Key::Page_Down => {
                self.move_by(8);
                Stop
            }
            gdk::Key::Page_Up => {
                self.move_by(-8);
                Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter if ctrl => {
                self.open_actions();
                Stop
            }
            gdk::Key::Return | gdk::Key::KP_Enter => {
                self.activate(shift);
                Stop
            }
            gdk::Key::Tab | gdk::Key::ISO_Left_Tab => {
                self.cycle_tab(shift || key == gdk::Key::ISO_Left_Tab);
                Stop
            }
            gdk::Key::_1 if ctrl => {
                self.set_tab(Tab::Recent);
                Stop
            }
            gdk::Key::_2 if ctrl => {
                self.set_tab(Tab::Pinned);
                Stop
            }
            gdk::Key::_3 if ctrl => {
                self.set_tab(Tab::Snippets);
                Stop
            }
            gdk::Key::Menu => {
                self.open_actions();
                Stop
            }
            gdk::Key::F10 if shift => {
                self.open_actions();
                Stop
            }
            gdk::Key::F1 | gdk::Key::question if key == gdk::Key::F1 || self.search.text().is_empty() => {
                self.show_help();
                Stop
            }
            gdk::Key::comma if ctrl => {
                self.open_settings();
                Stop
            }
            gdk::Key::space if ctrl => {
                self.toggle_mark();
                self.show_preview();
                Stop
            }
            gdk::Key::P if ctrl && shift => {
                self.toggle_private();
                Stop
            }
            gdk::Key::p if ctrl => {
                self.toggle_pin();
                Stop
            }
            gdk::Key::s if ctrl => {
                self.toggle_favorite();
                Stop
            }
            gdk::Key::c if ctrl && self.search.selection_bounds().is_none() => {
                self.copy_selected();
                Stop
            }
            gdk::Key::n if ctrl => {
                self.edit_snippet(None);
                Stop
            }
            gdk::Key::e if ctrl => {
                if let Some(Item::Snippet(s)) = self.selected() {
                    self.edit_snippet(Some(s));
                }
                Stop
            }
            gdk::Key::Delete if ctrl || self.search.text().is_empty() => {
                self.delete_selected();
                Stop
            }
            gdk::Key::Home if ctrl => {
                self.select_index(0);
                Stop
            }
            _ => Proceed,
        }
    }
}

fn find_named(w: &gtk::Widget, name: &str) -> Option<gtk::Widget> {
    if w.widget_name() == name {
        return Some(w.clone());
    }
    let mut c = w.first_child();
    while let Some(child) = c {
        if let Some(f) = find_named(&child, name) {
            return Some(f);
        }
        c = child.next_sibling();
    }
    None
}

/// A scrolled area that doesn't ask for its content's natural width, so long text or a
/// large picture never widens the card.
fn scrolled(child: &impl IsA<gtk::Widget>, h: gtk::PolicyType, v: gtk::PolicyType) -> gtk::ScrolledWindow {
    let s = gtk::ScrolledWindow::new();
    s.set_hscrollbar_policy(h);
    s.set_vscrollbar_policy(v);
    s.set_child(Some(child));
    s
}

fn build_preview() -> (gtk::Box, Preview, gtk::Button) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("cb-prev");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("cb-pm");
    let pi = icon("text-x-generic-symbolic", 15);
    head.append(&pi);
    let meta = label("", "cb-pm-text");
    head.append(&meta);
    page.append(&head);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 6);
    body.add_css_class("cb-pbody-box");
    let scroll = scrolled(&body, gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    page.append(&scroll);
    let picture = gtk::Picture::new();
    picture.set_content_fit(gtk::ContentFit::Contain);
    picture.set_can_shrink(true);
    picture.add_css_class("cb-ppic");
    let picture_box = scrolled(&picture, gtk::PolicyType::External, gtk::PolicyType::External);
    picture_box.set_vexpand(true);
    picture_box.set_visible(false);
    page.append(&picture_box);
    let facts = gtk::Grid::new();
    facts.add_css_class("cb-facts");
    facts.set_row_spacing(6);
    facts.set_column_spacing(14);
    page.append(&facts);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.add_css_class("cb-pacts");
    let paste = gtk::Button::new();
    let pb = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    let enter = gtk::Label::new(Some("↵"));
    enter.add_css_class("cb-enter");
    pb.append(&enter);
    let paste_label = gtk::Label::new(Some("Paste"));
    pb.append(&paste_label);
    paste.set_child(Some(&pb));
    paste.add_css_class("suggested-action");
    paste.set_focus_on_click(false);
    paste.set_tooltip_text(Some("Paste into the window you were in (Enter)"));
    bar.append(&paste);
    let plain = gtk::Button::new();
    let plain_label = gtk::Label::new(Some("Paste as plain text"));
    plain.set_child(Some(&plain_label));
    plain.set_focus_on_click(false);
    bar.append(&plain);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let pin = gtk::Button::from_icon_name("view-pin-symbolic");
    let favorite = gtk::Button::from_icon_name("non-starred-symbolic");
    let (edit, _) = icon_button("document-edit-symbolic", "Edit");
    edit.set_tooltip_text(Some("Edit the snippet (Ctrl E)"));
    let (actions, actions_label) = icon_button("view-more-horizontal-symbolic", "Actions");
    actions.set_tooltip_text(Some("Change it, send it, ask the Assistant (Ctrl Enter)"));
    for b in [&pin, &favorite] {
        b.add_css_class("icon-button");
        b.add_css_class("cb-pact-icon");
        b.set_focus_on_click(false);
    }
    for b in [&edit, &actions] {
        b.add_css_class("flat");
    }
    bar.append(&pin);
    bar.append(&favorite);
    bar.append(&edit);
    bar.append(&actions);
    page.append(&bar);
    let preview = Preview {
        icon: pi,
        meta,
        scroll,
        body,
        picture_box,
        picture,
        facts,
        bar,
        paste_label,
        plain,
        plain_label,
        pin,
        favorite,
        edit,
        actions,
        actions_label,
        left: RefCell::default(),
    };
    (page, preview, paste)
}

fn build_pane() -> (gtk::Box, Pane, gtk::Button) {
    let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.add_css_class("cb-prev");
    page.add_css_class("cb-actp");
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("cb-afield");
    let field = gtk::SearchEntry::new();
    field.set_hexpand(true);
    field.set_placeholder_text(Some("Search actions"));
    field.set_search_delay(30);
    field.update_property(&[gtk::accessible::Property::Label("Search actions")]);
    head.append(&field);
    let about = gtk::Label::new(None);
    about.add_css_class("cb-ah");
    about.set_ellipsize(gtk::pango::EllipsizeMode::End);
    about.set_max_width_chars(30);
    about.set_width_chars(1);
    head.append(&about);
    page.append(&head);
    let grid = gtk::Box::new(gtk::Orientation::Horizontal, 16);
    grid.add_css_class("cb-agrid");
    grid.set_homogeneous(true);
    let columns = [gtk::Box::new(gtk::Orientation::Vertical, 4), gtk::Box::new(gtk::Orientation::Vertical, 4)];
    for c in &columns {
        c.set_valign(gtk::Align::Start);
        grid.append(c);
    }
    let scroll = scrolled(&grid, gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroll.set_vexpand(true);
    page.append(&scroll);
    let bar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    bar.add_css_class("cb-pacts");
    let (back, _) = icon_button("go-previous-symbolic", "Back");
    back.add_css_class("flat");
    bar.append(&back);
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    bar.append(&spacer);
    let hint = gtk::Label::new(Some("↵ runs it · Esc goes back"));
    hint.add_css_class("cb-hint");
    bar.append(&hint);
    page.append(&bar);
    let pane = Pane { field, about, scroll, columns, items: RefCell::default(), selected: Cell::new(None), subject: RefCell::default() };
    (page, pane, back)
}

pub fn build(app: &gtk::Application, only: Option<String>) -> Rc<Popup> {
    let window = gtk::Window::new();
    window.set_application(Some(app));
    window.add_css_class("nyx-clipboard");
    window.set_title(Some("Clipboard"));
    window.set_decorated(false);
    if gtk4_layer_shell::is_supported() {
        window.init_layer_shell();
        window.set_namespace(Some("nyx-clipboard"));
        window.set_layer(Layer::Overlay);
        for e in [Edge::Top, Edge::Bottom, Edge::Left, Edge::Right] {
            window.set_anchor(e, true);
        }
        window.set_exclusive_zone(-1);
        // A test copy (NYX_CLIPBOARD_NO_KEYBOARD) never takes the keyboard.
        window.set_keyboard_mode(if std::env::var_os("NYX_CLIPBOARD_NO_KEYBOARD").is_some() {
            KeyboardMode::None
        } else {
            KeyboardMode::Exclusive
        });
    }

    let root = gtk::Box::new(gtk::Orientation::Vertical, 0);
    root.add_css_class("cb-root");
    window.set_child(Some(&root));

    let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
    card.add_css_class("cb-card");
    card.set_size_request(WIDTH, HEIGHT);
    card.set_halign(gtk::Align::Fill);
    card.set_valign(gtk::Align::Fill);
    card.set_vexpand(true);
    card.set_hexpand(true);
    root.append(&card);

    // Search, the three views with their counts, and private mode.
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("cb-head");
    let search = gtk::SearchEntry::new();
    search.set_hexpand(true);
    search.set_search_delay(60);
    search.add_css_class("cb-search");
    search.update_property(&[gtk::accessible::Property::Label("Search the clipboard")]);
    head.append(&search);
    let seg = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    seg.add_css_class("cb-tabs");
    let mut tabs = Vec::new();
    let mut group: Option<gtk::ToggleButton> = None;
    for (t, name, key) in [(Tab::Recent, "Recent", "Ctrl 1"), (Tab::Pinned, "Pinned", "Ctrl 2"), (Tab::Snippets, "Snippets", "Ctrl 3")] {
        let b = gtk::ToggleButton::new();
        let bx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        bx.append(&gtk::Label::new(Some(name)));
        let n = gtk::Label::new(None);
        n.add_css_class("cb-count");
        bx.append(&n);
        b.set_child(Some(&bx));
        b.set_tooltip_text(Some(&format!("{name} ({key})")));
        b.set_focus_on_click(false);
        b.update_property(&[gtk::accessible::Property::Label(name)]);
        if let Some(g) = &group {
            b.set_group(Some(g));
        } else {
            group = Some(b.clone());
        }
        seg.append(&b);
        tabs.push((t, b, n));
    }
    head.append(&seg);
    let private = gtk::ToggleButton::new();
    let pbx = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    pbx.append(&icon("view-conceal-symbolic", 15));
    pbx.append(&gtk::Label::new(Some("Private")));
    private.set_child(Some(&pbx));
    private.set_tooltip_text(Some("Private mode: stop saving what you copy until you turn it off (Ctrl Shift P)"));
    private.add_css_class("cb-priv");
    private.set_focus_on_click(false);
    head.append(&private);
    card.append(&head);

    // Kinds, as chips, and the prefixes that do the same in the search.
    let kinds_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    kinds_row.add_css_class("cb-kinds");
    let mut kinds = Vec::new();
    let mut kgroup: Option<gtk::ToggleButton> = None;
    for (k, name) in KINDS {
        let b = gtk::ToggleButton::with_label(name);
        b.add_css_class("cb-kind");
        b.set_focus_on_click(false);
        if let Some(g) = &kgroup {
            b.set_group(Some(g));
        } else {
            kgroup = Some(b.clone());
        }
        kinds_row.append(&b);
        kinds.push((k, b));
    }
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    kinds_row.append(&spacer);
    let tip = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    tip.add_css_class("cb-tip");
    tip.append(&gtk::Label::new(Some("or type")));
    for p in ["img:", "url:", "fav:"] {
        let c = gtk::Label::new(Some(p));
        c.add_css_class("cb-tip-code");
        tip.append(&c);
    }
    kinds_row.append(&tip);
    card.append(&kinds_row);

    let banner = gtk::Label::new(None);
    banner.add_css_class("cb-banner");
    banner.set_xalign(0.0);
    banner.set_wrap(true);
    banner.set_visible(false);
    card.append(&banner);

    // The list and, beside it, the chosen item.
    let body = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    body.add_css_class("cb-body");
    body.set_vexpand(true);
    let list_side = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list_side.add_css_class("cb-listside");
    list_side.set_size_request(LIST_WIDTH, -1);
    let list = gtk::ListBox::new();
    list.add_css_class("cb-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(false);
    list.update_property(&[gtk::accessible::Property::Label("Clipboard history")]);
    let scroller = scrolled(&list, gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    scroller.set_vexpand(true);
    list_side.append(&scroller);
    let empty = gtk::Label::new(None);
    empty.add_css_class("cb-empty");
    empty.set_wrap(true);
    empty.set_justify(gtk::Justification::Center);
    empty.set_vexpand(true);
    empty.set_visible(false);
    list_side.append(&empty);
    body.append(&list_side);
    body.append(&gtk::Separator::new(gtk::Orientation::Vertical));

    let side = gtk::Stack::new();
    side.set_hexpand(true);
    side.set_transition_type(gtk::StackTransitionType::Crossfade);
    // Reduce motion (the desktop's animations off) means no crossfade either.
    let animate = gtk::Settings::default().is_none_or(|s| s.is_gtk_enable_animations());
    side.set_transition_duration(if animate { 120 } else { 0 });

    let (preview_page, preview, paste_b) = build_preview();
    side.add_named(&preview_page, Some("preview"));
    let (pane_page, pane, pane_back) = build_pane();
    side.add_named(&pane_page, Some("actions"));

    // An action's result (Assistant, OCR) or the shortcuts.
    let result_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    result_page.add_css_class("cb-page");
    let result_title = label("", "cb-page-title");
    result_page.append(&result_title);
    let result_text = gtk::TextView::new();
    result_text.set_editable(false);
    result_text.set_cursor_visible(false);
    result_text.set_wrap_mode(gtk::WrapMode::WordChar);
    result_text.set_left_margin(10);
    result_text.set_right_margin(10);
    result_text.set_top_margin(8);
    result_text.set_bottom_margin(8);
    result_text.add_css_class("cb-result");
    let rs = scrolled(&result_text, gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    rs.set_vexpand(true);
    result_page.append(&rs);
    let rb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    rb.add_css_class("cb-pacts");
    let (back, _) = icon_button("go-previous-symbolic", "Back");
    back.add_css_class("flat");
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    let copy_b = gtk::Button::with_label("Copy");
    let use_paste = gtk::Button::with_label("Paste");
    use_paste.add_css_class("suggested-action");
    let use_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for b in [&back, &copy_b, &use_paste] {
        b.set_focus_on_click(false);
    }
    rb.append(&back);
    rb.append(&spacer);
    use_buttons.append(&copy_b);
    use_buttons.append(&use_paste);
    rb.append(&use_buttons);
    result_page.append(&rb);
    side.add_named(&result_page, Some("result"));

    // Snippet editor.
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 8);
    editor.add_css_class("cb-page");
    let eh = label("Snippet", "cb-editor-title");
    editor.append(&eh);
    let editor_name = gtk::Entry::new();
    editor_name.set_placeholder_text(Some("Name"));
    editor.append(&editor_name);
    let editor_body = gtk::TextView::new();
    editor_body.set_wrap_mode(gtk::WrapMode::WordChar);
    editor_body.set_left_margin(10);
    editor_body.set_right_margin(10);
    editor_body.set_top_margin(8);
    editor_body.set_bottom_margin(8);
    editor_body.add_css_class("cb-result");
    editor_body.update_property(&[gtk::accessible::Property::Label("Snippet text")]);
    let es = scrolled(&editor_body, gtk::PolicyType::Never, gtk::PolicyType::Automatic);
    es.set_vexpand(true);
    editor.append(&es);
    let fills = gtk::FlowBox::new();
    fills.add_css_class("cb-fills");
    fills.set_selection_mode(gtk::SelectionMode::None);
    fills.set_column_spacing(5);
    fills.set_row_spacing(4);
    fills.set_max_children_per_line(6);
    fills.append(&gtk::Label::new(Some("Filled in when pasted:")));
    for f in ["{date}", "{time}", "{datetime}", "{weekday}", "{clipboard}"] {
        let c = gtk::Label::new(Some(f));
        c.add_css_class("cb-fill");
        c.set_halign(gtk::Align::Start);
        fills.append(&c);
    }
    editor.append(&fills);
    let eb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    eb.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.add_css_class("flat");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    eb.append(&cancel);
    eb.append(&save);
    editor.append(&eb);
    side.add_named(&editor, Some("editor"));

    // Asking for a target (which note).
    let target_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    target_page.add_css_class("cb-page");
    let target_title = label("", "cb-page-title");
    target_title.set_wrap(true);
    target_page.append(&target_title);
    let target = gtk::Entry::new();
    target_page.append(&target);
    side.add_named(&target_page, Some("target"));
    body.append(&side);
    card.append(&body);

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.add_css_class("cb-foot");
    let hint = label("", "cb-hint");
    let toast = label("", "cb-toast");
    toast.set_visible(false);
    foot.append(&hint);
    foot.append(&toast);
    let (settings_b, _) = icon_button("preferences-system-symbolic", "Settings");
    settings_b.add_css_class("flat");
    settings_b.add_css_class("cb-set");
    settings_b.set_tooltip_text(Some("Clipboard's settings: history, privacy, codes (Ctrl ,)"));
    foot.append(&settings_b);
    card.append(&foot);

    let popup = Rc::new(Popup {
        window: window.clone(),
        card: card.clone(),
        search: search.clone(),
        tabs,
        private: private.clone(),
        kinds_row,
        kinds,
        banner,
        list_side,
        side: side.clone(),
        list: list.clone(),
        scroller: scroller.clone(),
        empty,
        hint,
        toast,
        preview,
        pane,
        result_title,
        result_text,
        result_buttons: use_buttons.clone(),
        editor_name,
        editor_body,
        target_title,
        target,
        tab: Cell::new(Tab::Recent),
        kind: Cell::new(None),
        items: RefCell::default(),
        marked: RefCell::default(),
        details: RefCell::default(),
        offset: Cell::new(0),
        exhausted: Cell::new(false),
        loading: Cell::new(false),
        generation: Cell::new(0),
        only,
        narrow: Cell::new(false),
        listening: RefCell::default(),
        ticker: RefCell::default(),
        toast_timer: RefCell::default(),
        pending_refresh: Cell::new(false),
        result: RefCell::default(),
        editing: Cell::new(0),
        target_for: RefCell::default(),
        armed: RefCell::default(),
        status: RefCell::default(),
        me: RefCell::default(),
    });
    *popup.me.borrow_mut() = Rc::downgrade(&popup);

    let w = Rc::downgrade(&popup);
    search.connect_search_changed(move |_| {
        if let Some(p) = w.upgrade()
            && p.is_open()
        {
            p.marked.borrow_mut().clear();
            p.refresh(true);
        }
    });
    for (t, b, _) in &popup.tabs {
        let w = Rc::downgrade(&popup);
        let t = *t;
        b.connect_clicked(move |b| {
            if let Some(p) = w.upgrade() {
                if b.is_active() {
                    p.set_tab(t);
                } else if p.tab.get() == t {
                    b.set_active(true);
                }
            }
        });
    }
    for (k, b) in &popup.kinds {
        let w = Rc::downgrade(&popup);
        let k = *k;
        b.connect_clicked(move |b| {
            if let Some(p) = w.upgrade() {
                if b.is_active() {
                    p.set_kind(k);
                } else if p.kind.get() == k {
                    b.set_active(true);
                }
            }
        });
    }
    let w = Rc::downgrade(&popup);
    private.connect_clicked(move |b| {
        if let Some(p) = w.upgrade()
            && b.is_active() != p.status.borrow().private
        {
            p.toggle_private();
        }
    });
    let w = Rc::downgrade(&popup);
    list.connect_row_selected(move |_, _| {
        if let Some(p) = w.upgrade() {
            p.show_preview();
        }
    });
    let w = Rc::downgrade(&popup);
    list.connect_row_activated(move |_, _| {
        if let Some(p) = w.upgrade() {
            p.activate(false);
        }
    });
    let w = Rc::downgrade(&popup);
    list.set_header_func(move |row, before| {
        let Some(p) = w.upgrade() else { return };
        let items = p.items.borrow();
        let day_of = |r: &gtk::ListBoxRow| match items.get(r.index() as usize) {
            Some(Item::Entry(e)) => Some(day(e.last_used)),
            _ => None,
        };
        let this = if p.tab.get() == Tab::Recent { day_of(row) } else { None };
        match this {
            Some(d) if before.and_then(day_of).as_ref() != Some(&d) => row.set_header(Some(&label(&d, "cb-day"))),
            _ => row.set_header(None::<&gtk::Widget>),
        }
    });
    let w = Rc::downgrade(&popup);
    scroller.vadjustment().connect_value_changed(move |a| {
        if a.value() + a.page_size() >= a.upper() - 200.0
            && let Some(p) = w.upgrade()
        {
            p.more();
        }
    });
    let pv = &popup.preview;
    let w = Rc::downgrade(&popup);
    paste_b.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.activate(false);
        }
    });
    let w = Rc::downgrade(&popup);
    pv.plain.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            match p.selected() {
                Some(Item::Entry(_)) => p.activate(true),
                _ => p.copy_selected(),
            }
        }
    });
    let w = Rc::downgrade(&popup);
    pv.pin.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.toggle_pin();
        }
    });
    let w = Rc::downgrade(&popup);
    pv.favorite.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.toggle_favorite();
        }
    });
    let w = Rc::downgrade(&popup);
    pv.edit.connect_clicked(move |_| {
        if let Some(p) = w.upgrade()
            && let Some(Item::Snippet(s)) = p.selected()
        {
            p.edit_snippet(Some(s));
        }
    });
    let w = Rc::downgrade(&popup);
    pv.actions.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.open_actions();
        }
    });
    let w = Rc::downgrade(&popup);
    popup.pane.field.connect_search_changed(move |_| {
        if let Some(p) = w.upgrade() {
            p.filter_actions();
        }
    });
    for b in [&pane_back, &back, &cancel] {
        let w = Rc::downgrade(&popup);
        b.connect_clicked(move |_| {
            if let Some(p) = w.upgrade() {
                p.back_to_preview();
            }
        });
    }
    let w = Rc::downgrade(&popup);
    copy_b.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.result_use(false);
        }
    });
    let w = Rc::downgrade(&popup);
    use_paste.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.result_use(true);
        }
    });
    let w = Rc::downgrade(&popup);
    save.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.save_snippet();
        }
    });
    let w = Rc::downgrade(&popup);
    popup.target.connect_activate(move |_| {
        if let Some(p) = w.upgrade() {
            p.submit_target();
        }
    });
    let w = Rc::downgrade(&popup);
    settings_b.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.open_settings();
        }
    });

    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let w = Rc::downgrade(&popup);
    keys.connect_key_pressed(move |_, key, _, state| match w.upgrade() {
        Some(p) => p.key(key, state),
        None => glib::Propagation::Proceed,
    });
    window.add_controller(keys);

    // A click beside the card closes the popup.
    let click = gtk::GestureClick::new();
    click.set_button(0);
    let w = Rc::downgrade(&popup);
    click.connect_pressed(move |g, _, x, y| {
        let Some(p) = w.upgrade() else { return };
        let Some(root) = g.widget() else { return };
        let inside = root.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|hit| hit == p.card || hit.is_ancestor(&p.card));
        if !inside {
            p.hide();
        }
    });
    root.add_controller(click);
    popup
}

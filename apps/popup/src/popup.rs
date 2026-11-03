//! The popup: a layer-shell overlay over the focused screen holding one solid card,
//! placed at the screen's centre or at the pointer. A click beside the card closes it.
//! Typing searches; arrows move; Enter pastes. It stays in memory while hidden but
//! listens to nothing until it's opened.

use crate::{client, thumbs};
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use nyx_clipboard_core::actions::{Action, Group, Outcome};
use nyx_clipboard_core::model::{CodeInfo, Detail, Entry, Kind, Status, Transient};
use nyx_clipboard_core::prefs::{Position, Settings};
use nyx_clipboard_core::query::{Search, View};
use nyx_clipboard_core::snippet::Snippet;
use nyx_clipboard_core::now;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::{Rc, Weak};
use std::time::Duration;

pub const WIDTH: i32 = 560;
const MAX_LIST: i32 = 480;
const PAGE: u32 = 50;

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

pub struct Popup {
    window: gtk::Window,
    card: gtk::Box,
    search: gtk::SearchEntry,
    tabs: Vec<(Tab, gtk::ToggleButton)>,
    private: gtk::ToggleButton,
    banner: gtk::Label,
    stack: gtk::Stack,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    hint: gtk::Label,
    toast: gtk::Label,
    result_title: gtk::Label,
    result_text: gtk::TextView,
    result_buttons: gtk::Box,
    editor_name: gtk::Entry,
    editor_body: gtk::TextView,
    target_title: gtk::Label,
    target: gtk::Entry,
    tab: Cell<Tab>,
    items: RefCell<Vec<Item>>,
    marked: RefCell<Vec<i64>>,
    details: RefCell<HashMap<i64, Detail>>,
    offset: Cell<u32>,
    exhausted: Cell<bool>,
    loading: Cell<bool>,
    generation: Cell<u64>,
    only: Option<String>,
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

fn ago(ts: f64) -> String {
    let s = (now() - ts).max(0.0);
    if s < 60.0 {
        "now".into()
    } else if s < 3600.0 {
        format!("{} min", (s / 60.0) as i64)
    } else if s < 86_400.0 {
        format!("{} h", (s / 3600.0) as i64)
    } else {
        nyx_clipboard_core::snippet::local(ts, "%b %-d")
    }
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
    ("Mark for pasting together", "Ctrl + Space"),
    ("Copy without pasting", "Ctrl + C"),
    ("Actions", "Menu · Shift + F10 · right-click"),
    ("Pin or unpin", "Ctrl + P"),
    ("Add to favorites", "Ctrl + S"),
    ("Delete", "Ctrl + Delete"),
    ("Recent · Pinned · Snippets", "Tab · Ctrl + 1 2 3"),
    ("New snippet · Edit snippet", "Ctrl + N · Ctrl + E"),
    ("Private mode", "Ctrl + Shift + P"),
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
            self.set_tab(tab);
        }
    }

    fn place(&self) {
        let settings = Settings::load();
        let monitor = match &self.only {
            Some(name) => monitor_named(name),
            None => focused_monitor(),
        };
        self.window.set_monitor(monitor.as_ref());
        self.card.set_margin_start(0);
        self.card.set_margin_top(0);
        let at_pointer = settings.position == Position::Cursor && self.only.is_none();
        match pointer().filter(|_| at_pointer) {
            Some(((x, y), (w, h))) => {
                self.card.set_halign(gtk::Align::Start);
                self.card.set_valign(gtk::Align::Start);
                self.card.set_margin_start((x - 24).clamp(8, (w - WIDTH - 8).max(8)));
                self.card.set_margin_top((y + 12).clamp(8, (h - 620).max(8)));
            }
            None => {
                self.card.set_halign(gtk::Align::Center);
                self.card.set_valign(gtk::Align::Center);
            }
        }
    }

    pub fn show(&self) {
        if self.is_open() {
            self.focus_search();
            return;
        }
        self.place();
        self.stack.set_visible_child_name("list");
        self.set_tab_quiet(Tab::Recent);
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
        // For trying it on a test output: NYX_CLIPBOARD_TEST_VIEW=pinned|snippets|help|actions|editor.
        if self.only.is_some()
            && let Ok(view) = std::env::var("NYX_CLIPBOARD_TEST_VIEW")
        {
            let w = self.weak();
            glib::timeout_add_local_once(Duration::from_millis(700), move || {
                let Some(p) = w.upgrade() else { return };
                match view.as_str() {
                    "pinned" => p.set_tab(Tab::Pinned),
                    "snippets" => p.set_tab(Tab::Snippets),
                    "help" => p.show_help(),
                    "actions" => p.open_actions(None),
                    "editor" => p.edit_snippet(None),
                    "images" => p.search.set_text("img: "),
                    _ => {}
                }
            });
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
        self.items.borrow_mut().clear();
        while let Some(c) = self.list.first_child() {
            self.list.remove(&c);
        }
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
            format!("{marked} marked · ↵ pastes them together · Ctrl+Space unmarks")
        } else if self.tab.get() == Tab::Snippets {
            "↵ Paste · Ctrl+N New · Ctrl+E Edit · Menu Actions · F1 Keys".to_string()
        } else {
            "↵ Paste · ⇧↵ Plain · Ctrl+P Pin · Menu Actions · F1 Keys".to_string()
        };
        self.hint.set_text(&text);
    }

    fn set_tab_quiet(&self, tab: Tab) {
        self.tab.set(tab);
        for (t, b) in &self.tabs {
            b.set_active(*t == tab);
        }
        self.search.set_placeholder_text(Some(match tab {
            Tab::Snippets => "Search snippets",
            _ => "Search the clipboard",
        }));
    }

    fn set_tab(&self, tab: Tab) {
        if self.tab.get() == tab {
            return;
        }
        self.set_tab_quiet(tab);
        self.marked.borrow_mut().clear();
        self.stack.set_visible_child_name("list");
        self.refresh(true);
        self.focus_search();
    }

    fn cycle_tab(&self, back: bool) {
        let order = [Tab::Recent, Tab::Pinned, Tab::Snippets];
        let i = order.iter().position(|t| *t == self.tab.get()).unwrap_or(0);
        let next = if back { (i + order.len() - 1) % order.len() } else { (i + 1) % order.len() };
        self.set_tab(order[next]);
    }

    fn queue_refresh(&self) {
        if self.pending_refresh.replace(true) {
            return;
        }
        let w = self.weak();
        glib::timeout_add_local_once(Duration::from_millis(120), move || {
            if let Some(p) = w.upgrade() {
                p.pending_refresh.set(false);
                if p.is_open() && p.stack.visible_child_name().as_deref() == Some("list") {
                    p.refresh(false);
                }
            }
        });
    }

    /// Read again from the start. `reset` moves the selection to the top.
    fn refresh(&self, reset: bool) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.offset.set(0);
        self.exhausted.set(false);
        let keep = if reset { None } else { self.selected_key() };
        let text = self.search.text().to_string();
        let tab = self.tab.get();
        let w = self.weak();
        act(async move {
            let status = client::status().await;
            let (codes, secrets) = if tab == Tab::Recent && text.is_empty() {
                (client::codes().await.unwrap_or_default(), client::transient().await.unwrap_or_default())
            } else {
                (Vec::new(), Vec::new())
            };
            let rows: std::result::Result<Vec<Item>, String> = match tab {
                Tab::Snippets => client::snippets(&text).await.map(|v| v.into_iter().map(Item::Snippet).collect()),
                _ => {
                    let view = if tab == Tab::Pinned { View::Pinned } else { View::Recent };
                    client::search(&Search { text: text.clone(), view, offset: 0, limit: PAGE, kind: None })
                        .await
                        .map(|v| v.into_iter().map(Item::Entry).collect())
                }
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
        if !s.watching {
            self.banner.set_text(&format!(
                "The clipboard isn't being watched{}",
                s.problem.as_deref().map(|p| format!(": {p}")).unwrap_or_default()
            ));
            self.banner.add_css_class("danger");
            self.banner.set_visible(true);
        } else if s.private {
            self.banner.set_text("Private mode: nothing new is saved");
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
        let text = self.search.text().to_string();
        let view = if self.tab.get() == Tab::Pinned { View::Pinned } else { View::Recent };
        let offset = self.offset.get();
        let w = self.weak();
        act(async move {
            let rows = client::search(&Search { text, view, offset, limit: PAGE, kind: None }).await.unwrap_or_default();
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
        let mut n = 0;
        for item in &items {
            let numbered = matches!(item, Item::Entry(_) | Item::Snippet(_));
            n += usize::from(numbered);
            let row = self.row(item, if numbered { n } else { 0 });
            self.list.append(&row);
        }
        let select = keep.and_then(|k| items.iter().position(|i| Self::key_of(i) == k)).unwrap_or(0);
        let none = items.is_empty();
        *self.items.borrow_mut() = items;
        self.empty.set_visible(none);
        self.scroller.set_visible(!none);
        self.empty.set_text(match (self.tab.get(), self.search.text().is_empty()) {
            (_, false) => "Nothing matches",
            (Tab::Recent, true) => "Nothing copied yet. Copy something and it shows up here.",
            (Tab::Pinned, true) => "No pinned items. Ctrl + P pins the selected item.",
            (Tab::Snippets, true) => "No snippets. Ctrl + N adds one; Save as snippet in an item's actions makes one from it.",
        });
        if let Some(r) = self.list.row_at_index(select as i32) {
            self.list.select_row(Some(&r));
        }
        self.update_hint();
    }

    /// A list row; `n` (1–9) labels it with its Alt + n key.
    fn row(&self, item: &Item, n: usize) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.add_css_class("cb-row");
        let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let glyph = gtk::Box::new(gtk::Orientation::Vertical, 0);
        glyph.add_css_class("cb-glyph");
        glyph.set_size_request(32, 32);
        glyph.set_valign(gtk::Align::Center);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 1);
        text.set_hexpand(true);
        text.set_valign(gtk::Align::Center);
        let (title, meta, icon, accent) = match item {
            Item::Code(c) => {
                row.add_css_class("cb-code");
                let from = if c.sites.is_empty() { c.app.clone() } else { format!("{} · {}", c.app, c.sites.join(", ")) };
                (c.code.clone(), format!("Verification code · {from} · {}", left(c.expires)), "dialog-password-symbolic", true)
            }
            Item::Secret(s) => {
                row.add_css_class("cb-secret");
                (format!("{} (not saved)", s.label), format!("Kept in memory · {}", left(s.expires)), "security-high-symbolic", false)
            }
            Item::Entry(e) => {
                let mut parts = vec![e.kind.label().to_string()];
                if let Some(a) = &e.app {
                    parts.push(a.rsplit('.').next().unwrap_or(a).to_string());
                }
                parts.push(ago(e.last_used));
                if e.kind == Kind::Image || e.size > 4096 {
                    parts.push(size(e.size));
                }
                (e.preview.clone(), parts.join(" · "), e.kind.icon(), false)
            }
            Item::Snippet(s) => (s.name.clone(), nyx_clipboard_core::classify::preview(&s.body), "insert-text-symbolic", false),
        };
        if let Item::Entry(e) = item
            && let Some(img) = &e.image
        {
            let pic = gtk::Image::new();
            pic.set_pixel_size(32);
            pic.set_vexpand(true);
            pic.add_css_class("cb-thumb");
            thumbs::load_icon(&pic, img, 64);
            glyph.append(&pic);
        } else {
            let i = gtk::Image::from_icon_name(icon);
            i.set_pixel_size(16);
            i.set_vexpand(true);
            if accent {
                i.add_css_class("accent");
            }
            glyph.append(&i);
        }
        b.append(&glyph);
        let t = label(&title, "cb-title");
        if matches!(item, Item::Code(_)) {
            t.add_css_class("cb-code-text");
        }
        text.append(&t);
        let m = label(&meta, "cb-meta");
        m.set_widget_name("meta");
        text.append(&m);
        b.append(&text);
        if let Item::Entry(e) = item {
            if self.marked.borrow().contains(&e.id) {
                row.add_css_class("marked");
                let check = gtk::Image::from_icon_name("object-select-symbolic");
                check.add_css_class("accent");
                b.append(&check);
            }
            if e.favorite {
                let s = gtk::Image::from_icon_name("starred-symbolic");
                s.add_css_class("cb-mark");
                s.set_tooltip_text(Some("Favorite"));
                b.append(&s);
            }
            if e.pinned {
                let s = gtk::Image::from_icon_name("view-pin-symbolic");
                s.add_css_class("cb-mark");
                s.set_tooltip_text(Some("Pinned"));
                b.append(&s);
            }
        }
        if (1..=9).contains(&n) {
            let k = label(&format!("Alt {n}"), "cb-key");
            k.set_hexpand(false);
            k.set_max_width_chars(-1);
            k.set_valign(gtk::Align::Center);
            b.append(&k);
        }
        row.set_child(Some(&b));
        // Hover previews: the whole text or a larger image, fetched when first wanted.
        if let Item::Entry(e) = item {
            row.set_has_tooltip(true);
            let id = e.id;
            let image = e.image.clone();
            let w = self.weak();
            row.connect_query_tooltip(move |row, _, _, _, tip| {
                let Some(p) = w.upgrade() else { return false };
                if let Some(img) = &image {
                    let pic = gtk::Picture::new();
                    pic.set_size_request(320, 220);
                    pic.set_content_fit(gtk::ContentFit::Contain);
                    thumbs::load(&pic, img, 360);
                    tip.set_custom(Some(&pic));
                    return true;
                }
                if let Some(d) = p.details.borrow().get(&id) {
                    let mut t: String = d.text.chars().take(1500).collect();
                    if d.text.chars().count() > 1500 || d.truncated {
                        t.push_str("\n…");
                    }
                    let l = gtk::Label::new(Some(&t));
                    l.set_wrap(true);
                    l.set_wrap_mode(gtk::pango::WrapMode::WordChar);
                    l.set_max_width_chars(70);
                    l.set_xalign(0.0);
                    l.add_css_class("cb-preview");
                    tip.set_custom(Some(&l));
                    return true;
                }
                let w2 = p.weak();
                let row = row.clone();
                act(async move {
                    if let Ok(d) = client::detail(id).await
                        && let Some(p) = w2.upgrade()
                    {
                        p.details.borrow_mut().insert(id, d);
                        row.trigger_tooltip_query();
                    }
                });
                false
            });
        }
        let right = gtk::GestureClick::new();
        right.set_button(3);
        let w = self.weak();
        let r2 = row.clone();
        right.connect_pressed(move |g, _, x, y| {
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(p) = w.upgrade() {
                p.list.select_row(Some(&r2));
                p.open_actions(Some((x, y)));
            }
        });
        row.add_controller(right);
        row
    }

    fn tick(&self) {
        let t = now();
        let mut expired = false;
        let items = self.items.borrow().clone();
        for (i, item) in items.iter().enumerate() {
            let (expires, prefix) = match item {
                Item::Code(c) => {
                    let from = if c.sites.is_empty() { c.app.clone() } else { format!("{} · {}", c.app, c.sites.join(", ")) };
                    (c.expires, format!("Verification code · {from}"))
                }
                Item::Secret(s) => (s.expires, "Kept in memory".to_string()),
                _ => continue,
            };
            if expires <= t {
                expired = true;
                continue;
            }
            if let Some(row) = self.list.row_at_index(i as i32)
                && let Some(meta) = find_named(row.upcast_ref(), "meta")
                && let Ok(l) = meta.downcast::<gtk::Label>()
            {
                l.set_text(&format!("{prefix} · {}", left(expires)));
            }
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

    /// Enter: paste what's selected (or everything marked).
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
                    Ok(_) => p.toast(if e.pinned { "Unpinned" } else { "Pinned" }, false),
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
                    Ok(_) => p.toast(if e.favorite { "Removed from favorites" } else { "Added to favorites" }, false),
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
                    act(async move {
                        let _ = client::delete_snippet(s.id).await;
                        if let Some(p) = w.upgrade() {
                            p.toast("Snippet deleted", false);
                            p.refresh(false);
                        }
                    });
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

    // ---- Snippet editor

    fn edit_snippet(&self, existing: Option<Snippet>) {
        let s = existing.unwrap_or_default();
        self.editing.set(s.id);
        self.editor_name.set_text(&s.name);
        self.editor_body.buffer().set_text(&s.body);
        self.stack.set_visible_child_name("editor");
        self.hint.set_text("Ctrl+S Save · Esc Cancel");
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
                        p.back_to_list();
                        p.set_tab(Tab::Snippets);
                        p.toast("Snippet saved", false);
                        p.refresh(false);
                    }
                    Err(e) => p.toast(&e, true),
                }
            }
        });
    }

    fn back_to_list(&self) {
        self.stack.set_visible_child_name("list");
        self.result.borrow_mut().take();
        self.target_for.borrow_mut().take();
        self.focus_search();
        self.update_hint();
    }

    // ---- Actions

    fn open_actions(&self, at: Option<(f64, f64)>) {
        let Some(item) = self.selected() else { return };
        let Some(row) = self.list.selected_row() else { return };
        match item {
            Item::Entry(e) => {
                let w = self.weak();
                act(async move {
                    let r = client::actions(e.id).await;
                    let Some(p) = w.upgrade() else { return };
                    match r {
                        Ok(list) => p.menu(&row, at, e, list),
                        Err(err) => p.toast(&err, true),
                    }
                });
            }
            Item::Snippet(s) => {
                let acts = [("paste", "Paste", "edit-paste-symbolic"), ("copy", "Copy", "edit-copy-symbolic"), ("edit", "Edit", "document-edit-symbolic"), ("delete", "Delete", "user-trash-symbolic")];
                let pop = self.popover(&row, at);
                let lb = gtk::ListBox::new();
                lb.add_css_class("cb-menu");
                for (id, l, i) in acts {
                    lb.append(&menu_row(id, l, i));
                }
                let w = self.weak();
                let pop2 = pop.clone();
                lb.connect_row_activated(move |_, r| {
                    pop2.popdown();
                    let Some(p) = w.upgrade() else { return };
                    match r.widget_name().as_str() {
                        "paste" => p.activate(false),
                        "copy" => p.copy_selected(),
                        "edit" => p.edit_snippet(Some(s.clone())),
                        "delete" => p.delete_selected(),
                        _ => {}
                    }
                });
                pop.set_child(Some(&lb));
                pop.popup();
                if let Some(r) = lb.row_at_index(0) {
                    r.grab_focus();
                }
            }
            _ => {}
        }
    }

    fn popover(&self, row: &gtk::ListBoxRow, at: Option<(f64, f64)>) -> gtk::Popover {
        let pop = gtk::Popover::new();
        pop.add_css_class("cb-popover");
        pop.set_parent(row);
        pop.set_has_arrow(false);
        pop.set_position(gtk::PositionType::Bottom);
        if let Some((x, y)) = at {
            pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
        }
        pop.connect_closed(|p| {
            let p = p.clone();
            glib::idle_add_local_once(move || p.unparent());
        });
        let w = self.weak();
        pop.connect_closed(move |_| {
            if let Some(p) = w.upgrade() {
                p.focus_search();
            }
        });
        pop
    }

    fn menu(&self, row: &gtk::ListBoxRow, at: Option<(f64, f64)>, entry: Entry, list: Vec<Action>) {
        let pop = self.popover(row, at);
        let scroll = gtk::ScrolledWindow::new();
        scroll.set_hscrollbar_policy(gtk::PolicyType::Never);
        scroll.set_propagate_natural_height(true);
        scroll.set_max_content_height(420);
        let lb = gtk::ListBox::new();
        lb.add_css_class("cb-menu");
        let mut group: Option<Group> = None;
        for a in &list {
            if group != Some(a.group) {
                if group.is_some() {
                    let sep = gtk::ListBoxRow::new();
                    sep.set_selectable(false);
                    sep.set_activatable(false);
                    sep.set_can_focus(false);
                    let l = label(&a.group.label().to_uppercase(), "cb-menu-group");
                    sep.set_child(Some(&l));
                    lb.append(&sep);
                }
                group = Some(a.group);
            }
            let r = menu_row(&a.id, &a.label, &a.icon);
            if a.destructive {
                r.add_css_class("danger");
            }
            lb.append(&r);
        }
        scroll.set_child(Some(&lb));
        let w = self.weak();
        let pop2 = pop.clone();
        let actions: HashMap<String, Action> = list.into_iter().map(|a| (a.id.clone(), a)).collect();
        lb.connect_row_activated(move |_, r| {
            let id = r.widget_name().to_string();
            let Some(a) = actions.get(&id).cloned() else { return };
            let Some(p) = w.upgrade() else { return };
            if a.destructive && !p.armed(&format!("action:{id}"), "Press again to delete") {
                return;
            }
            pop2.popdown();
            p.run_action(&entry, &a);
        });
        pop.set_child(Some(&scroll));
        pop.popup();
        if let Some(r) = lb.row_at_index(0) {
            r.grab_focus();
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
                self.stack.set_visible_child_name("target");
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
            self.stack.set_visible_child_name("result");
        }
        let w = self.weak();
        act(async move {
            let r = client::run(id, &action, args).await;
            let Some(p) = w.upgrade() else { return };
            match r {
                Ok(o) => p.outcome(o, &label, previews),
                Err(e) => {
                    if previews {
                        p.back_to_list();
                    }
                    p.toast(&e, true);
                }
            }
        });
    }

    fn outcome(&self, o: Outcome, label: &str, previews: bool) {
        if !o.ok {
            if previews {
                self.back_to_list();
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
            self.stack.set_visible_child_name("result");
            self.hint.set_text("Ctrl+C Copy · ↵ Paste · Esc Back");
            return;
        }
        self.back_to_list();
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
                        p.back_to_list();
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
        self.back_to_list();
        self.run_with(id, action, "Add to a note".into(), false, serde_json::json!({ "target": target }));
    }

    fn save_dialog(&self, entry: Entry) {
        self.hide();
        act(async move {
            let (name, bytes) = if let Some(img) = &entry.image {
                let ext = img.rsplit('.').next().unwrap_or("png").to_string();
                let name = format!("Clipboard {}.{ext}", nyx_clipboard_core::snippet::local(entry.created, "%Y-%m-%d %H.%M.%S"));
                (name, std::fs::read(img).ok())
            } else {
                let d = client::detail(entry.id).await.ok();
                let ext = match entry.kind {
                    Kind::Markdown => "md",
                    Kind::Html => "html",
                    Kind::Code => "txt",
                    _ => "txt",
                };
                let name = format!("Clipboard {}.{ext}", nyx_clipboard_core::snippet::local(entry.created, "%Y-%m-%d %H.%M"));
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
            t.push_str(&format!("{what:<28} {keys}\n"));
        }
        self.result_title.set_text("Keyboard shortcuts");
        self.result_text.buffer().set_text(t.trim_end());
        *self.result.borrow_mut() = None;
        self.result_buttons.set_visible(false);
        self.stack.set_visible_child_name("result");
        self.hint.set_text("Esc Back");
    }

    /// Keys for the whole popup. Typing goes to the search field.
    fn key(&self, key: gdk::Key, state: gdk::ModifierType) -> glib::Propagation {
        use glib::Propagation::{Proceed, Stop};
        let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
        let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
        let alt = state.contains(gdk::ModifierType::ALT_MASK);
        let page = self.stack.visible_child_name().unwrap_or_default();
        match page.as_str() {
            "result" => {
                return match key {
                    gdk::Key::Escape | gdk::Key::BackSpace => {
                        self.back_to_list();
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
                        self.back_to_list();
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
                        self.back_to_list();
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
                self.open_actions(None);
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
                self.open_actions(None);
                Stop
            }
            gdk::Key::F10 if shift => {
                self.open_actions(None);
                Stop
            }
            gdk::Key::F1 | gdk::Key::question if key == gdk::Key::F1 || self.search.text().is_empty() => {
                self.show_help();
                Stop
            }
            gdk::Key::space if ctrl => {
                self.toggle_mark();
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

fn menu_row(id: &str, text: &str, icon: &str) -> gtk::ListBoxRow {
    let r = gtk::ListBoxRow::new();
    r.set_widget_name(id);
    r.add_css_class("cb-menu-row");
    let b = gtk::Box::new(gtk::Orientation::Horizontal, 10);
    let i = gtk::Image::from_icon_name(icon);
    i.set_pixel_size(16);
    b.append(&i);
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.set_hexpand(true);
    b.append(&l);
    r.set_child(Some(&b));
    r
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
    card.set_size_request(WIDTH, -1);
    card.set_halign(gtk::Align::Center);
    card.set_valign(gtk::Align::Center);
    card.set_vexpand(true);
    card.set_hexpand(true);
    root.append(&card);

    // Search and the three views.
    let head = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    head.add_css_class("cb-head");
    let search = gtk::SearchEntry::new();
    search.set_hexpand(true);
    search.set_search_delay(60);
    search.add_css_class("cb-search");
    search.update_property(&[gtk::accessible::Property::Label("Search the clipboard")]);
    head.append(&search);
    let seg = gtk::Box::new(gtk::Orientation::Horizontal, 2);
    seg.add_css_class("cb-segmented");
    let mut tabs = Vec::new();
    let mut group: Option<gtk::ToggleButton> = None;
    for (t, icon, tip) in [
        (Tab::Recent, "document-open-recent-symbolic", "Recent (Ctrl+1)"),
        (Tab::Pinned, "view-pin-symbolic", "Pinned (Ctrl+2)"),
        (Tab::Snippets, "insert-text-symbolic", "Snippets (Ctrl+3)"),
    ] {
        let b = gtk::ToggleButton::new();
        b.set_icon_name(icon);
        b.set_tooltip_text(Some(tip));
        b.set_focus_on_click(false);
        b.update_property(&[gtk::accessible::Property::Label(tip)]);
        if let Some(g) = &group {
            b.set_group(Some(g));
        } else {
            group = Some(b.clone());
        }
        seg.append(&b);
        tabs.push((t, b));
    }
    head.append(&seg);
    let private = gtk::ToggleButton::new();
    private.set_icon_name("view-conceal-symbolic");
    private.set_tooltip_text(Some("Private mode: stop saving copies (Ctrl+Shift+P)"));
    private.add_css_class("cb-icon-button");
    private.set_focus_on_click(false);
    head.append(&private);
    card.append(&head);

    let banner = gtk::Label::new(None);
    banner.add_css_class("cb-banner");
    banner.set_xalign(0.0);
    banner.set_wrap(true);
    banner.set_visible(false);
    card.append(&banner);

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::Crossfade);
    stack.set_transition_duration(120);
    stack.set_vhomogeneous(false);
    stack.set_interpolate_size(true);

    let list_page = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let list = gtk::ListBox::new();
    list.add_css_class("cb-list");
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.set_activate_on_single_click(true);
    list.update_property(&[gtk::accessible::Property::Label("Clipboard history")]);
    let scroller = gtk::ScrolledWindow::new();
    scroller.set_hscrollbar_policy(gtk::PolicyType::Never);
    scroller.set_vscrollbar_policy(gtk::PolicyType::Automatic);
    scroller.set_propagate_natural_height(true);
    scroller.set_max_content_height(MAX_LIST);
    scroller.set_child(Some(&list));
    list_page.append(&scroller);
    let empty = gtk::Label::new(None);
    empty.add_css_class("cb-empty");
    empty.set_wrap(true);
    empty.set_justify(gtk::Justification::Center);
    empty.set_visible(false);
    list_page.append(&empty);
    stack.add_named(&list_page, Some("list"));

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
    let rs = gtk::ScrolledWindow::new();
    rs.set_hscrollbar_policy(gtk::PolicyType::Never);
    rs.set_propagate_natural_height(true);
    rs.set_min_content_height(120);
    rs.set_max_content_height(MAX_LIST - 60);
    rs.set_child(Some(&result_text));
    result_page.append(&rs);
    let rb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    rb.set_halign(gtk::Align::End);
    let back = gtk::Button::with_label("Back");
    let copy_b = gtk::Button::with_label("Copy");
    let paste_b = gtk::Button::with_label("Paste");
    paste_b.add_css_class("suggested-action");
    let use_buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    for b in [&back, &copy_b, &paste_b] {
        b.set_focus_on_click(false);
    }
    rb.append(&back);
    use_buttons.append(&copy_b);
    use_buttons.append(&paste_b);
    rb.append(&use_buttons);
    result_page.append(&rb);
    stack.add_named(&result_page, Some("result"));

    // Snippet editor.
    let editor = gtk::Box::new(gtk::Orientation::Vertical, 8);
    editor.add_css_class("cb-page");
    editor.append(&label("Snippet: {date} {time} {datetime} {weekday} {clipboard} are filled in when it's pasted", "cb-page-title"));
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
    let es = gtk::ScrolledWindow::new();
    es.set_min_content_height(160);
    es.set_max_content_height(320);
    es.set_propagate_natural_height(true);
    es.set_child(Some(&editor_body));
    editor.append(&es);
    let eb = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    eb.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::with_label("Save");
    save.add_css_class("suggested-action");
    eb.append(&cancel);
    eb.append(&save);
    editor.append(&eb);
    stack.add_named(&editor, Some("editor"));

    // Asking for a target (which note).
    let target_page = gtk::Box::new(gtk::Orientation::Vertical, 8);
    target_page.add_css_class("cb-page");
    let target_title = label("", "cb-page-title");
    target_title.set_wrap(true);
    target_page.append(&target_title);
    let target = gtk::Entry::new();
    target_page.append(&target);
    stack.add_named(&target_page, Some("target"));
    card.append(&stack);

    let foot = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    foot.add_css_class("cb-foot");
    let hint = label("", "cb-hint");
    let toast = label("", "cb-toast");
    toast.set_visible(false);
    foot.append(&hint);
    foot.append(&toast);
    card.append(&foot);

    let popup = Rc::new(Popup {
        window: window.clone(),
        card: card.clone(),
        search: search.clone(),
        tabs,
        private: private.clone(),
        banner,
        stack: stack.clone(),
        list: list.clone(),
        scroller: scroller.clone(),
        empty,
        hint,
        toast,
        result_title,
        result_text,
        result_buttons: use_buttons.clone(),
        editor_name,
        editor_body,
        target_title,
        target,
        tab: Cell::new(Tab::Recent),
        items: RefCell::default(),
        marked: RefCell::default(),
        details: RefCell::default(),
        offset: Cell::new(0),
        exhausted: Cell::new(false),
        loading: Cell::new(false),
        generation: Cell::new(0),
        only,
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
    for (t, b) in &popup.tabs {
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
    let w = Rc::downgrade(&popup);
    private.connect_clicked(move |b| {
        if let Some(p) = w.upgrade()
            && b.is_active() != p.status.borrow().private
        {
            p.toggle_private();
        }
    });
    let w = Rc::downgrade(&popup);
    list.connect_row_activated(move |_, _| {
        if let Some(p) = w.upgrade() {
            p.activate(false);
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
    let w = Rc::downgrade(&popup);
    back.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.back_to_list();
        }
    });
    let w = Rc::downgrade(&popup);
    copy_b.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.result_use(false);
        }
    });
    let w = Rc::downgrade(&popup);
    paste_b.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.result_use(true);
        }
    });
    let w = Rc::downgrade(&popup);
    cancel.connect_clicked(move |_| {
        if let Some(p) = w.upgrade() {
            p.back_to_list();
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

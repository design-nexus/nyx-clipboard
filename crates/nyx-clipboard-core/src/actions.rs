//! The action registry's types: built-in actions picked by content, and actions other
//! apps add, either at run time over `Actions1.Register` (Nyx apps only) or as a manifest
//! in `~/.local/share/nyx-clipboard/actions/<id>.toml` (used only once approved).

use crate::model::Kind;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Group {
    Paste,
    Organize,
    Transform,
    Convert,
    Send,
    Assistant,
    Apps,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Paste => "Paste",
            Group::Organize => "Organize",
            Group::Transform => "Transform",
            Group::Convert => "Convert",
            Group::Send => "Send to",
            Group::Assistant => "Assistant",
            Group::Apps => "Apps",
        }
    }
}

/// An action as menus show it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Action {
    pub id: String,
    pub label: String,
    pub icon: String,
    pub group: Group,
    /// `builtin`, or the app or manifest that added it.
    pub source: String,
    /// Needs a second click (Delete).
    #[serde(default)]
    pub destructive: bool,
    /// Its result is shown before it replaces anything (Assistant, OCR).
    #[serde(default)]
    pub previews: bool,
}

fn a(id: &str, label: &str, icon: &str, group: Group) -> Action {
    Action { id: id.into(), label: label.into(), icon: icon.into(), group, source: "builtin".into(), destructive: false, previews: false }
}

/// What an entry is, for picking actions.
pub struct Subject<'a> {
    pub kind: Kind,
    pub text: &'a str,
    pub formatted: bool,
    pub pinned: bool,
    pub favorite: bool,
}

/// The built-in actions for an entry, in menu order. Integration actions
/// (Group::Send, Group::Assistant, OCR) are listed here; the daemon drops the ones
/// whose app isn't installed.
pub fn builtin(s: &Subject<'_>) -> Vec<Action> {
    let mut out = vec![a("paste", "Paste", "edit-paste-symbolic", Group::Paste)];
    if s.formatted || matches!(s.kind, Kind::Rich | Kind::Html) {
        out.push(a("paste-plain", "Paste as plain text", "format-text-plaintext-symbolic", Group::Paste));
    }
    out.push(a("copy", "Copy", "edit-copy-symbolic", Group::Paste));
    out.push(if s.pinned {
        a("unpin", "Unpin", "view-pin-symbolic", Group::Organize)
    } else {
        a("pin", "Pin", "view-pin-symbolic", Group::Organize)
    });
    out.push(if s.favorite {
        a("unfavorite", "Remove from favorites", "starred-symbolic", Group::Organize)
    } else {
        a("favorite", "Add to favorites", "non-starred-symbolic", Group::Organize)
    });
    if s.kind.has_text() {
        out.push(a("snippet", "Save as a snippet", "insert-text-symbolic", Group::Organize));
    }
    let mut del = a("delete", "Delete", "user-trash-symbolic", Group::Organize);
    del.destructive = true;
    out.push(del);

    if s.kind == Kind::Files {
        out.push(a("copy-path", "Copy as path", "edit-copy-symbolic", Group::Paste));
        out.push(a("copy-uri", "Copy as file link", "insert-link-symbolic", Group::Paste));
        out.push(a("reveal", "Show in Files", "folder-open-symbolic", Group::Send));
    }
    if s.kind.has_text() && s.kind != Kind::Files {
        let text_actions = [
            ("case-upper", "UPPERCASE"),
            ("case-lower", "lowercase"),
            ("case-title", "Title Case"),
            ("case-sentence", "Sentence case"),
            ("case-snake", "snake_case"),
            ("case-kebab", "kebab-case"),
            ("case-camel", "camelCase"),
        ];
        for (id, label) in text_actions {
            out.push(a(id, label, "font-size-symbolic", Group::Transform));
        }
        out.push(a("trim", "Trim spaces", "format-justify-left-symbolic", Group::Transform));
        out.push(a("join-lines", "Join lines", "format-justify-fill-symbolic", Group::Transform));
        if s.text.contains("http") && crate::transform::clean_urls_in(s.text) != s.text {
            out.push(a("clean-url", "Remove tracking from links", "web-browser-symbolic", Group::Transform));
        }
    }
    if s.formatted || matches!(s.kind, Kind::Rich | Kind::Html) {
        out.push(a("remove-formatting", "Remove formatting", "format-text-plaintext-symbolic", Group::Transform));
        out.push(a("html-to-markdown", "Convert to Markdown", "text-x-generic-symbolic", Group::Convert));
    }
    if s.kind == Kind::Html {
        out.push(a("html-to-text", "Convert to plain text", "text-x-generic-symbolic", Group::Convert));
    }
    if s.kind == Kind::Markdown {
        out.push(a("markdown-to-html", "Convert to HTML", "format-text-rich-symbolic", Group::Convert));
    }
    if s.kind.has_text() && crate::transform::json_pretty(s.text).is_some() {
        out.push(a("json-pretty", "Format JSON", "applications-engineering-symbolic", Group::Convert));
        out.push(a("json-compact", "Compact JSON", "applications-engineering-symbolic", Group::Convert));
    }
    if s.kind.has_text() && s.text.len() < 64 * 1024 && s.kind != Kind::Files {
        out.push(a("url-encode", "URL-encode", "insert-link-symbolic", Group::Convert));
        if s.text.contains('%') {
            out.push(a("url-decode", "URL-decode", "insert-link-symbolic", Group::Convert));
        }
    }
    if s.kind == Kind::Image {
        let mut ocr = a("ocr", "Extract text (Capture)", "scanner-symbolic", Group::Convert);
        ocr.previews = true;
        out.push(ocr);
        out.push(a("save-image", "Save image…", "document-save-symbolic", Group::Send));
    }
    if s.kind.has_text() && s.kind != Kind::Files {
        out.push(a("save-file", "Save as a file…", "document-save-symbolic", Group::Send));
        out.push(a("note-new", "New note", "accessories-text-editor-symbolic", Group::Send));
        out.push(a("note-append", "Add to a note…", "accessories-text-editor-symbolic", Group::Send));
    }
    out.push(a("drive", "Save to Drive", "folder-remote-symbolic", Group::Send));
    out.push(a("share", "Share…", "send-to-symbolic", Group::Send));
    if s.kind.has_text() && s.kind != Kind::Files {
        let ai = [
            ("ai-summarize", "Summarize", "chat-bubble-text-symbolic"),
            ("ai-rewrite", "Rewrite", "document-edit-symbolic"),
            ("ai-translate", "Translate", "translations-symbolic"),
            ("ai-extract", "Pull out the details", "edit-find-symbolic"),
        ];
        for (id, label, icon) in ai {
            let mut x = a(id, label, icon, Group::Assistant);
            x.previews = true;
            out.push(x);
        }
        if s.kind == Kind::Code {
            let mut x = a("ai-explain", "Explain code", "dialog-information-symbolic", Group::Assistant);
            x.previews = true;
            out.push(x);
        }
    }
    out
}

/// What happens to an external action's output.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Output {
    /// Put it on the clipboard (and in the history).
    Copy,
    /// Show it in the popup, with Copy and Replace.
    #[default]
    Preview,
    /// Nothing; the action does its own thing.
    None,
}

/// A Nyx app's Control tool to call with `{"text": …}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DbusTarget {
    pub bus: String,
    pub path: String,
    pub tool: String,
}

/// An action another app adds.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Manifest {
    /// Reverse-DNS, unique (`com.example.upload`).
    pub id: String,
    pub label: String,
    #[serde(default = "default_icon")]
    pub icon: String,
    /// The kinds it applies to; empty for every kind with text.
    #[serde(default)]
    pub kinds: Vec<Kind>,
    /// A program to run with the text on stdin; its stdout is the result.
    #[serde(default)]
    pub exec: Vec<String>,
    #[serde(default)]
    pub dbus: Option<DbusTarget>,
    #[serde(default)]
    pub output: Output,
}

fn default_icon() -> String {
    "application-x-addon-symbolic".into()
}

impl Manifest {
    pub fn validate(&self) -> Result<(), String> {
        let id_ok = self.id.len() <= 96
            && self.id.contains('.')
            && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_');
        if !id_ok {
            return Err("an action id is reverse-DNS: letters, digits, '.', '-', '_'".into());
        }
        if self.label.trim().is_empty() || self.label.len() > 80 {
            return Err("an action needs a short label".into());
        }
        match (self.exec.is_empty(), &self.dbus) {
            (false, None) => {
                if !self.exec[0].starts_with('/') {
                    return Err("exec must start with an absolute path".into());
                }
            }
            (true, Some(d)) => {
                if !d.bus.starts_with("io.github.design_nexus.nyx.") {
                    return Err("D-Bus actions can only call Nyx apps".into());
                }
            }
            _ => return Err("an action has either exec or dbus".into()),
        }
        Ok(())
    }

    pub fn applies_to(&self, kind: Kind) -> bool {
        if self.kinds.is_empty() { kind.has_text() } else { self.kinds.contains(&kind) }
    }

    pub fn action(&self, source: &str) -> Action {
        Action {
            id: format!("ext:{}", self.id),
            label: self.label.clone(),
            icon: self.icon.clone(),
            group: Group::Apps,
            source: source.to_string(),
            destructive: false,
            previews: self.output == Output::Preview,
        }
    }
}

/// What running an action did, for the popup.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Outcome {
    pub ok: bool,
    /// One line for a toast.
    pub message: String,
    /// A result to preview (Assistant, OCR, external actions).
    pub text: Option<String>,
    /// The result was copied (and the entry it became).
    pub copied: Option<i64>,
    /// The popup should close (paste, reveal, share).
    pub close: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(s: &Subject<'_>) -> Vec<String> {
        builtin(s).into_iter().map(|a| a.id).collect()
    }

    #[test]
    fn picks_by_content() {
        let text = Subject { kind: Kind::Text, text: "hello", formatted: false, pinned: false, favorite: false };
        let t = ids(&text);
        assert!(t.contains(&"case-upper".to_string()) && t.contains(&"pin".to_string()));
        assert!(!t.contains(&"paste-plain".to_string()) && !t.contains(&"ocr".to_string()));
        let img = Subject { kind: Kind::Image, text: "", formatted: false, pinned: true, favorite: false };
        let i = ids(&img);
        assert!(i.contains(&"ocr".to_string()) && i.contains(&"unpin".to_string()) && !i.contains(&"case-upper".to_string()));
        let link = Subject { kind: Kind::Url, text: "https://x.y/?utm_source=a", formatted: true, pinned: false, favorite: false };
        let l = ids(&link);
        assert!(l.contains(&"clean-url".to_string()) && l.contains(&"paste-plain".to_string()));
        let json = Subject { kind: Kind::Code, text: "{\"a\": 1}", formatted: false, pinned: false, favorite: false };
        assert!(ids(&json).contains(&"json-pretty".to_string()) && ids(&json).contains(&"ai-explain".to_string()));
    }

    #[test]
    fn manifests() {
        let m: Manifest = toml::from_str("id = \"com.example.up\"\nlabel = \"Upload\"\nexec = [\"/usr/bin/up\"]\noutput = \"copy\"").unwrap();
        assert!(m.validate().is_ok());
        assert!(m.applies_to(Kind::Text) && !m.applies_to(Kind::Image));
        let bad: Manifest = toml::from_str("id = \"up\"\nlabel = \"Upload\"\nexec = [\"up\"]").unwrap();
        assert!(bad.validate().is_err());
        let other: Manifest =
            toml::from_str("id = \"a.b\"\nlabel = \"X\"\n[dbus]\nbus = \"org.evil\"\npath = \"/\"\ntool = \"x\"").unwrap();
        assert!(other.validate().is_err());
    }
}

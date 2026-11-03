//! What a copy is, what was kept of it, and what callers see.

use serde::{Deserialize, Serialize};

/// What a copy is, for filters, icons and which actions apply.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    #[default]
    Text,
    /// Formatted text: HTML or RTF alongside plain text.
    Rich,
    Url,
    Code,
    Markdown,
    Html,
    Image,
    Files,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Rich => "rich",
            Kind::Url => "url",
            Kind::Code => "code",
            Kind::Markdown => "markdown",
            Kind::Html => "html",
            Kind::Image => "image",
            Kind::Files => "files",
        }
    }

    pub fn parse(s: &str) -> Option<Kind> {
        Some(match s {
            "text" => Kind::Text,
            "rich" => Kind::Rich,
            "url" => Kind::Url,
            "code" => Kind::Code,
            "markdown" => Kind::Markdown,
            "html" => Kind::Html,
            "image" => Kind::Image,
            "files" => Kind::Files,
            _ => return None,
        })
    }

    /// Plain text can be pasted from it.
    pub fn has_text(self) -> bool {
        !matches!(self, Kind::Image)
    }

    /// A symbolic icon name.
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Text => "text-x-generic-symbolic",
            Kind::Rich | Kind::Html => "format-text-rich-symbolic",
            Kind::Url => "web-browser-symbolic",
            Kind::Code => "applications-engineering-symbolic",
            Kind::Markdown => "text-x-generic-symbolic",
            Kind::Image => "image-x-generic-symbolic",
            Kind::Files => "folder-documents-symbolic",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Text => "Text",
            Kind::Rich => "Formatted text",
            Kind::Url => "Link",
            Kind::Code => "Code",
            Kind::Markdown => "Markdown",
            Kind::Html => "HTML",
            Kind::Image => "Image",
            Kind::Files => "Files",
        }
    }
}

/// One representation of a copy: a MIME type and its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rep {
    pub mime: String,
    pub data: Vec<u8>,
}

/// A copy, classified and ready to store.
#[derive(Clone, Debug)]
pub struct Captured {
    pub kind: Kind,
    /// The plain text (for text kinds, files as paths); empty for images.
    pub text: String,
    /// What is kept, best first.
    pub reps: Vec<Rep>,
    /// Identifies the content, so the same copy again is recognized.
    pub hash: String,
    /// A short single-line description for the list.
    pub preview: String,
    /// Width and height, for images whose header says.
    pub dims: Option<(u32, u32)>,
}

impl Captured {
    pub fn size(&self) -> usize {
        self.reps.iter().map(|r| r.data.len()).sum()
    }
}

/// A history entry as lists show it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Entry {
    pub id: i64,
    pub kind: Kind,
    pub preview: String,
    /// Bytes kept, over every representation.
    pub size: u64,
    /// Characters of text (0 for images).
    pub chars: u64,
    pub mimes: Vec<String>,
    /// The app that was focused when it was copied, when known.
    #[serde(default)]
    pub app: Option<String>,
    pub created: f64,
    pub last_used: f64,
    pub uses: u32,
    pub pinned: bool,
    pub favorite: bool,
    /// For images: the stored file, which the popup previews lazily.
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub dims: Option<(u32, u32)>,
}

/// An entry with its full text, for previews and actions.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Detail {
    pub entry: Entry,
    /// The plain text, cut at [`DETAIL_TEXT`] bytes.
    pub text: String,
    pub truncated: bool,
    /// The formatted version, when there is one (also cut).
    #[serde(default)]
    pub html: Option<String>,
    /// For files: the paths.
    #[serde(default)]
    pub files: Vec<String>,
}

pub const DETAIL_TEXT: usize = 256 * 1024;

/// An item that isn't stored: a probable secret, kept for a moment in memory so it can
/// still be pasted from the popup.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Transient {
    pub id: String,
    /// What it looked like ("Access token", "Private key"), never the value.
    pub label: String,
    pub created: f64,
    pub expires: f64,
}

/// A verification code as the popup sees it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct CodeInfo {
    pub id: String,
    pub code: String,
    /// The app that sent the notification.
    pub app: String,
    /// The websites it seems to be for.
    pub sites: Vec<String>,
    pub received: f64,
    pub expires: f64,
}

/// Daemon state for the popup and Settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Status {
    pub entries: u64,
    pub pinned: u64,
    pub snippets: u64,
    pub bytes: u64,
    pub private: bool,
    /// The clipboard is being watched (data-control works here).
    pub watching: bool,
    /// Why it isn't, when it isn't.
    #[serde(default)]
    pub problem: Option<String>,
    pub version: String,
}

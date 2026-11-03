//! What a search asks for: text, a view, a kind (typed as a prefix like `img:`), paging.

use crate::model::Kind;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum View {
    #[default]
    Recent,
    Pinned,
    Favorites,
}

/// `Clipboard1.Search` takes this as JSON; every field is optional.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Search {
    /// What was typed, prefixes and all (`img: cat`).
    pub text: String,
    pub view: View,
    pub kind: Option<Kind>,
    pub offset: u32,
    /// At most 200; 0 means 50.
    pub limit: u32,
}

impl Search {
    pub fn limit(&self) -> u32 {
        match self.limit {
            0 => 50,
            n => n.min(200),
        }
    }
}

/// A query with its prefixes taken out.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Parsed {
    pub text: String,
    pub kind: Option<Kind>,
    pub favorites: bool,
    pub pinned: bool,
}

/// `img: cat` → kind Image, text `cat`. Prefixes: `img:`/`image:`, `url:`/`link:`,
/// `file:`/`files:`, `code:`, `text:`, `rich:`/`html:`, `md:`, `fav:`/`star:`, `pin:`.
pub fn parse(input: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut rest = input.trim_start();
    while let Some((head, tail)) = rest.split_once(':') {
        if head.contains(char::is_whitespace) || head.is_empty() {
            break;
        }
        let matched = match head.to_ascii_lowercase().as_str() {
            "img" | "image" | "images" => {
                p.kind = Some(Kind::Image);
                true
            }
            "url" | "link" | "links" => {
                p.kind = Some(Kind::Url);
                true
            }
            "file" | "files" | "path" => {
                p.kind = Some(Kind::Files);
                true
            }
            "code" => {
                p.kind = Some(Kind::Code);
                true
            }
            "text" => {
                p.kind = Some(Kind::Text);
                true
            }
            "rich" => {
                p.kind = Some(Kind::Rich);
                true
            }
            "html" => {
                p.kind = Some(Kind::Html);
                true
            }
            "md" | "markdown" => {
                p.kind = Some(Kind::Markdown);
                true
            }
            "fav" | "star" | "starred" => {
                p.favorites = true;
                true
            }
            "pin" | "pinned" => {
                p.pinned = true;
                true
            }
            _ => false,
        };
        if !matched {
            break;
        }
        rest = tail.trim_start();
    }
    p.text = rest.trim().to_lowercase();
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes() {
        assert_eq!(parse("img: cat"), Parsed { text: "cat".into(), kind: Some(Kind::Image), ..Default::default() });
        assert_eq!(parse("fav: url: GitHub"), Parsed { text: "github".into(), kind: Some(Kind::Url), favorites: true, pinned: false });
        assert_eq!(parse("https://x.y").text, "https://x.y");
        assert_eq!(parse("note: hello").text, "note: hello");
        assert_eq!(Search::default().limit(), 50);
    }
}

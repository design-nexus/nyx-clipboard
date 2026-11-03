//! Omarchy's own clipboard history (`~/.local/state/omarchy/clipboard-history.json`),
//! read once so the history doesn't start empty. Omarchy keeps its file and watchers.

use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Text(String),
    Image { path: PathBuf, mime: String },
}

/// Newest first, as Omarchy keeps them. Items Omarchy itself would skip are skipped;
/// a missing or unreadable file gives nothing.
pub fn read(path: &Path) -> Vec<Item> {
    let Ok(text) = std::fs::read_to_string(path) else { return Vec::new() };
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    items
        .into_iter()
        .filter_map(|v| {
            if let Value::String(t) = &v {
                return (!t.trim().is_empty()).then(|| Item::Text(t.clone()));
            }
            let kind = v.get("type").or_else(|| v.get("kind")).and_then(Value::as_str).unwrap_or("");
            match kind {
                "text" => {
                    let t = v.get("text").and_then(Value::as_str).unwrap_or("");
                    (!t.trim().is_empty()).then(|| Item::Text(t.to_string()))
                }
                "image" => {
                    let p = v.get("path").and_then(Value::as_str).unwrap_or("");
                    let mime = v.get("mime").and_then(Value::as_str).filter(|m| !m.is_empty()).unwrap_or("image/png");
                    (!p.is_empty()).then(|| Item::Image { path: PathBuf::from(p), mime: mime.to_string() })
                }
                _ => None,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_both_shapes() {
        let dir = std::env::temp_dir().join(format!("nyx-clipboard-omarchy-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("h.json");
        std::fs::write(
            &f,
            r#"["plain", {"type":"text","text":"hello"}, {"type":"image","path":"/tmp/a.png"}, {"type":"text","text":"  "}, 5]"#,
        )
        .unwrap();
        assert_eq!(
            read(&f),
            vec![
                Item::Text("plain".into()),
                Item::Text("hello".into()),
                Item::Image { path: "/tmp/a.png".into(), mime: "image/png".into() }
            ]
        );
        std::fs::write(&f, "not json").unwrap();
        assert!(read(&f).is_empty());
        assert!(read(&dir.join("missing.json")).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}

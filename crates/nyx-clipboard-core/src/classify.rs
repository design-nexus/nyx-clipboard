//! Which of the offered representations are read and kept, and what kind of copy it is.

use crate::model::{Captured, Kind, Rep};

/// Where the plain text of every entry is kept.
pub const TEXT: &str = "text/plain;charset=utf-8";
pub const HTML: &str = "text/html";
pub const URI_LIST: &str = "text/uri-list";
pub const GNOME_FILES: &str = "x-special/gnome-copied-files";
pub const PASSWORD_HINT: &str = "x-kde-passwordManagerHint";

/// Text types in the order they're preferred.
const TEXT_TYPES: &[&str] = &[TEXT, "UTF8_STRING", "text/plain", "STRING", "TEXT"];
const IMAGE_TYPES: &[&str] = &["image/png", "image/jpeg", "image/webp", "image/gif", "image/bmp", "image/tiff"];

/// The representations to read from an offer, as `(offered type, stored type)`, best
/// first. At most one text and one image are read.
pub fn plan(offered: &[String]) -> Vec<(String, String)> {
    let has = |m: &str| offered.iter().any(|o| o == m);
    let mut out = Vec::new();
    if let Some(t) = TEXT_TYPES.iter().find(|t| has(t)) {
        out.push((t.to_string(), TEXT.to_string()));
    } else if let Some(t) = offered.iter().find(|o| o.starts_with("text/plain")) {
        out.push((t.clone(), TEXT.to_string()));
    }
    for m in [HTML, URI_LIST, GNOME_FILES] {
        if has(m) {
            out.push((m.to_string(), m.to_string()));
        }
    }
    let image = IMAGE_TYPES
        .iter()
        .find(|t| has(t))
        .map(|t| t.to_string())
        .or_else(|| offered.iter().find(|o| o.starts_with("image/") && *o != "image/svg+xml").cloned());
    if let Some(i) = image {
        out.push((i.clone(), i));
    }
    out
}

/// The copy carries a password manager's "don't keep this" hint.
pub fn has_password_hint(offered: &[String]) -> bool {
    offered.iter().any(|o| o == PASSWORD_HINT)
}

/// Classify what was read. `None` when there's nothing worth keeping (empty text).
pub fn classify(reps: Vec<Rep>) -> Option<Captured> {
    let mut reps: Vec<Rep> = reps.into_iter().filter(|r| !r.data.is_empty()).collect();
    let text = reps.iter().find(|r| r.mime == TEXT).map(|r| decode_text(&r.data)).unwrap_or_default();
    if let Some(r) = reps.iter_mut().find(|r| r.mime == TEXT) {
        r.data = text.as_bytes().to_vec();
    }
    let image = reps.iter().find(|r| r.mime.starts_with("image/"));
    if let Some(img) = image {
        let dims = image_dims(&img.data);
        let format = img.mime.trim_start_matches("image/").to_uppercase();
        let preview = match dims {
            Some((w, h)) => format!("{format} image · {w} × {h}"),
            None => format!("{format} image"),
        };
        let hash = digest(&img.data);
        // An image keeps its own bytes first; text beside it (a browser's alt text or
        // markup) stays as a fallback.
        reps.sort_by_key(|r| !r.mime.starts_with("image/"));
        return Some(Captured { kind: Kind::Image, text, reps, hash, preview, dims });
    }
    let files = reps.iter().find(|r| r.mime == URI_LIST).map(|r| file_paths(&String::from_utf8_lossy(&r.data))).unwrap_or_default();
    if !files.is_empty() {
        let text = files.join("\n");
        let preview = files_preview(&files);
        if !reps.iter().any(|r| r.mime == TEXT) {
            reps.push(Rep { mime: TEXT.into(), data: text.as_bytes().to_vec() });
        }
        return Some(Captured { kind: Kind::Files, hash: digest(text.as_bytes()), text, reps, preview, dims: None });
    }
    if text.trim().is_empty() {
        return None;
    }
    let has_html = reps.iter().any(|r| r.mime == HTML);
    let kind = text_kind(&text, has_html);
    let preview = preview(&text);
    Some(Captured { kind, hash: digest(text.as_bytes()), text, reps, preview, dims: None })
}

/// The kind of a piece of text; `formatted` when it came with an HTML version.
pub fn text_kind(text: &str, formatted: bool) -> Kind {
    let t = text.trim();
    if is_url(t) {
        return Kind::Url;
    }
    if is_path(t) {
        return Kind::Files;
    }
    if formatted {
        return Kind::Rich;
    }
    if looks_like_html(t) {
        return Kind::Html;
    }
    if looks_like_markdown(t) {
        return Kind::Markdown;
    }
    if looks_like_code(t) {
        return Kind::Code;
    }
    Kind::Text
}

pub fn digest(data: &[u8]) -> String {
    blake3::hash(data).to_hex().to_string()
}

/// UTF-8 (or UTF-16 with a byte order mark), without a trailing NUL.
pub fn decode_text(data: &[u8]) -> String {
    if data.len() >= 2 {
        let le = data.starts_with(&[0xFF, 0xFE]);
        let be = data.starts_with(&[0xFE, 0xFF]);
        if le || be {
            let units: Vec<u16> = data[2..]
                .as_chunks::<2>().0.iter()
                .map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) })
                .collect();
            let s = String::from_utf16_lossy(&units);
            return s.trim_end_matches('\0').to_string();
        }
    }
    let data = data.strip_suffix(&[0]).unwrap_or(data);
    String::from_utf8_lossy(data).into_owned()
}

/// The first line that has something on it, on one line, at most 200 characters.
pub fn preview(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
    let first = lines.next().unwrap_or("");
    let mut p: String = first.split_whitespace().collect::<Vec<_>>().join(" ");
    let more = lines.next().is_some();
    if p.chars().count() > 200 {
        p = p.chars().take(199).collect::<String>() + "…";
    } else if more {
        p.push_str(" …");
    }
    p
}

fn files_preview(files: &[String]) -> String {
    let name = |p: &str| p.rsplit('/').find(|s| !s.is_empty()).unwrap_or(p).to_string();
    match files.len() {
        1 => files[0].clone(),
        n => {
            let names: Vec<String> = files.iter().take(3).map(|f| name(f)).collect();
            let rest = if n > 3 { ", …" } else { "" };
            format!("{n} files: {}{rest}", names.join(", "))
        }
    }
}

/// Local paths from a `text/uri-list` (only `file://` URIs; comments skipped).
pub fn file_paths(list: &str) -> Vec<String> {
    list.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|rest| {
            // `file://host/path`: the host is empty or localhost here.
            let path = rest.strip_prefix("localhost").unwrap_or(rest);
            percent_decode(path)
        })
        .filter(|p| p.starts_with('/'))
        .collect()
}

/// `text/uri-list` for local paths.
pub fn uri_list(paths: &[String]) -> String {
    paths.iter().map(|p| format!("file://{}\r\n", percent_encode_path(p))).collect()
}

pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(h), Some(l)) = ((bytes[i + 1] as char).to_digit(16), (bytes[i + 2] as char).to_digit(16))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode_path(p: &str) -> String {
    let mut out = String::new();
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn is_url(t: &str) -> bool {
    if t.is_empty() || t.len() > 8192 || t.chars().any(char::is_whitespace) {
        return false;
    }
    let lower = t.to_ascii_lowercase();
    ["http://", "https://", "ftp://", "mailto:"].iter().any(|p| lower.starts_with(p) && lower.len() > p.len() + 2)
        || (lower.starts_with("www.") && lower[4..].contains('.'))
}

fn is_path(t: &str) -> bool {
    !t.contains('\n') && t.len() < 4096 && (t.starts_with('/') || t.starts_with("~/")) && t.len() > 1 && !t.contains("//")
}

fn looks_like_html(t: &str) -> bool {
    let lower = t.to_ascii_lowercase();
    (lower.starts_with("<!doctype html") || lower.starts_with("<html") || lower.starts_with('<'))
        && ["</div>", "</p>", "</span>", "</a>", "</html>", "</body>", "</li>", "</table>", "<br"].iter().any(|tag| lower.contains(tag))
}

fn looks_like_markdown(t: &str) -> bool {
    let mut signals = 0;
    for line in t.lines().take(200) {
        let l = line.trim_start();
        if ["# ", "## ", "### ", "```", "- [ ]", "- [x]", "> "].iter().any(|p| l.starts_with(p)) {
            signals += 2;
        } else if l.starts_with("- ") || l.starts_with("* ") || l.starts_with("1. ") {
            signals += 1;
        }
        if l.contains("](") && l.contains('[') {
            signals += 1;
        }
        if l.matches("**").count() >= 2 || l.matches('`').count() >= 2 {
            signals += 1;
        }
    }
    signals >= 3
}

fn looks_like_code(t: &str) -> bool {
    const STARTS: &[&str] = &[
        "fn ", "pub ", "let ", "const ", "use ", "impl ", "struct ", "enum ", "mod ", "def ", "class ", "import ", "from ",
        "#include", "function ", "return ", "var ", "package ", "func ", "if (", "for (", "while (", "} else", "#!/", "SELECT ",
        "select ", "export ", "async ", "await ", "local ", "#[", "@", "$ ", "elif ", "try:", "except",
    ];
    let lines: Vec<&str> = t.lines().filter(|l| !l.trim().is_empty()).take(400).collect();
    if lines.is_empty() {
        return false;
    }
    let mut hits = 0usize;
    for line in &lines {
        let l = line.trim();
        let indented = line.starts_with("    ") || line.starts_with('\t');
        let ends = l.ends_with(';') || l.ends_with('{') || l.ends_with('}') || l.ends_with("):") || l.ends_with("=>");
        let starts = STARTS.iter().any(|s| l.starts_with(s));
        let ops = l.contains("::") || l.contains("->") || l.contains("=>") || l.contains("!=") || l.contains("==") || l.contains("&&");
        if ends || starts || (indented && ops) || (ops && l.contains('(')) {
            hits += 1;
        }
    }
    if lines.len() == 1 {
        let l = lines[0].trim();
        return hits == 1 && (l.ends_with(';') || l.contains("::") || l.contains("=>") || l.starts_with("fn ") || l.starts_with("def "));
    }
    hits * 10 >= lines.len() * 4
}

/// Width and height from a PNG, GIF or JPEG header.
pub fn image_dims(d: &[u8]) -> Option<(u32, u32)> {
    if d.len() >= 24 && d.starts_with(b"\x89PNG\r\n\x1a\n") {
        let w = u32::from_be_bytes(d[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(d[20..24].try_into().ok()?);
        return Some((w, h));
    }
    if d.len() >= 10 && (d.starts_with(b"GIF87a") || d.starts_with(b"GIF89a")) {
        return Some((u16::from_le_bytes([d[6], d[7]]) as u32, u16::from_le_bytes([d[8], d[9]]) as u32));
    }
    if d.len() > 4 && d[0] == 0xFF && d[1] == 0xD8 {
        let mut i = 2;
        while i + 9 < d.len() {
            if d[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = d[i + 1];
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
                let h = u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32;
                let w = u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32;
                return Some((w, h));
            }
            let len = u16::from_be_bytes([d[i + 2], d[i + 3]]) as usize;
            i += 2 + len;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> Vec<Rep> {
        vec![Rep { mime: TEXT.into(), data: t.as_bytes().to_vec() }]
    }

    #[test]
    fn plans_one_text_and_one_image() {
        let offered: Vec<String> =
            ["TARGETS", "text/plain", "UTF8_STRING", "text/html", "image/png", "image/jpeg", "x-kde-passwordManagerHint"]
                .iter()
                .map(|s| s.to_string())
                .collect();
        let p = plan(&offered);
        assert_eq!(p[0], ("UTF8_STRING".to_string(), TEXT.to_string()));
        assert!(p.iter().any(|(o, _)| o == "text/html"));
        assert_eq!(p.iter().filter(|(o, _)| o.starts_with("image/")).count(), 1);
        assert!(has_password_hint(&offered));
    }

    #[test]
    fn kinds() {
        assert_eq!(classify(text("https://example.com/a?b=c")).unwrap().kind, Kind::Url);
        assert_eq!(classify(text("/home/ken/notes.md")).unwrap().kind, Kind::Files);
        assert_eq!(classify(text("fn main() {\n    println!(\"hi\");\n}\n")).unwrap().kind, Kind::Code);
        assert_eq!(classify(text("# Title\n\n- one\n- two\n\nSee [link](http://x.y)")).unwrap().kind, Kind::Markdown);
        assert_eq!(classify(text("<div><p>Hello</p></div>")).unwrap().kind, Kind::Html);
        assert_eq!(classify(text("Just some words, copied from a page.")).unwrap().kind, Kind::Text);
        assert!(classify(text("   \n ")).is_none());
        let mut rich = text("Hello");
        rich.push(Rep { mime: HTML.into(), data: b"<b>Hello</b>".to_vec() });
        assert_eq!(classify(rich).unwrap().kind, Kind::Rich);
    }

    #[test]
    fn files_and_images() {
        let reps = vec![Rep { mime: URI_LIST.into(), data: b"file:///home/ken/My%20File.txt\r\nfile:///tmp/b.png\r\n".to_vec() }];
        let c = classify(reps).unwrap();
        assert_eq!(c.kind, Kind::Files);
        assert_eq!(c.text, "/home/ken/My File.txt\n/tmp/b.png");
        assert!(c.preview.starts_with("2 files"));
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&640u32.to_be_bytes());
        png.extend_from_slice(&480u32.to_be_bytes());
        let c = classify(vec![Rep { mime: "image/png".into(), data: png }]).unwrap();
        assert_eq!(c.kind, Kind::Image);
        assert_eq!(c.dims, Some((640, 480)));
        assert_eq!(c.preview, "PNG image · 640 × 480");
    }

    #[test]
    fn previews_and_paths() {
        assert_eq!(preview("\n  first   line \nsecond"), "first line …");
        assert_eq!(file_paths("# c\nfile:///a%2Fb\nhttp://x\n"), vec!["/a/b".to_string()]);
        assert_eq!(uri_list(&["/a b".into()]), "file:///a%20b\r\n");
        assert_eq!(decode_text(b"\xFF\xFEh\0i\0"), "hi");
        assert_eq!(decode_text(b"hi\0"), "hi");
    }
}

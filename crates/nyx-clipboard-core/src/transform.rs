//! The built-in text actions: case, whitespace, formatting, links and formats.

/// Text case conversions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Case {
    Upper,
    Lower,
    Title,
    Sentence,
    Snake,
    Kebab,
    Camel,
}

pub fn change_case(text: &str, case: Case) -> String {
    match case {
        Case::Upper => text.to_uppercase(),
        Case::Lower => text.to_lowercase(),
        Case::Title => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for c in text.chars() {
                if c.is_alphanumeric() {
                    if start {
                        out.extend(c.to_uppercase());
                    } else {
                        out.extend(c.to_lowercase());
                    }
                    start = false;
                } else {
                    out.push(c);
                    start = c.is_whitespace() || c == '-' || c == '(' || c == '"';
                }
            }
            out
        }
        Case::Sentence => {
            let mut out = String::with_capacity(text.len());
            let mut start = true;
            for c in text.chars() {
                if start && c.is_alphabetic() {
                    out.extend(c.to_uppercase());
                    start = false;
                } else {
                    out.extend(c.to_lowercase());
                    if c.is_alphanumeric() {
                        start = false;
                    }
                }
                if matches!(c, '.' | '!' | '?' | '\n') {
                    start = true;
                }
            }
            out
        }
        Case::Snake | Case::Kebab | Case::Camel => {
            let words = words(text);
            match case {
                Case::Snake => words.join("_"),
                Case::Kebab => words.join("-"),
                _ => words
                    .iter()
                    .enumerate()
                    .map(|(i, w)| {
                        if i == 0 {
                            w.clone()
                        } else {
                            let mut c = w.chars();
                            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                        }
                    })
                    .collect(),
            }
        }
    }
}

/// Lowercase words, split on anything that isn't a letter or digit and on camelCase.
fn words(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut prev_lower = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if c.is_uppercase() && prev_lower && !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            cur.extend(c.to_lowercase());
            prev_lower = c.is_lowercase() || c.is_ascii_digit();
        } else {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            prev_lower = false;
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// No spaces at line ends, no blank lines at either end, at most one blank line in a row.
pub fn trim_whitespace(text: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    let mut blank = 0;
    for line in text.lines().map(str::trim_end) {
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        out.push(line);
    }
    while out.first().is_some_and(|l| l.is_empty()) {
        out.remove(0);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    let joined = out.join("\n");
    // Leading indentation of the first line is kept unless the whole thing is one line.
    if joined.lines().count() <= 1 { joined.trim().to_string() } else { joined }
}

/// Everything on one line with single spaces.
pub fn join_lines(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Query parameters that only track where a click came from.
const TRACKING: &[&str] = &[
    "fbclid", "gclid", "gclsrc", "dclid", "gbraid", "wbraid", "msclkid", "yclid", "twclid", "ttclid", "li_fat_id", "mc_cid",
    "mc_eid", "igshid", "igsh", "_hsenc", "_hsmi", "__hssc", "__hstc", "__hsfp", "hsCtaTracking", "mkt_tok", "oly_anon_id",
    "oly_enc_id", "rb_clickid", "s_cid", "vero_conv", "vero_id", "wickedid", "_openstat", "ref_src", "ref_url", "si", "spm",
    "scm", "share_source", "cmpid", "srsltid", "ved", "ei", "sca_esv", "feature", "pp", "trk", "trackingId",
];

/// The link without tracking parameters (`utm_*`, `fbclid`, …). Anything that isn't
/// an http(s) link comes back unchanged.
pub fn clean_url(url: &str) -> String {
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        return url.to_string();
    }
    let (base, frag) = match url.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (url, None),
    };
    let Some((path, query)) = base.split_once('?') else { return url.to_string() };
    // `si` and `feature` are only tracking on a few hosts; elsewhere they may matter.
    let host = path.split('/').nth(2).unwrap_or("").to_ascii_lowercase();
    let host_only = |k: &str| match k {
        "si" | "feature" | "pp" => host.ends_with("youtube.com") || host.ends_with("youtu.be") || host.ends_with("spotify.com"),
        "ved" | "ei" | "sca_esv" => host.contains("google."),
        "trk" | "trackingId" => host.ends_with("linkedin.com"),
        _ => true,
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .filter(|p| {
            let key = p.split('=').next().unwrap_or("");
            let lk = key.to_ascii_lowercase();
            !(lk.starts_with("utm_") || lk.starts_with("pk_") && lk != "pk" || TRACKING.contains(&key) && host_only(key))
        })
        .collect();
    let mut out = path.to_string();
    if !kept.is_empty() {
        out.push('?');
        out.push_str(&kept.join("&"));
    }
    if let Some(f) = frag {
        // `#:~:text=` highlights and `#utm` fragments are tracking too.
        if !f.starts_with(":~:") && !f.to_ascii_lowercase().starts_with("utm") {
            out.push('#');
            out.push_str(f);
        }
    }
    out
}

/// Every link in `text` cleaned.
pub fn clean_urls_in(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (i, part) in text.split(' ').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut lines = part.split('\n');
        if let Some(first) = lines.next() {
            out.push_str(&clean_url(first));
        }
        for l in lines {
            out.push('\n');
            out.push_str(&clean_url(l));
        }
    }
    out
}

pub fn json_pretty(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    serde_json::to_string_pretty(&v).ok()
}

pub fn json_compact(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    serde_json::to_string(&v).ok()
}

pub fn url_encode(text: &str) -> String {
    let mut out = String::new();
    for b in text.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn url_decode(text: &str) -> String {
    crate::classify::percent_decode(&text.replace('+', " "))
}

pub fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn decode_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" | "#39" => '\'',
        "nbsp" => ' ',
        "mdash" => '—',
        "ndash" => '–',
        "hellip" => '…',
        "copy" => '©',
        "reg" => '®',
        "laquo" => '«',
        "raquo" => '»',
        "lsquo" => '‘',
        "rsquo" => '’',
        "ldquo" => '“',
        "rdquo" => '”',
        _ => {
            let n = name.strip_prefix('#')?;
            let v = match n.strip_prefix(['x', 'X']) {
                Some(h) => u32::from_str_radix(h, 16).ok()?,
                None => n.parse().ok()?,
            };
            char::from_u32(v)?
        }
    })
}

pub fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        match after.find(';').filter(|&j| j <= 10).and_then(|j| decode_entity(&after[..j]).map(|c| (j, c))) {
            Some((j, c)) => {
                out.push(c);
                rest = &after[j + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

enum Token<'a> {
    Text(&'a str),
    /// Name (lowercase), closing, attributes.
    Tag(String, bool, &'a str),
}

fn tokens(html: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let mut rest = html;
    while !rest.is_empty() {
        match rest.find('<') {
            Some(0) => {
                if let Some(stripped) = rest.strip_prefix("<!--") {
                    rest = stripped.find("-->").map(|j| &stripped[j + 3..]).unwrap_or("");
                    continue;
                }
                let Some(end) = rest.find('>') else {
                    out.push(Token::Text(rest));
                    break;
                };
                let inner = &rest[1..end];
                let closing = inner.starts_with('/');
                let body = inner.trim_start_matches('/');
                let name_end = body.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(body.len());
                let name = body[..name_end].to_ascii_lowercase();
                out.push(Token::Tag(name, closing, &body[name_end..]));
                rest = &rest[end + 1..];
            }
            Some(i) => {
                out.push(Token::Text(&rest[..i]));
                rest = &rest[i..];
            }
            None => {
                out.push(Token::Text(rest));
                break;
            }
        }
    }
    out
}

fn attr<'a>(attrs: &'a str, name: &str) -> Option<&'a str> {
    let lower = attrs.to_ascii_lowercase();
    let i = lower.find(&format!("{name}="))?;
    let v = &attrs[i + name.len() + 1..];
    let (q, v) = match v.chars().next()? {
        '"' => ('"', &v[1..]),
        '\'' => ('\'', &v[1..]),
        _ => (' ', v),
    };
    Some(&v[..v.find(q).unwrap_or(v.len())])
}

const BLOCKS: &[&str] = &["p", "div", "section", "article", "header", "footer", "ul", "ol", "table", "blockquote", "pre", "h1", "h2", "h3", "h4", "h5", "h6"];

/// The text of an HTML fragment, with block elements on their own lines.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::new();
    let mut skip = 0;
    for t in tokens(html) {
        match t {
            Token::Tag(name, closing, _) => match name.as_str() {
                "script" | "style" | "head" | "title" => skip = if closing { 0 } else { 1 },
                "br" => out.push('\n'),
                "li" if !closing => out.push_str("\n• "),
                "tr" if closing => out.push('\n'),
                "td" | "th" if closing => out.push('\t'),
                n if BLOCKS.contains(&n) => out.push('\n'),
                _ => {}
            },
            Token::Text(s) if skip == 0 => out.push_str(&decode_entities(&s.replace(['\n', '\r'], " "))),
            Token::Text(_) => {}
        }
    }
    trim_whitespace(&out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n"))
}

/// HTML as Markdown: headings, emphasis, code, links, lists, quotes, paragraphs.
pub fn html_to_markdown(html: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    let mut pre = false;
    let mut links: Vec<String> = Vec::new();
    let mut ordered: Vec<(bool, u32)> = Vec::new();
    for t in tokens(html) {
        match t {
            Token::Tag(name, closing, attrs) => match (name.as_str(), closing) {
                ("script" | "style" | "head" | "title", c) => skip = !c,
                ("br", _) => out.push('\n'),
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                    let n = name[1..].parse::<usize>().unwrap_or(1);
                    out.push_str(&format!("\n\n{} ", "#".repeat(n)));
                }
                ("strong" | "b", _) => out.push_str("**"),
                ("em" | "i", _) => out.push('*'),
                ("code", _) if !pre => out.push('`'),
                ("pre", false) => {
                    pre = true;
                    out.push_str("\n\n```\n");
                }
                ("pre", true) => {
                    pre = false;
                    out.push_str("\n```\n\n");
                }
                ("a", false) => {
                    links.push(attr(attrs, "href").unwrap_or("").to_string());
                    out.push('[');
                }
                ("a", true) => {
                    let href = links.pop().unwrap_or_default();
                    out.push_str(&format!("]({href})"));
                }
                ("ul", false) => ordered.push((false, 0)),
                ("ol", false) => ordered.push((true, 0)),
                ("ul" | "ol", true) => {
                    ordered.pop();
                    out.push('\n');
                }
                ("li", false) => {
                    let depth = ordered.len().saturating_sub(1);
                    let marker = match ordered.last_mut() {
                        Some((true, n)) => {
                            *n += 1;
                            format!("{n}.")
                        }
                        _ => "-".to_string(),
                    };
                    out.push_str(&format!("\n{}{marker} ", "  ".repeat(depth)));
                }
                ("blockquote", false) => out.push_str("\n\n> "),
                ("img", _) => {
                    let alt = attr(attrs, "alt").unwrap_or("");
                    let src = attr(attrs, "src").unwrap_or("");
                    out.push_str(&format!("![{alt}]({src})"));
                }
                (n, _) if BLOCKS.contains(&n) => out.push_str("\n\n"),
                _ => {}
            },
            Token::Text(s) if !skip => {
                if pre {
                    out.push_str(&decode_entities(s));
                } else {
                    if s.starts_with(char::is_whitespace) && !s.trim().is_empty() && !out.ends_with(char::is_whitespace) {
                        out.push(' ');
                    }
                    out.push_str(&decode_entities(&s.split_whitespace().collect::<Vec<_>>().join(" ")).replace("  ", " "));
                    if s.ends_with(char::is_whitespace) && !s.trim().is_empty() {
                        out.push(' ');
                    }
                }
            }
            Token::Text(_) => {}
        }
    }
    trim_whitespace(&out)
}

fn inline_markdown(text: &str) -> String {
    let mut out = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut bold = false;
    let mut em = false;
    while i < chars.len() {
        let c = chars[i];
        if c == '`'
            && let Some(j) = chars[i + 1..].iter().position(|&x| x == '`')
        {
            let code: String = chars[i + 1..i + 1 + j].iter().collect();
            out.push_str(&format!("<code>{}</code>", escape_html(&code)));
            i += j + 2;
            continue;
        }
        if c == '['
            && let Some(close) = chars[i..].iter().position(|&x| x == ']')
            && chars.get(i + close + 1) == Some(&'(')
            && let Some(end) = chars[i + close + 2..].iter().position(|&x| x == ')')
        {
            let label: String = chars[i + 1..i + close].iter().collect();
            let href: String = chars[i + close + 2..i + close + 2 + end].iter().collect();
            out.push_str(&format!("<a href=\"{}\">{}</a>", escape_html(&href), inline_markdown(&label)));
            i += close + 3 + end;
            continue;
        }
        if c == '*' && chars.get(i + 1) == Some(&'*') {
            out.push_str(if bold { "</strong>" } else { "<strong>" });
            bold = !bold;
            i += 2;
            continue;
        }
        if c == '*' || (c == '_' && (i == 0 || !chars[i - 1].is_alphanumeric() || em)) {
            out.push_str(if em { "</em>" } else { "<em>" });
            em = !em;
            i += 1;
            continue;
        }
        out.push_str(&escape_html(&c.to_string()));
        i += 1;
    }
    if bold {
        out.push_str("</strong>");
    }
    if em {
        out.push_str("</em>");
    }
    out
}

/// Markdown as HTML: headings, paragraphs, lists, quotes, code blocks, emphasis, links.
pub fn markdown_to_html(md: &str) -> String {
    let mut out = String::new();
    let mut para: Vec<String> = Vec::new();
    let mut list: Option<&'static str> = None;
    let mut code: Option<String> = None;
    let flush = |out: &mut String, para: &mut Vec<String>| {
        if !para.is_empty() {
            out.push_str(&format!("<p>{}</p>\n", inline_markdown(&para.join(" "))));
            para.clear();
        }
    };
    let end_list = |out: &mut String, list: &mut Option<&'static str>| {
        if let Some(tag) = list.take() {
            out.push_str(&format!("</{tag}>\n"));
        }
    };
    for line in md.lines() {
        if let Some(buf) = code.as_mut() {
            if line.trim_start().starts_with("```") {
                out.push_str(&format!("<pre><code>{}</code></pre>\n", escape_html(buf.trim_end_matches('\n'))));
                code = None;
            } else {
                buf.push_str(line);
                buf.push('\n');
            }
            continue;
        }
        let l = line.trim();
        if l.starts_with("```") {
            flush(&mut out, &mut para);
            end_list(&mut out, &mut list);
            code = Some(String::new());
            continue;
        }
        if l.is_empty() {
            flush(&mut out, &mut para);
            end_list(&mut out, &mut list);
            continue;
        }
        let hashes = l.chars().take_while(|&c| c == '#').count();
        if (1..=6).contains(&hashes) && l[hashes..].starts_with(' ') {
            flush(&mut out, &mut para);
            end_list(&mut out, &mut list);
            out.push_str(&format!("<h{hashes}>{}</h{hashes}>\n", inline_markdown(l[hashes..].trim())));
            continue;
        }
        let bullet = l.strip_prefix("- ").or_else(|| l.strip_prefix("* "));
        let numbered = l.split_once(". ").filter(|(n, _)| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())).map(|(_, r)| r);
        if let Some((item, tag)) = bullet.map(|b| (b, "ul")).or(numbered.map(|n| (n, "ol"))) {
            flush(&mut out, &mut para);
            if list != Some(tag) {
                end_list(&mut out, &mut list);
                out.push_str(&format!("<{tag}>\n"));
                list = Some(tag);
            }
            out.push_str(&format!("<li>{}</li>\n", inline_markdown(item)));
            continue;
        }
        if let Some(q) = l.strip_prefix("> ").or_else(|| l.strip_prefix('>')) {
            flush(&mut out, &mut para);
            end_list(&mut out, &mut list);
            out.push_str(&format!("<blockquote>{}</blockquote>\n", inline_markdown(q)));
            continue;
        }
        end_list(&mut out, &mut list);
        para.push(l.to_string());
    }
    if let Some(buf) = code {
        out.push_str(&format!("<pre><code>{}</code></pre>\n", escape_html(buf.trim_end_matches('\n'))));
    }
    flush(&mut out, &mut para);
    end_list(&mut out, &mut list);
    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cases() {
        assert_eq!(change_case("hello world", Case::Upper), "HELLO WORLD");
        assert_eq!(change_case("hello WORLD again", Case::Title), "Hello World Again");
        assert_eq!(change_case("HELLO. WORLD here", Case::Sentence), "Hello. World here");
        assert_eq!(change_case("Hello World-again", Case::Snake), "hello_world_again");
        assert_eq!(change_case("helloWorld again", Case::Kebab), "hello-world-again");
        assert_eq!(change_case("hello world again", Case::Camel), "helloWorldAgain");
    }

    #[test]
    fn whitespace() {
        assert_eq!(trim_whitespace("\n\n  a  \n\n\n\nb   \n\n"), "  a\n\nb");
        assert_eq!(trim_whitespace("   one line   "), "one line");
        assert_eq!(join_lines("a\n  b\tc"), "a b c");
    }

    #[test]
    fn urls() {
        assert_eq!(clean_url("https://ex.com/a?utm_source=x&id=3&fbclid=abc#top"), "https://ex.com/a?id=3#top");
        assert_eq!(clean_url("https://ex.com/a?utm_source=x"), "https://ex.com/a");
        assert_eq!(clean_url("https://www.youtube.com/watch?v=abc&si=xyz"), "https://www.youtube.com/watch?v=abc");
        assert_eq!(clean_url("https://ex.com/a?si=keep"), "https://ex.com/a?si=keep");
        assert_eq!(clean_url("https://ex.com/a#:~:text=hello"), "https://ex.com/a#:~:text=hello");
        assert_eq!(clean_url("https://ex.com/a?x=1#:~:text=hello"), "https://ex.com/a?x=1");
        assert_eq!(clean_url("mailto:a@b.c?utm_source=x"), "mailto:a@b.c?utm_source=x");
        assert_eq!(clean_urls_in("see https://ex.com/?gclid=1 now"), "see https://ex.com/ now");
    }

    #[test]
    fn formats() {
        assert_eq!(html_to_text("<p>Hello <b>you</b> &amp; me</p><ul><li>one</li><li>two</li></ul><script>x()</script>"), "Hello you & me\n\n• one\n• two");
        assert_eq!(html_to_markdown("<h2>Title</h2><p>A <a href=\"https://x.y\">link</a> and <strong>bold</strong>.</p>"), "## Title\n\nA [link](https://x.y) and **bold**.");
        assert_eq!(markdown_to_html("# Hi\n\nSome *em* and `code`.\n\n- a\n- b"), "<h1>Hi</h1>\n<p>Some <em>em</em> and <code>code</code>.</p>\n<ul>\n<li>a</li>\n<li>b</li>\n</ul>");
        assert_eq!(json_pretty("{\"a\":1}").unwrap(), "{\n  \"a\": 1\n}");
        assert_eq!(json_compact("{ \"a\" : [1, 2] }").unwrap(), "{\"a\":[1,2]}");
        assert!(json_pretty("not json").is_none());
        assert_eq!(url_encode("a b&c"), "a%20b%26c");
        assert_eq!(url_decode("a%20b+c"), "a b c");
        assert_eq!(decode_entities("&lt;&#x41;&#66;&bogus;"), "<AB&bogus;");
    }
}

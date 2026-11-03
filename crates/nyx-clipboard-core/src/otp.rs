//! Verification codes in notifications, found by pattern and context (no model), and
//! matched to the website asking for one.
//!
//! A code counts only next to words that say what it is ("code", "verification",
//! "2FA", …); numbers that look like prices, order numbers, years, times or phone
//! numbers don't. A notification often names the site (a domain, a sender address, the
//! brand): that decides whether a code can go to a page without asking.

use regex::Regex;
use std::sync::LazyLock;

/// A probable verification code and what the notification said about where it's for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub code: String,
    pub score: i32,
    /// Registrable domains named in the notification (`github.com`).
    pub sites: Vec<String>,
    /// Lowercase words of the notification, for brand matching (`github`).
    pub words: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    /// The notification names another site: don't offer the code there.
    None,
    /// Nothing says where it's for: offer it only after the user confirms.
    Low,
    /// The notification names this site.
    High,
}

impl Confidence {
    pub fn as_str(self) -> &'static str {
        match self {
            Confidence::None => "none",
            Confidence::Low => "low",
            Confidence::High => "high",
        }
    }
}

static CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(code|codes|otp|one[\s-]?time|verification|verify|verifying|passcode|pin|2fa|mfa|two[\s-]?(?:factor|step)|security\s+code|login\s+code|sign[\s-]?in\s+code|access\s+code|authentication|authenticate|confirm(?:ation)?|token|código|codice|kod|code\s+de)\b",
    )
    .expect("context pattern")
});

static CANDIDATE: LazyLock<Regex> = LazyLock::new(|| {
    // Digits (4–8, or 3+3 split by a space or dash), a letter-dash prefix (`G-123456`),
    // or 6–8 capitals and digits mixed (`K7P2QX`).
    Regex::new(r"\b(?:[A-Z]-)?(\d{3}[ -]\d{3}|\d{4,8})\b|\b([A-Z0-9]{6,8})\b").expect("candidate pattern")
});

static DOMAIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b((?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,24})\b").expect("domain pattern")
});

const NOT_TLDS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "txt", "pdf", "html", "htm", "md", "rs", "js", "ts", "py", "json", "toml", "zip", "exe", "doc",
    "docx", "xls", "xlsx", "csv", "mp4", "mp3", "svg", "webp", "log", "sh",
];

/// The best verification code in a notification, if there is one.
pub fn detect(summary: &str, body: &str) -> Option<Detection> {
    let text = format!("{summary}\n{body}");
    if text.len() > 16 * 1024 || !CONTEXT.is_match(&text) {
        return None;
    }
    let mut best: Option<(i32, String)> = None;
    for caps in CANDIDATE.captures_iter(&text) {
        let m = caps.get(0).expect("whole match");
        let (raw, digits_only) = match (caps.get(1), caps.get(2)) {
            (Some(d), _) => (d.as_str().to_string(), true),
            (None, Some(a)) => {
                let a = a.as_str();
                let letters = a.chars().any(|c| c.is_ascii_alphabetic());
                let digits = a.chars().any(|c| c.is_ascii_digit());
                if !(letters && digits) {
                    continue;
                }
                (a.to_string(), false)
            }
            _ => continue,
        };
        let code: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
        let score = score(&text, m.start(), m.end(), &code, digits_only);
        if best.as_ref().is_none_or(|(s, _)| score > *s) {
            best = Some((score, code));
        }
    }
    let (score, code) = best?;
    if score < 5 {
        return None;
    }
    Some(Detection { code, score, sites: sites_in(&text), words: words_in(&text) })
}

fn score(text: &str, start: usize, end: usize, code: &str, digits: bool) -> i32 {
    let before = &text[..start];
    let after = &text[end..];
    let near_before: String = before.chars().rev().take(48).collect::<Vec<_>>().into_iter().rev().collect::<String>().to_lowercase();
    let near_after: String = after.chars().take(48).collect::<String>().to_lowercase();
    let mut s = 3; // a context word is somewhere in the notification
    static LEAD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(code|otp|pin|passcode|token|código|codice)\b[^.\n\d]{0,20}$|(is|:|：)\s*$").expect("lead pattern")
    });
    static TRAIL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^\s*(is\s+your|is\s+the|is\s+a|is\s+my|to\s+(verify|confirm|sign|log|continue|complete|finish)|as\s+your)")
            .expect("trail pattern")
    });
    if LEAD.is_match(&near_before) {
        s += 3;
    }
    if TRAIL.is_match(&near_after) {
        s += 2;
    }
    if CONTEXT.is_match(&near_before) || CONTEXT.is_match(&near_after) {
        s += 1;
    }
    match code.len() {
        6 => s += 1,
        4 | 5 | 7 | 8 => {}
        _ => s -= 1,
    }
    // Things that are numbers but not codes.
    let prev = before.chars().next_back();
    let next = after.chars().next();
    if matches!(prev, Some('$' | '€' | '£' | '¥' | '#' | '+' | '/' | '.' | ',')) || matches!(next, Some('%' | '/' | ',' | '.')) && after.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
        s -= 5;
    }
    if matches!(next, Some(':')) && after.chars().nth(1).is_some_and(|c| c.is_ascii_digit()) {
        s -= 5;
    }
    static NOT_CODE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(order|invoice|ref(?:erence)?|account|acct|tracking|phone|tel|call|ticket|case|room|flight|zip|usd|eur|gbp|total|amount|balance|no\.|number)\W{0,3}$").expect("not-code pattern")
    });
    if NOT_CODE.is_match(&near_before) {
        s -= 4;
    }
    if digits && code.len() == 4 && code.parse::<u32>().is_ok_and(|y| (1900..=2099).contains(&y)) && !LEAD.is_match(&near_before) {
        s -= 3;
    }
    s
}

fn sites_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in DOMAIN.find_iter(text) {
        let d = m.as_str().to_ascii_lowercase();
        let tld = d.rsplit('.').next().unwrap_or("");
        if NOT_TLDS.contains(&tld) || d.chars().filter(|c| *c == '.').count() > 6 {
            continue;
        }
        let site = registrable(&d);
        if !out.contains(&site) {
            out.push(site);
        }
        if out.len() >= 8 {
            break;
        }
    }
    out
}

fn words_in(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for w in text.split(|c: char| !c.is_alphanumeric()) {
        if w.len() < 3 || w.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        let w = w.to_lowercase();
        if !out.contains(&w) {
            out.push(w);
        }
        if out.len() >= 96 {
            break;
        }
    }
    out
}

/// Two-label public suffixes common enough to matter.
const SECOND_LEVEL: &[&str] = &[
    "co.uk", "org.uk", "ac.uk", "gov.uk", "me.uk", "com.au", "net.au", "org.au", "co.nz", "co.jp", "ne.jp", "or.jp", "com.br",
    "com.mx", "co.in", "co.za", "com.cn", "com.tw", "com.hk", "co.kr", "com.sg", "com.tr", "com.ar", "co.il",
];

/// The registrable part of a host (`accounts.google.co.uk` → `google.co.uk`).
pub fn registrable(host: &str) -> String {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let labels: Vec<&str> = host.split('.').collect();
    if labels.len() <= 2 {
        return host;
    }
    let last_two = labels[labels.len() - 2..].join(".");
    let keep = if SECOND_LEVEL.contains(&last_two.as_str()) { 3 } else { 2 };
    labels[labels.len().saturating_sub(keep)..].join(".")
}

/// The host of a URL (`https://github.com/login` → `github.com`).
pub fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next()?;
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = host.split(':').next()?.trim().to_ascii_lowercase();
    (!host.is_empty() && host.contains('.')).then_some(host)
}

/// Labels too generic to stand for a brand.
const GENERIC: &[&str] = &["www", "mail", "login", "account", "accounts", "app", "auth", "secure", "id", "my", "web", "online", "signin", "sso"];

/// Whether a code from this notification may go to `origin_host`.
pub fn confidence(sites: &[String], words: &[String], app: &str, origin_host: &str) -> Confidence {
    let site = registrable(origin_host);
    if sites.contains(&site) {
        return Confidence::High;
    }
    let brand = site.split('.').next().unwrap_or("");
    let brand_ok = brand.len() >= 3 && !GENERIC.contains(&brand);
    let named = brand_ok && (words.iter().any(|w| w == brand) || app.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w == brand));
    if named {
        return Confidence::High;
    }
    // A notification that names other sites (and not this one) is for them.
    if !sites.is_empty() {
        return Confidence::None;
    }
    Confidence::Low
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code(summary: &str, body: &str) -> Option<String> {
        detect(summary, body).map(|d| d.code)
    }

    #[test]
    fn finds_codes() {
        assert_eq!(code("GitHub", "Your GitHub launch code is 483920."), Some("483920".into()));
        assert_eq!(code("Google", "G-123456 is your Google verification code."), Some("123456".into()));
        assert_eq!(code("Sign in", "Use code 123 456 to sign in to Example."), Some("123456".into()));
        assert_eq!(code("Verify your email", "Your verification code: K7P2QX"), Some("K7P2QX".into()));
        assert_eq!(code("Login", "Enter 8841 to confirm it's you. This code expires in 10 minutes."), Some("8841".into()));
        assert_eq!(code("Código", "Tu código de verificación es 552210"), Some("552210".into()));
    }

    #[test]
    fn ignores_numbers_that_arent_codes() {
        assert_eq!(code("Order shipped", "Your order #483920 has shipped"), None);
        assert_eq!(code("Reminder", "Meeting at 10:30 in room 4021"), None);
        assert_eq!(code("Payment", "You paid $1250 to Example. Confirmation will follow."), None);
        assert_eq!(code("Chat", "See you in 2026!"), None);
        assert_eq!(code("Weather", "High of 21 today, 18 tomorrow"), None);
        assert_eq!(code("Build", "Build 4471 finished"), None);
    }

    #[test]
    fn sites_and_confidence() {
        let d = detect("noreply@github.com", "Your GitHub launch code is 483920. Visit https://github.com/sessions").unwrap();
        assert_eq!(d.sites, vec!["github.com".to_string()]);
        assert_eq!(confidence(&d.sites, &d.words, "Thunderbird", "github.com"), Confidence::High);
        assert_eq!(confidence(&d.sites, &d.words, "Thunderbird", "evil.example"), Confidence::None);
        let d = detect("Slack", "Your confirmation code is 552-210").unwrap();
        assert_eq!(confidence(&d.sites, &d.words, "Slack", "app.slack.com"), Confidence::High);
        let d = detect("Messages", "Your verification code is 552210").unwrap();
        assert_eq!(confidence(&d.sites, &d.words, "Messages", "bank.example.co.uk"), Confidence::Low);
        assert_eq!(registrable("accounts.google.co.uk"), "google.co.uk");
        assert_eq!(registrable("a.b.github.com"), "github.com");
        assert_eq!(host_of("https://user@Login.Example.com:8443/x?y"), Some("login.example.com".into()));
    }
}

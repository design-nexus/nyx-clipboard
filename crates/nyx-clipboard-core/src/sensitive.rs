//! Copies that look like secrets: private keys, access tokens, passwords. They are never
//! written to the history. The popup can still paste one for a short while from memory.

use regex::Regex;
use std::sync::LazyLock;

/// What a secret looked like, for the "Not saved" row. `None` when it looks ordinary.
pub fn detect(text: &str) -> Option<&'static str> {
    let t = text.trim();
    if t.is_empty() || t.len() > 64 * 1024 {
        return None;
    }
    if t.contains("-----BEGIN") && t.contains("PRIVATE KEY") {
        return Some("Private key");
    }
    if t.starts_with("otpauth://") || t.starts_with("otpauth-migration://") {
        return Some("Authenticator secret");
    }
    // Multi-line text with ordinary words is a document, not a secret.
    if t.lines().count() > 3 {
        return None;
    }
    static TOKENS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"\b(?:ghp|gho|ghu|ghs|ghr)_[A-Za-z0-9]{30,}\b", "GitHub token"),
            (r"\bgithub_pat_[A-Za-z0-9_]{40,}\b", "GitHub token"),
            (r"\bglpat-[A-Za-z0-9_\-]{20,}\b", "GitLab token"),
            (r"\bxox[abposr]-[A-Za-z0-9\-]{10,}\b", "Slack token"),
            (r"\bsk-(?:proj-|ant-)?[A-Za-z0-9_\-]{20,}\b", "API key"),
            (r"\b(?:sk|rk|pk)_(?:live|test)_[A-Za-z0-9]{16,}\b", "API key"),
            (r"\bAKIA[0-9A-Z]{16}\b", "AWS access key"),
            (r"\bAIza[0-9A-Za-z_\-]{35}\b", "Google API key"),
            (r"\bya29\.[0-9A-Za-z_\-]{20,}", "Access token"),
            (r"\bnpm_[A-Za-z0-9]{36}\b", "npm token"),
            (r"\bhf_[A-Za-z0-9]{30,}\b", "Hugging Face token"),
            (r"\beyJ[A-Za-z0-9_\-]{8,}\.eyJ[A-Za-z0-9_\-]{8,}\.[A-Za-z0-9_\-]{8,}\b", "Access token"),
            (r"(?i)\b(?:password|passwd|pwd|secret|api[_-]?key|token)\s*[:=]\s*\S{6,}", "Password"),
            (r"(?i)\bauthorization:\s*(?:bearer|basic)\s+\S{10,}", "Access token"),
        ]
        .into_iter()
        .map(|(p, l)| (Regex::new(p).expect("valid pattern"), l))
        .collect()
    });
    for (re, label) in TOKENS.iter() {
        if re.is_match(t) {
            return Some(label);
        }
    }
    looks_like_password(t).then_some("Password")
}

/// One token with no spaces that mixes character classes and is hard to guess: what a
/// generated password or key looks like, and what ordinary copies (words, paths, URLs,
/// hashes, ids) don't.
fn looks_like_password(t: &str) -> bool {
    let n = t.chars().count();
    if !(12..=128).contains(&n) || t.chars().any(char::is_whitespace) {
        return false;
    }
    if t.contains("://") || t.starts_with('/') || t.starts_with("~/") || t.contains('@') && t.contains('.') {
        return false;
    }
    // Hex (hashes, commit ids, colours) and UUIDs are copied all the time.
    if t.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return false;
    }
    // Identifiers and file names.
    if t.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.') && !mixed_digits_inside(t) {
        return false;
    }
    let lower = t.chars().any(|c| c.is_ascii_lowercase());
    let upper = t.chars().any(|c| c.is_ascii_uppercase());
    let digit = t.chars().any(|c| c.is_ascii_digit());
    let symbol = t.chars().any(|c| c.is_ascii_punctuation());
    let classes = [lower, upper, digit, symbol].iter().filter(|b| **b).count();
    let entropy = shannon(t);
    (classes == 4 && entropy >= 3.0) || (classes >= 3 && n >= 20 && entropy >= 3.8)
}

/// Digits scattered among letters (`aB3xK9`), not just a number at the end (`file_2`).
fn mixed_digits_inside(t: &str) -> bool {
    let chars: Vec<char> = t.chars().collect();
    let switches = chars.windows(2).filter(|w| w[0].is_ascii_digit() != w[1].is_ascii_digit()).count();
    switches >= 4
}

fn shannon(t: &str) -> f64 {
    let mut counts = std::collections::HashMap::new();
    let mut n = 0.0;
    for c in t.chars() {
        *counts.entry(c).or_insert(0.0) += 1.0;
        n += 1.0;
    }
    counts.values().map(|&c: &f64| -(c / n) * (c / n).log2()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets() {
        assert_eq!(detect("ghp_abcdefghijklmnopqrstuvwxyz0123456789"), Some("GitHub token"));
        assert_eq!(detect("AKIAIOSFODNN7EXAMPLE"), Some("AWS access key"));
        assert_eq!(detect("-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----"), Some("Private key"));
        assert_eq!(detect("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U"), Some("Access token"));
        assert_eq!(detect("password: hunter2hunter2"), Some("Password"));
        assert_eq!(detect("Tr0ub4dor&3xQ!"), Some("Password"));
        assert_eq!(detect("otpauth://totp/x?secret=ABC"), Some("Authenticator secret"));
    }

    #[test]
    fn ordinary() {
        for t in [
            "hello world",
            "https://example.com/path?q=1&r=Two3",
            "/home/ken/Projects/nyx-clipboard/Cargo.toml",
            "5c4172947c18876c05d3173d2ebe2a1e434b098b",
            "550e8400-e29b-41d4-a716-446655440000",
            "my_variable_name_2",
            "ken@example.com",
            "io.github.design_nexus.nyx.Clipboard1",
            "483920",
            "README.md",
        ] {
            assert_eq!(detect(t), None, "{t}");
        }
    }
}

//! Snippets: saved text to paste again, with a few placeholders.
//!
//! `{date}` (2026-11-02), `{time}` (22:43), `{datetime}`, `{weekday}`, `{clipboard}`
//! (the newest text in the history) and `{{`/`}}` for literal braces.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default)]
pub struct Snippet {
    pub id: i64,
    pub name: String,
    pub body: String,
    pub created: f64,
    pub updated: f64,
}

/// What placeholders are filled with.
pub struct Context<'a> {
    pub clipboard: &'a str,
    /// Seconds since the epoch.
    pub now: f64,
}

pub fn expand(body: &str, ctx: &Context<'_>) -> String {
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(t) = tail.strip_prefix("{{") {
            out.push('{');
            rest = t;
            continue;
        }
        if let Some(t) = tail.strip_prefix("}}") {
            out.push('}');
            rest = t;
            continue;
        }
        if tail.starts_with('{')
            && let Some(end) = tail.find('}')
        {
            let name = &tail[1..end];
            let value = match name {
                "date" => Some(local(ctx.now, "%Y-%m-%d")),
                "time" => Some(local(ctx.now, "%H:%M")),
                "datetime" => Some(local(ctx.now, "%Y-%m-%d %H:%M")),
                "weekday" => Some(local(ctx.now, "%A")),
                "clipboard" => Some(ctx.clipboard.to_string()),
                _ => None,
            };
            if let Some(v) = value {
                out.push_str(&v);
                rest = &tail[end + 1..];
                continue;
            }
        }
        out.push_str(&tail[..1]);
        rest = &tail[1..];
    }
    out.push_str(rest);
    out
}

/// `t` in local time, formatted with strftime.
pub fn local(t: f64, fmt: &str) -> String {
    let Ok(cfmt) = std::ffi::CString::new(fmt) else { return String::new() };
    // SAFETY: tm is plain data filled by localtime_r; buf has the length given.
    unsafe {
        let secs = t as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&secs, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 128];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), cfmt.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders() {
        let ctx = Context { clipboard: "copied", now: 0.0 };
        assert_eq!(expand("Hi {clipboard}, {{literal}} {unknown}", &ctx), "Hi copied, {literal} {unknown}");
        assert_eq!(expand("{date}", &ctx).len(), 10);
        assert_eq!(expand("no braces", &ctx), "no braces");
        assert_eq!(expand("odd } and {", &ctx), "odd } and {");
    }
}

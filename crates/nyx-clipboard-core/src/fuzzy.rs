//! Fuzzy matching: every character of the query in order, scored so that whole
//! substrings, word starts and runs rank first.

/// A score for `needle` in `hay` (higher is better), or `None` when it doesn't match.
/// Case-insensitive; `needle` should already be lowercase.
pub fn score(needle: &str, hay: &str) -> Option<i32> {
    if needle.is_empty() {
        return Some(0);
    }
    let lower = hay.to_lowercase();
    if let Some(pos) = lower.find(needle) {
        // A substring beats any scattered match; earlier and at a word start is better.
        let at_word = pos == 0 || !lower[..pos].chars().next_back().is_some_and(char::is_alphanumeric);
        return Some(10_000 - (pos.min(2000) as i32) + if at_word { 500 } else { 0 });
    }
    let hay: Vec<char> = lower.chars().take(4096).collect();
    let mut score = 0;
    let mut hi = 0;
    let mut run = 0;
    let mut first = None;
    for nc in needle.chars() {
        if nc.is_whitespace() {
            run = 0;
            continue;
        }
        let mut found = false;
        while hi < hay.len() {
            let hc = hay[hi];
            hi += 1;
            if hc == nc {
                first.get_or_insert(hi);
                let word_start = hi == 1 || !hay[hi - 2].is_alphanumeric();
                run += 1;
                score += 10 + run * 5 + if word_start { 30 } else { 0 };
                found = true;
                break;
            }
            run = 0;
        }
        if !found {
            return None;
        }
    }
    Some(score - (first.unwrap_or(0).min(500) as i32) / 4 - (hi as i32 - first.unwrap_or(0) as i32).min(1000) / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranks() {
        assert!(score("cfg", "Cargo.toml config").is_some());
        assert!(score("xyz", "Cargo.toml").is_none());
        let sub = score("config", "my config file").unwrap();
        let scattered = score("config", "c o n f i g").unwrap();
        assert!(sub > scattered);
        let word = score("git", "github token").unwrap();
        let mid = score("git", "legit").unwrap();
        assert!(word > mid);
        assert_eq!(score("", "anything"), Some(0));
    }
}

//! Rust-side snippet construction. The FTS5 tables of **Data Model §10** are
//! contentless (`content=''`), so SQLite's `snippet()`/`highlight()` can't read
//! column text from them. Instead `storage` resolves a hit's source text (note
//! body / task `notes_md` / transcript text) and this builder windows an excerpt
//! around the first query-term match, wrapping matches in `[`…`]`.
//!
//! This keeps snippet generation honest (no dependence on a capability the
//! contentless index doesn't have) and deterministic (same input → same excerpt,
//! matching the op-log rebuild oracle).

/// The omission marker placed at a truncated snippet boundary (U+2026).
pub const ELLIPSIS: char = '\u{2026}';

/// Build a `[`-marked excerpt of `text` around the first `query` token match.
///
/// - Case-insensitive token match; each occurrence of any query token is wrapped
///   `[like this]`.
/// - The window is centered on the first match and clamped to ~`max_chars`
///   characters, with a leading/trailing `…` when truncated.
/// - With no match (or empty query), returns the leading `max_chars` of `text`.
#[must_use]
pub fn make_snippet(text: &str, query: &str, max_chars: usize) -> String {
    // ASCII case-folding keeps a strict 1:1 char correspondence between `chars`
    // and `lower` (unlike Unicode `to_lowercase`, which can change length and
    // desync the parallel indices). Case-insensitive for ASCII; non-ASCII matches
    // by exact codepoint — adequate for an excerpt highlighter.
    let tokens: Vec<Vec<char>> = query
        .split_whitespace()
        .map(|t| {
            t.trim_matches('"')
                .chars()
                .map(|c| c.to_ascii_lowercase())
                .collect::<Vec<char>>()
        })
        .filter(|t| !t.is_empty())
        .collect();

    let chars: Vec<char> = text.chars().collect();
    let lower: Vec<char> = chars.iter().map(|c| c.to_ascii_lowercase()).collect();

    // Find the first match position (char index), if any.
    let first = tokens
        .iter()
        .filter_map(|tok| find_token(&lower, tok))
        .min();

    let start = match first {
        Some(pos) => pos.saturating_sub(max_chars / 3),
        None => 0,
    };
    let end = (start + max_chars).min(chars.len());

    let mut out = String::new();
    if start > 0 {
        out.push(ELLIPSIS);
    }
    out.push_str(&highlight(&chars[start..end], &lower[start..end], &tokens));
    if end < chars.len() {
        out.push(ELLIPSIS);
    }
    out
}

/// Find the char-index of the first occurrence of `needle` (already ASCII-folded)
/// within `lower` (also folded), as a whole substring.
fn find_token(lower: &[char], needle: &[char]) -> Option<usize> {
    if needle.is_empty() || needle.len() > lower.len() {
        return None;
    }
    (0..=lower.len() - needle.len()).find(|&i| lower[i..i + needle.len()] == needle[..])
}

/// Wrap each token occurrence in the window with `[`…`]`.
fn highlight(window: &[char], lower_window: &[char], tokens: &[Vec<char>]) -> String {
    let mut out = String::new();
    let mut i = 0;
    'outer: while i < window.len() {
        for n in tokens {
            if !n.is_empty() && i + n.len() <= window.len() && lower_window[i..i + n.len()] == n[..]
            {
                out.push('[');
                out.extend(&window[i..i + n.len()]);
                out.push(']');
                i += n.len();
                continue 'outer;
            }
        }
        out.push(window[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn highlights_match_and_windows() {
        let text = "The quarterly planning session covers budget and staffing.";
        let s = make_snippet(text, "planning", 40);
        assert!(s.contains("[planning]"));
    }

    #[test]
    fn case_insensitive_match() {
        let s = make_snippet("Ship the Report today", "report", 40);
        assert!(
            s.contains("[Report]"),
            "preserves original case inside markers: {s}"
        );
    }

    #[test]
    fn no_match_returns_leading_window() {
        let s = make_snippet("alpha beta gamma delta", "zzz", 11);
        assert!(!s.contains('['));
        assert!(s.starts_with("alpha"));
        assert!(s.ends_with(ELLIPSIS));
    }

    #[test]
    fn empty_query_returns_leading_window() {
        let s = make_snippet("hello world", "", 100);
        assert_eq!(s, "hello world");
    }

    #[test]
    fn deterministic() {
        let t = "one two three planning four planning five";
        assert_eq!(
            make_snippet(t, "planning", 30),
            make_snippet(t, "planning", 30)
        );
    }
}

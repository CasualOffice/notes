//! Tiny, dependency-free tokenizer shared by the lexical retrieval channel and the
//! citation-grounding check. Deterministic and Unicode-lowercasing: splits on any
//! non-alphanumeric boundary and folds case, so the same text tokenizes identically
//! on every platform (the op-log correctness oracle expects reproducible output).

/// Lowercase alphanumeric tokens of `s`, in order, splitting on any other char.
#[must_use]
pub(crate) fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for ch in s.chars() {
        if ch.is_alphanumeric() {
            cur.extend(ch.to_lowercase());
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// A leading excerpt of `text`, truncated to at most `max` chars on a char
/// boundary (never mid-codepoint), with an ellipsis when clipped.
#[must_use]
pub(crate) fn snippet(text: &str, max: usize) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= max {
        return trimmed.to_string();
    }
    let head: String = trimmed.chars().take(max).collect();
    format!("{}…", head.trim_end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenize_splits_and_lowercases() {
        assert_eq!(
            tokenize("Quarterly-Revenue, planning!"),
            vec!["quarterly", "revenue", "planning"]
        );
    }

    #[test]
    fn tokenize_empty_and_symbols() {
        assert!(tokenize("").is_empty());
        assert!(tokenize("--- , .").is_empty());
    }

    #[test]
    fn snippet_truncates_on_char_boundary() {
        let s = snippet("héllo world this is long", 7);
        assert!(s.ends_with('…'));
        assert!(s.chars().count() <= 8);
    }

    #[test]
    fn snippet_keeps_short_text() {
        assert_eq!(snippet("  short  ", 100), "short");
    }
}

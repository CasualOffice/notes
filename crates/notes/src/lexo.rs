//! Fractional / LexoRank-style `order_key` generation for projected `block` rows
//! (Data Model §4.2: `order_key TEXT` "fractional index (LexoRank) for reorder").
//!
//! Blocks are re-projected wholesale on every save (delete-and-reinsert per note),
//! so we generate a fresh set of evenly spaced, lexically ordered keys in document
//! order rather than mutating keys incrementally. Keys are fixed-width base-26
//! strings over `a`..=`z`; because they share a length, byte-lexical order equals
//! document order, and the even spacing leaves room for a later incremental
//! reorder helper ([`key_between`]) to insert between two siblings.

const DIGITS: usize = 6;

/// Generate `n` order keys in ascending document order.
///
/// The `i`-th key encodes the fraction `(i+1)/(n+1)` in base-26, guaranteeing
/// strictly increasing, evenly spaced, equal-length keys.
#[must_use]
pub fn order_keys(n: usize) -> Vec<String> {
    let den = (n as u128) + 1;
    (1..=n as u128).map(|i| encode_fraction(i, den)).collect()
}

/// Encode `num/den` (with `0 < num < den`) as a `DIGITS`-wide base-26 string.
fn encode_fraction(mut num: u128, den: u128) -> String {
    let mut s = String::with_capacity(DIGITS);
    for _ in 0..DIGITS {
        num = num.saturating_mul(26);
        let digit = (num / den) as u8;
        num %= den;
        s.push((b'a' + digit) as char);
    }
    s
}

/// Produce a key strictly between `lower` and `upper` for an incremental reorder.
///
/// Either bound may be `None` (open end). When both are `Some`, requires
/// `lower < upper`. Keys are drawn from `a`..=`z` and the result sorts strictly
/// between the bounds.
#[must_use]
pub fn key_between(lower: Option<&str>, upper: Option<&str>) -> String {
    match (lower, upper) {
        (None, None) => "n".to_string(),
        // Appending a mid digit always sorts strictly after `l`.
        (Some(l), None) => format!("{l}n"),
        (None, Some(u)) => key_before(u),
        (Some(l), Some(u)) => midpoint(l, u),
    }
}

/// A key that sorts strictly before `upper` (non-empty).
fn key_before(upper: &str) -> String {
    let bytes = upper.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if b > b'a' {
            let mut out = bytes[..i].to_vec();
            out.push(b - 1);
            return String::from_utf8(out).unwrap_or_default();
        }
    }
    // `upper` is all 'a's ("a", "aa", …): a shorter prefix sorts before it.
    upper[..upper.len().saturating_sub(1)].to_string()
}

/// A key strictly between two bounds (lexical, `lower < upper`).
fn midpoint(lower: &str, upper: &str) -> String {
    let width = lower.len().max(upper.len());
    let l = pad(lower, width);
    let u = pad(upper, width);
    let (mut a, mut b) = (0u128, 0u128);
    for (&la, &ua) in l.iter().zip(u.iter()) {
        a = a * 26 + u128::from(la - b'a');
        b = b * 26 + u128::from(ua - b'a');
    }
    let mid = (a + b) / 2;
    if mid > a {
        decode(mid, width)
    } else {
        // Adjacent at this width: append a mid digit — strictly between the bounds.
        format!("{lower}n")
    }
}

fn pad(s: &str, width: usize) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.resize(width, b'a');
    v
}

fn decode(mut n: u128, width: usize) -> String {
    let mut buf = vec![b'a'; width];
    for slot in buf.iter_mut().rev() {
        *slot = b'a' + (n % 26) as u8;
        n /= 26;
    }
    String::from_utf8(buf).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_keys_are_strictly_increasing() {
        for n in [0usize, 1, 2, 5, 33, 100] {
            let keys = order_keys(n);
            assert_eq!(keys.len(), n);
            for w in keys.windows(2) {
                assert!(w[0] < w[1], "not increasing: {:?} !< {:?}", w[0], w[1]);
            }
            assert!(keys.iter().all(|k| k.len() == DIGITS));
        }
    }

    #[test]
    fn key_between_open_ends() {
        let mid = key_between(None, None);
        let hi = key_between(Some(&mid), None);
        let lo = key_between(None, Some(&mid));
        assert!(lo < mid && mid < hi, "{lo} < {mid} < {hi}");
    }

    #[test]
    fn key_between_bounds() {
        let (a, z) = ("a".to_string(), "z".to_string());
        let m = key_between(Some(&a), Some(&z));
        assert!(a < m && m < z, "{a} < {m} < {z}");
    }

    #[test]
    fn key_between_adjacent_appends() {
        let m = key_between(Some("a"), Some("b"));
        assert!("a" < m.as_str() && m.as_str() < "b", "{m}");
    }
}

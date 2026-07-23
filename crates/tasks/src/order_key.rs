//! Fractional-index (`order_key`) utility for O(1) drag-reorder.
//!
//! Implements the reorder gesture of **Feature Specs §3.4** ("dragging within a
//! bucket rewrites only the moved item's `order_key` (fractional midpoint) — O(1),
//! no bulk renumber") and the `order_key TEXT` column of **Data Model §6** /
//! §4.2 ("LexoRank-style fractional index").
//!
//! ## The keyspace
//!
//! A key is a non-empty ASCII string over the base-62 alphabet
//! `0-9 A-Z a-z`, whose bytes are strictly ascending in ASCII, so **lexicographic
//! byte order equals numeric order**. A key `d0 d1 … dk` denotes the base-62
//! fraction `0.d0 d1 … dk` in the open interval `(0, 1)`.
//!
//! ## Canonical form
//!
//! Every key this module emits (and every key it accepts as a neighbour) is
//! *canonical*: non-empty and **not** ending in the zero digit `'0'`. Trailing
//! zeros are forbidden because `"V0"` and `"V"` denote the same fraction yet
//! compare differently as strings — allowing them would break the ordering
//! invariant. [`validate_key`] enforces this.
//!
//! ## Density under churn & rebalancing
//!
//! Repeatedly inserting between the *same* two neighbours grows the key length by
//! roughly one digit per insert (there is always room — the string simply gets
//! deeper), so keys never collide, but a hot spot lengthens over time. When keys
//! in a list get uncomfortably long, [`rebalance`] regenerates `n` evenly spaced,
//! minimal-length keys in one pass. The unit tests in this module assert the
//! ordering invariant and bounded per-insert growth under adversarial churn.

use crate::error::TaskError;

/// Base-62. Real digits are `0..BASE`; the value `BASE` is used only as an
/// internal "unbounded upper" sentinel inside [`midpoint`].
const BASE: u8 = 62;

/// Map an ASCII byte to its base-62 digit value, or `None` if out of alphabet.
const fn char_to_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'A'..=b'Z' => Some(c - b'A' + 10),
        b'a'..=b'z' => Some(c - b'a' + 36),
        _ => None,
    }
}

/// Map a base-62 digit value (`0..=61`) to its ASCII byte.
const fn digit_to_char(d: u8) -> u8 {
    match d {
        0..=9 => b'0' + d,
        10..=35 => b'A' + (d - 10),
        // 36..=61
        _ => b'a' + (d - 36),
    }
}

/// Parse and canonicality-check a key, returning its digit vector.
fn to_digits(s: &str) -> Result<Vec<u8>, TaskError> {
    if s.is_empty() {
        return Err(TaskError::InvalidOrderKey("empty".to_string()));
    }
    let mut v = Vec::with_capacity(s.len());
    for &c in s.as_bytes() {
        match char_to_digit(c) {
            Some(d) => v.push(d),
            None => {
                return Err(TaskError::InvalidOrderKey(format!(
                    "non-alphabet byte {c:#x} in {s:?}"
                )))
            }
        }
    }
    // Non-empty guaranteed above, so indexing the last element cannot panic.
    if v[v.len() - 1] == 0 {
        return Err(TaskError::InvalidOrderKey(format!(
            "trailing zero digit in {s:?} (non-canonical)"
        )));
    }
    Ok(v)
}

/// Render a digit vector back to its canonical ASCII key.
fn from_digits(d: &[u8]) -> String {
    // All bytes are valid ASCII by construction.
    d.iter().map(|&x| digit_to_char(x) as char).collect()
}

/// Assert a key is a valid, canonical fractional index.
///
/// # Errors
/// [`TaskError::InvalidOrderKey`] if `s` is empty, contains a non-alphabet byte,
/// or ends in the zero digit.
pub fn validate_key(s: &str) -> Result<(), TaskError> {
    to_digits(s).map(|_| ())
}

/// Core midpoint over digit slices.
///
/// Returns a digit vector strictly between `lo` and `hi` in base-62 fractional
/// space, where an empty/exhausted `lo` reads as the low boundary (all-zero) and
/// an empty/exhausted `hi` reads as the high boundary (unbounded). The result is
/// always non-empty and never ends in `0`.
fn midpoint(lo: &[u8], hi: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0usize;
    loop {
        let l = lo.get(i).copied().unwrap_or(0);
        // Real digits are < BASE; BASE means "hi is unbounded at this depth".
        let h = hi.get(i).copied().unwrap_or(BASE);
        if l == h {
            // Shared prefix digit — commit it and descend.
            out.push(l);
            i += 1;
            continue;
        }
        // Invariant: l < h here.
        let mid = (l + h) / 2;
        if mid != l {
            // A gap exists at this digit; a single midpoint digit finishes.
            out.push(mid);
            return out;
        }
        // mid == l  ⇒  h == l + 1: no gap. Commit l, then the upper bound
        // becomes unbounded, so find the first place we can step above lo's tail.
        out.push(l);
        i += 1;
        loop {
            let l2 = lo.get(i).copied().unwrap_or(0);
            let m2 = (l2 + BASE) / 2;
            if m2 != l2 {
                out.push(m2);
                return out;
            }
            out.push(l2);
            i += 1;
        }
    }
}

/// Generate a canonical key strictly between the two (optional) neighbours.
///
/// `None` means "unbounded": `key_between(None, None)` yields the first key of an
/// empty list; `key_between(Some(a), None)` appends after `a`;
/// `key_between(None, Some(b))` prepends before `b`.
///
/// # Errors
/// - [`TaskError::InvalidOrderKey`] if either neighbour is non-canonical.
/// - [`TaskError::OrderKeyOutOfOrder`] if `lo >= hi`.
pub fn key_between(lo: Option<&str>, hi: Option<&str>) -> Result<String, TaskError> {
    let lo_d = lo.map(to_digits).transpose()?;
    let hi_d = hi.map(to_digits).transpose()?;

    if let (Some(a), Some(b)) = (&lo_d, &hi_d) {
        // Digit-vector Ord equals key-string Ord (alphabet is ASCII-monotonic).
        if a >= b {
            return Err(TaskError::OrderKeyOutOfOrder {
                lo: lo.unwrap_or_default().to_string(),
                hi: hi.unwrap_or_default().to_string(),
            });
        }
    }

    let empty: [u8; 0] = [];
    let a = lo_d.as_deref().unwrap_or(empty.as_slice());
    let b = hi_d.as_deref().unwrap_or(empty.as_slice());
    let mid = midpoint(a, b);

    debug_assert!(!mid.is_empty(), "midpoint never returns empty");
    debug_assert!(mid.last() != Some(&0), "midpoint never ends in zero");
    if let (Some(av), Some(bv)) = (&lo_d, &hi_d) {
        debug_assert!(
            av.as_slice() < mid.as_slice() && mid.as_slice() < bv.as_slice(),
            "midpoint must be strictly between neighbours"
        );
    }

    Ok(from_digits(&mid))
}

/// The first key for an empty list (the mid of the whole keyspace).
#[must_use]
pub fn initial_key() -> String {
    // `midpoint(&[], &[])` is infallible and yields the single mid digit.
    from_digits(&midpoint(&[], &[]))
}

/// A key that sorts immediately **after** `lo` (append to a list).
///
/// # Errors
/// [`TaskError::InvalidOrderKey`] if `lo` is non-canonical.
pub fn key_after(lo: &str) -> Result<String, TaskError> {
    key_between(Some(lo), None)
}

/// A key that sorts immediately **before** `hi` (prepend to a list).
///
/// # Errors
/// [`TaskError::InvalidOrderKey`] if `hi` is non-canonical.
pub fn key_before(hi: &str) -> Result<String, TaskError> {
    key_between(None, Some(hi))
}

/// Regenerate `count` evenly spaced, minimal-length canonical keys.
///
/// Use this to *rebalance* a list whose keys have grown long under churn: the
/// returned vector is strictly increasing and each key is short (its length is
/// governed by `count`, not by the list's edit history). Returns an empty vector
/// for `count == 0`.
#[must_use]
pub fn rebalance(count: usize) -> Vec<String> {
    (1..=count)
        .map(|i| fraction_key(i as u64, (count as u64) + 1))
        .collect()
}

/// Base-62 expansion of the proper fraction `num/den` (`0 < num < den`) as a
/// canonical key. Distinct fractions map to distinct, order-preserving keys.
fn fraction_key(num: u64, den: u64) -> String {
    let mut n = num % den;
    let mut out = Vec::new();
    // A 24-digit cap bounds pathological denominators; evenly-spaced rebalance
    // fractions terminate far sooner.
    for _ in 0..24 {
        if n == 0 {
            break;
        }
        n *= u64::from(BASE);
        out.push((n / den) as u8);
        n %= den;
    }
    while out.last() == Some(&0) {
        out.pop();
    }
    if out.is_empty() {
        // Only reachable for a degenerate num==0; keep a valid mid digit.
        out.push(BASE / 2);
    }
    from_digits(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny deterministic LCG so the property tests are reproducible and need
    /// no external crate (workspace stays network-free, no new deps).
    struct Lcg(u64);
    impl Lcg {
        fn next_u64(&mut self) -> u64 {
            // Numerical Recipes constants.
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next_u64() % (n as u64)) as usize
        }
    }

    fn assert_strictly_sorted(keys: &[String]) {
        for w in keys.windows(2) {
            assert!(
                w[0] < w[1],
                "not strictly increasing: {:?} !< {:?}",
                w[0],
                w[1]
            );
        }
    }

    #[test]
    fn alphabet_is_ascii_monotonic() {
        // Every digit's byte must be strictly greater than the previous digit's,
        // which is what makes lexicographic order equal numeric order.
        for d in 1..BASE {
            assert!(digit_to_char(d) > digit_to_char(d - 1));
            assert_eq!(char_to_digit(digit_to_char(d)), Some(d));
        }
    }

    #[test]
    fn initial_and_bounds() {
        let k = initial_key();
        validate_key(&k).unwrap();
        // before < initial < after
        let b = key_before(&k).unwrap();
        let a = key_after(&k).unwrap();
        assert!(b < k && k < a, "{b} < {k} < {a}");
    }

    #[test]
    fn between_is_strict() {
        let a = key_between(None, None).unwrap();
        let b = key_after(&a).unwrap();
        let mid = key_between(Some(&a), Some(&b)).unwrap();
        assert!(a < mid && mid < b);
        validate_key(&mid).unwrap();
    }

    #[test]
    fn rejects_out_of_order_and_invalid() {
        let a = initial_key();
        let b = key_after(&a).unwrap();
        assert!(matches!(
            key_between(Some(&b), Some(&a)),
            Err(TaskError::OrderKeyOutOfOrder { .. })
        ));
        assert!(matches!(
            key_between(Some(&a), Some(&a)),
            Err(TaskError::OrderKeyOutOfOrder { .. })
        ));
        // Non-canonical inputs.
        assert!(key_before("V0").is_err()); // trailing zero
        assert!(key_before("").is_err()); // empty
        assert!(key_before("!!").is_err()); // bad byte
    }

    #[test]
    fn append_sequence_is_ordered() {
        // Simulate building a list by always appending at the end.
        let mut keys = vec![initial_key()];
        for _ in 0..1_000 {
            let next = key_after(keys.last().unwrap()).unwrap();
            validate_key(&next).unwrap();
            keys.push(next);
        }
        assert_strictly_sorted(&keys);
    }

    #[test]
    fn prepend_sequence_is_ordered() {
        let mut keys = vec![initial_key()];
        for _ in 0..1_000 {
            let prev = key_before(&keys[0]).unwrap();
            validate_key(&prev).unwrap();
            keys.insert(0, prev);
        }
        assert_strictly_sorted(&keys);
    }

    #[test]
    fn hotspot_churn_grows_slowly_and_stays_ordered() {
        // Adversarial: always insert between the SAME two neighbours. Key length
        // must grow ~1 digit per insert (bounded), never collide, stay ordered.
        let lo = initial_key();
        let hi = key_after(&lo).unwrap();
        let mut prev_hi = hi.clone();
        let mut inserted = Vec::new();
        for n in 0..500 {
            let mid = key_between(Some(&lo), Some(&prev_hi)).unwrap();
            assert!(lo < mid && mid < prev_hi);
            // Length grows at most ~1 digit per insert against a moving neighbour.
            assert!(
                mid.len() <= n + 2,
                "length {} blew past the ~1-digit/insert bound at n={n}",
                mid.len()
            );
            inserted.push(mid.clone());
            prev_hi = mid;
        }
        // The successive midpoints march monotonically down toward `lo`.
        for w in inserted.windows(2) {
            assert!(w[1] < w[0]);
        }
    }

    #[test]
    fn random_insertions_preserve_global_order_and_uniqueness() {
        // The master invariant: after arbitrary random insertions at random gaps,
        // the list is still strictly sorted and every key is distinct & canonical.
        let mut rng = Lcg(0x1234_5678_9abc_def0);
        let mut keys = vec![initial_key()];
        for _ in 0..3_000 {
            let pos = rng.below(keys.len() + 1); // gap index 0..=len
            let lo = if pos == 0 {
                None
            } else {
                Some(keys[pos - 1].as_str())
            };
            let hi = if pos == keys.len() {
                None
            } else {
                Some(keys[pos].as_str())
            };
            let k = key_between(lo, hi).unwrap();
            validate_key(&k).unwrap();
            keys.insert(pos, k);
        }
        assert_strictly_sorted(&keys);
        let mut sorted = keys.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), keys.len(), "duplicate keys generated");
    }

    #[test]
    fn rebalance_is_short_sorted_and_distinct() {
        for count in [0usize, 1, 2, 5, 62, 500] {
            let keys = rebalance(count);
            assert_eq!(keys.len(), count);
            assert_strictly_sorted(&keys);
            for k in &keys {
                validate_key(k).unwrap();
            }
            let mut d = keys.clone();
            d.dedup();
            assert_eq!(d.len(), keys.len());
        }
        // Rebalancing resets density: a hot-spot list of long keys becomes short.
        let long = {
            let lo = initial_key();
            let mut hi = key_after(&lo).unwrap();
            for _ in 0..200 {
                hi = key_between(Some(&lo), Some(&hi)).unwrap();
            }
            hi.len()
        };
        let fresh_max = rebalance(200).iter().map(String::len).max().unwrap();
        assert!(fresh_max < long, "rebalance did not reduce key length");
    }
}

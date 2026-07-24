//! Pure vector math for retrieval. Implements the cosine-similarity kernel behind
//! the brute-force KNN of Data Model §9.3 / §10.1 (the fallback before the native
//! `sqlite-vec` distance operator is loaded).
//!
//! All functions are total and allocation-light. Length mismatches are handled by
//! iterating the common prefix (`zip`) rather than panicking, since a non-test
//! path must never `unwrap`/panic (CLAUDE.md invariants).

/// Dot product over the common prefix of `a` and `b`.
#[must_use]
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Euclidean (L2) norm of `v`.
#[must_use]
pub fn norm(v: &[f32]) -> f32 {
    dot(v, v).sqrt()
}

/// Cosine similarity in `[-1.0, 1.0]`. Higher is more similar. Returns `0.0` if
/// either vector has zero magnitude (an all-zero vector has no direction).
#[must_use]
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    // Norms are non-negative, so `denom <= 0.0` is exactly `denom == 0.0` while
    // staying an ordering comparison (never a float-equality lint concern).
    let denom = norm(a) * norm(b);
    if denom <= 0.0 {
        return 0.0;
    }
    (dot(a, b) / denom).clamp(-1.0, 1.0)
}

/// L2-normalize `v` in place to unit length and return the original norm. If `v`
/// is all-zero it is left unchanged and `0.0` is returned (no NaN from `0/0`).
pub fn l2_normalize(v: &mut [f32]) -> f32 {
    let n = norm(v);
    if n > 0.0 {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
    n
}

/// Allocate a unit-normalized copy of `v` (see [`l2_normalize`]).
#[must_use]
pub fn normalized(v: &[f32]) -> Vec<f32> {
    let mut out = v.to_vec();
    l2_normalize(&mut out);
    out
}

/// Whether `v` is unit length within `tol` (useful in assertions/tests).
#[must_use]
pub fn is_unit(v: &[f32], tol: f32) -> bool {
    (norm(v) - 1.0).abs() <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_of_identical_is_one() {
        let v = vec![1.0, 2.0, 3.0];
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_orthogonal_is_zero() {
        assert!(cosine_similarity(&[1.0, 0.0], &[0.0, 1.0]).abs() < 1e-6);
    }

    #[test]
    fn cosine_of_opposite_is_minus_one() {
        assert!((cosine_similarity(&[1.0, 1.0], &[-1.0, -1.0]) + 1.0).abs() < 1e-6);
    }

    #[test]
    fn cosine_with_zero_vector_is_zero_not_nan() {
        let s = cosine_similarity(&[0.0, 0.0], &[1.0, 1.0]);
        assert_eq!(s, 0.0);
        assert!(!s.is_nan());
    }

    #[test]
    fn normalize_yields_unit_length() {
        let u = normalized(&[3.0, 4.0]);
        assert!(is_unit(&u, 1e-6));
        assert!((u[0] - 0.6).abs() < 1e-6 && (u[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn normalize_zero_is_left_alone() {
        let mut z = vec![0.0, 0.0];
        assert_eq!(l2_normalize(&mut z), 0.0);
        assert_eq!(z, vec![0.0, 0.0]);
    }
}

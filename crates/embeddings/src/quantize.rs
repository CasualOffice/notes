//! Matryoshka truncation and int8 quantization helpers (Data Model §9.3,
//! Architecture §14: *"Embeddings Matryoshka-truncated to 256 dims + int8"*).
//!
//! These are **pure functions used by the later storage/quantization path**. The
//! default at rest in this crate stays **f32** (see [`crate::VectorStore`]); these
//! helpers define the exact truncate/quantize transforms so the real embedder and
//! the eventual `sqlite-vec FLOAT[256]` (int8-backed) column agree byte-for-byte.
//!
//! - **Matryoshka truncation** exploits that a Matryoshka-trained embedding keeps
//!   its most significant semantics in the leading dimensions, so a unit-normalized
//!   prefix is a valid lower-dimensional embedding (re-normalized here).
//! - **int8 quantization** is symmetric per-vector: one `f32` scale, codes in
//!   `[-127, 127]`. Round-trips within `scale/2` per component.

use serde::{Deserialize, Serialize};

/// A symmetrically int8-quantized vector: `codes[i] * scale ≈ original[i]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Int8Vector {
    /// Quantized components in `[-127, 127]`.
    pub codes: Vec<i8>,
    /// The per-vector dequantization scale (`max|x| / 127`).
    pub scale: f32,
}

impl Int8Vector {
    /// Number of components.
    #[must_use]
    pub fn len(&self) -> usize {
        self.codes.len()
    }

    /// Whether the vector is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.codes.is_empty()
    }
}

/// Matryoshka-truncate `v` to its leading `dims` components and re-normalize to
/// unit length. If `dims >= v.len()` the whole (re-normalized) vector is returned.
#[must_use]
pub fn truncate(v: &[f32], dims: usize) -> Vec<f32> {
    let take = dims.min(v.len());
    let mut out = v[..take].to_vec();
    crate::math::l2_normalize(&mut out);
    out
}

/// Symmetric per-vector int8 quantization. `scale = max|x| / 127`; each component
/// is `round(x / scale)` clamped to `[-127, 127]`. An all-zero (or empty) vector
/// yields all-zero codes with `scale = 1.0` (never a divide-by-zero).
#[must_use]
pub fn quantize_int8(v: &[f32]) -> Int8Vector {
    let max_abs = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let scale = if max_abs > 0.0 { max_abs / 127.0 } else { 1.0 };
    let codes = v
        .iter()
        .map(|x| {
            let q = (x / scale).round();
            q.clamp(-127.0, 127.0) as i8
        })
        .collect();
    Int8Vector { codes, scale }
}

/// Reconstruct an approximate `f32` vector from its int8 codes.
#[must_use]
pub fn dequantize_int8(q: &Int8Vector) -> Vec<f32> {
    q.codes.iter().map(|&c| f32::from(c) * q.scale).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math;

    #[test]
    fn truncate_prefix_and_renormalizes() {
        let v = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6];
        let t = truncate(&v, 3);
        assert_eq!(t.len(), 3);
        assert!(math::is_unit(&t, 1e-6));
        // Directions of the kept prefix are preserved (monotone increasing here).
        assert!(t[0] < t[1] && t[1] < t[2]);
    }

    #[test]
    fn truncate_beyond_len_returns_full_normalized() {
        let t = truncate(&[3.0, 4.0], 10);
        assert_eq!(t.len(), 2);
        assert!(math::is_unit(&t, 1e-6));
    }

    #[test]
    fn int8_roundtrip_within_tolerance() {
        let v = vec![-0.9, -0.3, 0.0, 0.15, 0.42, 0.87, 1.0];
        let q = quantize_int8(&v);
        let back = dequantize_int8(&q);
        // Symmetric quantization error is bounded by half a step (scale/2).
        let tol = q.scale / 2.0 + 1e-6;
        for (orig, r) in v.iter().zip(back.iter()) {
            assert!((orig - r).abs() <= tol, "orig {orig} vs {r}, tol {tol}");
        }
    }

    #[test]
    fn int8_preserves_cosine_closely() {
        // Quantization must barely move the retrieval geometry.
        let a = vec![0.2, 0.5, -0.3, 0.8, -0.1];
        let qa = dequantize_int8(&quantize_int8(&a));
        assert!(math::cosine_similarity(&a, &qa) > 0.999);
    }

    #[test]
    fn zero_vector_quantizes_without_nan() {
        let q = quantize_int8(&[0.0, 0.0, 0.0]);
        assert_eq!(q.scale, 1.0);
        assert!(q.codes.iter().all(|&c| c == 0));
        assert!(dequantize_int8(&q).iter().all(|x| !x.is_nan()));
    }
}

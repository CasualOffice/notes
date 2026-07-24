//! The [`Embedder`] trait and a deterministic [`MockEmbedder`] test double.
//!
//! Implements the swappable embedder seam of **Architecture §5 ("Embeddings —
//! Trait + adapters")** and **HLD §5** (`embeddings` crate). The real model
//! (EmbeddingGemma-300M / bge-base via GGUF/ONNX, Matryoshka-256 int8) is OUT OF
//! SCOPE for this pure-Rust layer — it will be a second `impl Embedder` behind the
//! same trait. The [`MockEmbedder`] hashes text into a fixed-dimension unit vector
//! so retrieval (KNN, RRF fusion, citation-verify) is fully testable offline with
//! **no model and no native extension**.

use crate::error::EmbeddingResult;
use crate::math;

/// Produces dense vector embeddings for text. The single retrieval-side contract
/// every embedding backend (mock, GGUF, ONNX) implements.
///
/// `embed` is fallible: the real model seam can fail (model not resident, inference
/// error), surfaced as [`EmbeddingError::ModelNotLoaded`](crate::EmbeddingError) /
/// [`Embed`](crate::EmbeddingError::Embed). Retrieval then degrades to FTS-only
/// (Architecture §10) rather than blocking. `dimension` and `model_id` are
/// infallible metadata used to stamp `embedding.dims` / `embedding.embed_model`
/// provenance (Data Model §9.3).
pub trait Embedder: Send + Sync {
    /// Embed a batch of texts, returning one vector per input in order. Each
    /// returned vector has length [`Embedder::dimension`].
    fn embed(&self, texts: &[&str]) -> EmbeddingResult<Vec<Vec<f32>>>;

    /// The fixed output dimensionality (e.g. `256` after Matryoshka truncation).
    fn dimension(&self) -> usize;

    /// Provenance string recorded per embedding (`embedding.embed_model`), e.g.
    /// `"embeddinggemma-300m@256d-int8"`. A model swap re-embeds only rows whose
    /// stored `embed_model` differs (Data Model §9.3, §12).
    fn model_id(&self) -> &str;

    /// Convenience: embed a single text.
    ///
    /// # Errors
    /// Propagates any [`crate::EmbeddingError`] from [`Embedder::embed`].
    fn embed_one(&self, text: &str) -> EmbeddingResult<Vec<f32>> {
        let mut out = self.embed(&[text])?;
        Ok(out.pop().unwrap_or_default())
    }
}

/// A deterministic, model-free embedder for tests and offline development.
///
/// Guarantees, so retrieval is testable without a model:
/// - **Deterministic:** same text → byte-identical vector, every run, every
///   platform (integer hashing + `f64` arithmetic only).
/// - **Discriminative:** different text → different vector (distinct hash seed).
/// - **Unit-normalized:** every output has L2 norm `1.0`, so a dot product *is* the
///   cosine similarity — exactly the geometry the real embedder targets.
///
/// It carries no semantic meaning; it only reproduces the *shape and algebra* of a
/// real embedding space so KNN/RRF/citation-verify can be exercised deterministically.
#[derive(Clone, Debug)]
pub struct MockEmbedder {
    dims: usize,
    model_id: String,
}

impl MockEmbedder {
    /// The default Matryoshka dimensionality used across the product (Data Model
    /// §9.3: `256`).
    pub const DEFAULT_DIMS: usize = 256;

    /// A mock at the product-default 256 dimensions.
    #[must_use]
    pub fn new() -> Self {
        Self::with_dimension(Self::DEFAULT_DIMS)
    }

    /// A mock at an explicit dimensionality (small dims keep unit tests cheap).
    #[must_use]
    pub fn with_dimension(dims: usize) -> Self {
        Self {
            dims: dims.max(1),
            model_id: format!("mock-embedder@{dims}d"),
        }
    }

    /// Override the provenance string (to test mixed-`embed_model` KNN filtering).
    #[must_use]
    pub fn with_model_id(mut self, id: impl Into<String>) -> Self {
        self.model_id = id.into();
        self
    }

    /// The deterministic unit vector for `text`. Pure; also the kernel of
    /// [`Embedder::embed`].
    #[must_use]
    pub fn embed_text(&self, text: &str) -> Vec<f32> {
        // Seed a SplitMix64 stream from a stable FNV-1a-64 hash of the bytes, then
        // draw `dims` pseudo-random components in [-1, 1] and unit-normalize.
        let mut state = fnv1a64(text.as_bytes());
        let mut v = Vec::with_capacity(self.dims);
        for _ in 0..self.dims {
            v.push(unit_signed(splitmix64(&mut state)));
        }
        // Degenerate guard: a zero draw would have no direction. Pin one axis so
        // every text — including "" — maps to a valid unit vector. The norm is
        // non-negative, so `<= 0.0` is exactly "was zero".
        if math::l2_normalize(&mut v) <= 0.0 {
            if let Some(first) = v.first_mut() {
                *first = 1.0;
            }
        }
        v
    }
}

impl Default for MockEmbedder {
    fn default() -> Self {
        Self::new()
    }
}

impl Embedder for MockEmbedder {
    fn embed(&self, texts: &[&str]) -> EmbeddingResult<Vec<Vec<f32>>> {
        Ok(texts.iter().map(|t| self.embed_text(t)).collect())
    }

    fn dimension(&self) -> usize {
        self.dims
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }
}

/// FNV-1a 64-bit — a stable, platform-independent string hash for the PRNG seed.
/// (Not cryptographic; used only to spread distinct texts to distinct seeds.)
fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = OFFSET;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(PRIME);
    }
    h
}

/// SplitMix64 step — a fast, well-distributed deterministic PRNG.
fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Map a `u64` draw to a signed component in `[-1.0, 1.0)` via a 53-bit mantissa.
fn unit_signed(bits: u64) -> f32 {
    let unit = (bits >> 11) as f64 / (1u64 << 53) as f64; // [0, 1)
    (unit * 2.0 - 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_text_same_vector() {
        let e = MockEmbedder::with_dimension(64);
        assert_eq!(e.embed_text("hello world"), e.embed_text("hello world"));
    }

    #[test]
    fn different_text_different_vector() {
        let e = MockEmbedder::with_dimension(64);
        assert_ne!(e.embed_text("alpha"), e.embed_text("beta"));
    }

    #[test]
    fn every_vector_is_unit_normalized() {
        let e = MockEmbedder::with_dimension(128);
        for t in [
            "",
            "a",
            "the quick brown fox",
            "another sample chunk of text",
        ] {
            let v = e.embed_text(t);
            assert_eq!(v.len(), 128);
            assert!(math::is_unit(&v, 1e-5), "text {t:?} not unit length");
        }
    }

    #[test]
    fn empty_text_is_valid_unit_vector() {
        let e = MockEmbedder::with_dimension(16);
        let v = e.embed_text("");
        assert!(math::is_unit(&v, 1e-5));
    }

    #[test]
    fn batch_embed_matches_single_and_reports_metadata() {
        let e = MockEmbedder::with_dimension(32).with_model_id("mock@32d-test");
        let batch = e.embed(&["one", "two", "three"]).unwrap();
        assert_eq!(batch.len(), 3);
        assert_eq!(batch[1], e.embed_text("two"));
        assert_eq!(e.dimension(), 32);
        assert_eq!(e.model_id(), "mock@32d-test");
    }

    #[test]
    fn self_similarity_beats_cross_similarity() {
        // Unrelated texts should be near-orthogonal; identical texts perfectly
        // aligned — the property KNN relies on.
        let e = MockEmbedder::with_dimension(256);
        let a = e.embed_text("quarterly revenue planning meeting");
        let a2 = e.embed_text("quarterly revenue planning meeting");
        let b = e.embed_text("completely different subject matter");
        assert!((math::cosine_similarity(&a, &a2) - 1.0).abs() < 1e-5);
        assert!(math::cosine_similarity(&a, &b) < 0.5);
    }
}

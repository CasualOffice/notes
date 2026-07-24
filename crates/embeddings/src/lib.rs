//! # embeddings — local vector retrieval (pure-Rust + mock layer)
//!
//! Implements the retrieval-side substrate of **Architecture §5 ("Embeddings —
//! Trait + adapters; incremental content-hash-gated embedding; sqlite-vec
//! integration")** and **Data Model §9.2/§9.3** (`chunk` / `embedding` /
//! `vec_chunk`), feeding the hybrid FTS5 ∪ vector KNN + RRF pipeline of
//! **§10.1 / HLD §8.5** and the grounded Ask flow of **HLD §8.5 / Data Model §14.2**.
//!
//! This crate is the layer that runs with **no embedding model and no native
//! extension**, so retrieval, RRF fusion, and citation-verify are all testable
//! offline:
//!
//! - [`Embedder`] — the swappable embedding trait; [`MockEmbedder`] is a
//!   deterministic, unit-normalized test double (same text → same vector).
//! - [`VectorStore`] — persists per-chunk `f32` embeddings with `embed_model` /
//!   `dims` / `content_hash` provenance and answers **brute-force cosine KNN**
//!   ([`Neighbor`]). A `sqlite-vec` seam is documented on [`VectorStore`]
//!   (constants [`VEC_CHUNK_DDL`] / [`store::EMBEDDING_CHUNK_DDL`]) — the extension
//!   is **not** loaded here.
//! - Incremental, **content-hash-gated** embedding ([`ContentHash`],
//!   [`UpsertOutcome`], [`VectorStore::upsert_text_gated`]) skips re-embedding a
//!   chunk whose BLAKE3 content hash is unchanged (Data Model §9.2).
//! - [`quantize`] — Matryoshka truncation + symmetric int8 quantization as pure
//!   functions (documented, used by the later storage path; `f32` stays the
//!   default at rest).
//!
//! ## Out-of-scope seams (intentionally left)
//! The real embedder (EmbeddingGemma-300M / bge-base, GGUF/ONNX) is a future
//! `impl Embedder`; the native `sqlite-vec` `vec0` index is a future storage swap
//! behind the unchanged [`VectorStore`] API. Neither opens a socket — the crate is
//! fully local and offline (CLAUDE.md invariants).
//!
//! ## Grounding invariant
//! This crate only *retrieves* candidate chunks; it never fabricates. The
//! refusal-over-hallucination gate (an answer with zero resolvable citations
//! becomes `unanswered:true`, Data Model §14.2 / HLD N14) lives in `ai-workspace`,
//! which consumes [`Neighbor`] rankings from here.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod embedder;
pub mod error;
pub mod math;
pub mod quantize;
pub mod store;

pub use embedder::{Embedder, MockEmbedder};
pub use error::{EmbeddingError, EmbeddingResult};
pub use quantize::{dequantize_int8, quantize_int8, truncate, Int8Vector};
pub use store::{
    ContentHash, Neighbor, StoredEmbedding, UpsertOutcome, VectorStore, VEC_CHUNK_DDL,
};

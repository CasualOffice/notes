//! # ai-workspace — the evidence-grounded "Ask your notes" pipeline
//!
//! Implements the grounded RAG flow of **HLD §8.5** and the **`AnswerV1`**
//! citation-verify contract of **Data Model §14.2**, over the universal
//! `chunk`/`entity` retrieval spine of **Data Model §9.2/§10.1**. Refusal-over-
//! hallucination is a hard gate (**N14 / Foundation §4**): a displayed answer's every
//! citation resolves to a real retrieved chunk, or the answer is returned as
//! `unanswered:true` and nothing is shown.
//!
//! This is the **pure-Rust + mock** layer — it runs with **no real LLM and no real
//! embedding model**. [`MockLlm`](llm_api::MockLlm) (from `llm-api`) and
//! [`MockEmbedder`](embeddings::MockEmbedder) (from `embeddings`) make the entire
//! flow testable offline (CLAUDE.md: core crates build and pass with the network
//! disabled). The real embedding model, the native `sqlite-vec` KNN, and the FTS5
//! tables are documented seams behind the [`Retriever`] trait and `llm-llamacpp`.
//!
//! ## The pipeline ([`ask`])
//! 1. **Retrieve** ([`retrieve`]) — lexical BM25 ∪ vector KNN candidates, **fused by
//!    Reciprocal Rank Fusion** ([`search::rrf_fuse`], reused verbatim). A soft
//!    embedder failure degrades to lexical-only rather than failing. An optional
//!    rerank hook is a no-op stub.
//! 2. **Grounded decode** ([`decode`]) — a prompt built from *numbered* evidence
//!    drives [`ConstrainedLlm`](llm_api::ConstrainedLlm) through the repair →
//!    deterministic-fallback contract to an [`AnswerV1`]. The fallback is an honest
//!    refusal, so a malformed model can only ever yield `unanswered`.
//! 3. **Citation-verify** ([`verify`]) — the load-bearing gate: every citation must
//!    resolve to a retrieved chunk; survivors are rebuilt from ground truth; zero
//!    survivors ⇒ `unanswered`.
//! 4. **Suggestions** ([`suggest`]) — reversible, cited auto-link / auto-tag
//!    [`Suggestion`] records (data only; never applied).
//!
//! ## Public API
//! [`ask`]`(&`[`AskContext`]`, query) ->` [`AnswerVerdict`] is the entry point;
//! [`InMemoryCorpus`] is the offline [`Retriever`]; [`Suggestion`] +
//! [`auto_links_from_answer`] are the suggestion surface.

#![forbid(unsafe_code)]

pub mod ask;
pub mod chunk;
pub mod decode;
pub mod error;
pub mod retrieve;
pub mod suggest;
pub mod verify;

mod text;

// ---------------------------------------------------------------------------
// Flat re-exports of the public surface.
// ---------------------------------------------------------------------------

pub use ask::{ask, AnswerVerdict, Answered, AskConfig, AskContext, Unanswered};
pub use chunk::Chunk;
pub use decode::{answer_grammar, build_prompt, decode_answer, render_evidence};
pub use error::{AskError, AskResult};
pub use retrieve::{fuse_channels, rerank_identity, InMemoryCorpus, RetrievalResult, Retriever};
pub use suggest::{
    auto_links_from_answer, cited_entity, Suggestion, SuggestionKind, SuggestionState,
};
pub use verify::{verify, UnansweredReason, VerifiedAnswer};

// Re-export the answer/citation contract types callers need, so a consumer can work
// against `ai_workspace::{AnswerV1, Citation, SourceKind}` without also naming
// `llm-api` directly (they remain owned by `llm-api`, Data Model §14).
pub use llm_api::{AnswerV1, Citation, GenerationPath, SourceKind};

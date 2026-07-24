//! Error taxonomy for the Ask pipeline. Bridges to the shared
//! [`app_domain::AppError`] classes (HLD §10) so `ai.ask` surfaces a retrieval /
//! generation failure through the same typed wire shape as any other command.
//!
//! Note the deliberate asymmetry with the *grounding* verdict: a schema violation
//! or an unresolvable citation is **not** an [`AskError`] — it is handled inside
//! the pipeline and returned as an `unanswered:true` [`AnswerVerdict`](crate::AnswerVerdict)
//! ("Evidence or nothing", Foundation §4 / N14). [`AskError`] carries only real
//! transport/backend failures (the LLM backend, the vector store), on which the
//! caller may retry.

use app_domain::AppError;
use embeddings::EmbeddingError;
use llm_api::LlmError;

/// A transport/backend failure in the Ask pipeline (never a grounding verdict).
#[derive(Debug, thiserror::Error)]
pub enum AskError {
    /// The vector-retrieval layer failed durably (storage / corrupt blob /
    /// dimension mismatch). A *soft* embedder failure (model-not-loaded) does
    /// **not** land here — it degrades retrieval to FTS-only instead.
    #[error("retrieval error: {0}")]
    Retrieval(#[from] EmbeddingError),

    /// The constrained-decode backend failed (model not loaded, queue full,
    /// cancelled, decode error). Propagated, not absorbed by the fallback — the
    /// caller keeps its context and may retry (llm-api contract).
    #[error("llm error: {0}")]
    Llm(#[from] LlmError),

    /// A durable SQLite failure setting up or reading the in-memory index.
    #[error("storage error: {0}")]
    Storage(String),

    /// A JSON (de)serialization failure at a suggestion/answer boundary.
    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Convenience alias for fallible Ask paths.
pub type AskResult<T> = Result<T, AskError>;

impl From<rusqlite::Error> for AskError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<serde_json::Error> for AskError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

impl From<AskError> for AppError {
    fn from(e: AskError) -> Self {
        match e {
            // Reuse the embeddings->AppError bridge (retryable model-not-loaded,
            // validation on dimension mismatch, storage otherwise).
            AskError::Retrieval(inner) => inner.into(),
            AskError::Llm(inner) => match inner {
                LlmError::ModelNotLoaded => Self::ModelNotLoaded("llm model not loaded".into()),
                // Backpressure on the bounded request queue is a transient,
                // retryable condition (HLD §10 taxonomy).
                LlmError::QueueFull => Self::TransientIo("llm request queue full".into()),
                LlmError::Cancelled => Self::Cancelled,
                other => Self::Generation(other.to_string()),
            },
            AskError::Storage(m) => Self::Storage(m),
            AskError::Serialization(m) => Self::Serialization(m),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::ErrorClass;

    #[test]
    fn llm_model_not_loaded_bridges_to_retryable_class() {
        let app: AppError = AskError::Llm(LlmError::ModelNotLoaded).into();
        assert_eq!(app.class(), ErrorClass::ModelNotLoaded);
        assert!(app.retryable());
    }

    #[test]
    fn queue_full_bridges_to_transient_retryable() {
        let app: AppError = AskError::Llm(LlmError::QueueFull).into();
        assert_eq!(app.class(), ErrorClass::TransientIo);
        assert!(app.retryable());
    }

    #[test]
    fn decode_failed_bridges_to_generation_terminal() {
        let app: AppError = AskError::Llm(LlmError::DecodeFailed("x".into())).into();
        assert_eq!(app.class(), ErrorClass::Generation);
        assert!(!app.retryable());
    }

    #[test]
    fn retrieval_dimension_mismatch_bridges_to_validation() {
        let app: AppError = AskError::Retrieval(EmbeddingError::DimensionMismatch {
            expected: 256,
            actual: 8,
        })
        .into();
        assert_eq!(app.class(), ErrorClass::Validation);
    }
}

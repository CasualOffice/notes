//! Error taxonomy for the embeddings layer. Bridges to the shared
//! [`app_domain::AppError`] classes (HLD §10) so `ai-workspace` can surface an
//! embedding failure through the same typed wire shape as any other command.

use app_domain::AppError;

/// Failures raised by the [`Embedder`](crate::Embedder) trait and the
/// [`VectorStore`](crate::VectorStore).
#[derive(Debug, thiserror::Error)]
pub enum EmbeddingError {
    /// The underlying embedder (real model seam) failed to produce vectors.
    #[error("embedder failed: {0}")]
    Embed(String),

    /// A required embedding model is not resident yet (real model seam).
    /// Maps to the retryable [`app_domain::ErrorClass::ModelNotLoaded`].
    #[error("embedding model not loaded: {0}")]
    ModelNotLoaded(String),

    /// A vector's dimension does not match the store/model contract.
    #[error("dimension mismatch: expected {expected}, got {actual}")]
    DimensionMismatch { expected: usize, actual: usize },

    /// A persisted vector blob was not a whole number of little-endian `f32`s
    /// (corrupt row — the derived index must be rebuilt from truth).
    #[error("corrupt vector blob: {0} bytes is not a multiple of 4")]
    CorruptVectorBlob(usize),

    /// A durable SQLite/SQLCipher failure in the vector store.
    #[error("vector store error: {0}")]
    Storage(String),
}

/// Convenience alias for fallible embeddings paths.
pub type EmbeddingResult<T> = Result<T, EmbeddingError>;

impl From<rusqlite::Error> for EmbeddingError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e.to_string())
    }
}

impl From<EmbeddingError> for AppError {
    fn from(e: EmbeddingError) -> Self {
        match e {
            // A failed embed is a generation-family failure in the AI pipeline;
            // retrieval degrades to FTS-only meanwhile (Architecture §10 table).
            EmbeddingError::Embed(m) => Self::Generation(m),
            EmbeddingError::ModelNotLoaded(m) => Self::ModelNotLoaded(m),
            EmbeddingError::DimensionMismatch { expected, actual } => Self::Validation(format!(
                "dimension mismatch: expected {expected}, got {actual}"
            )),
            EmbeddingError::CorruptVectorBlob(n) => Self::Storage(format!(
                "corrupt vector blob: {n} bytes not a multiple of 4"
            )),
            EmbeddingError::Storage(m) => Self::Storage(m),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::ErrorClass;

    #[test]
    fn model_not_loaded_bridges_to_retryable_class() {
        let app: AppError = EmbeddingError::ModelNotLoaded("embedder".into()).into();
        assert_eq!(app.class(), ErrorClass::ModelNotLoaded);
        assert!(app.retryable());
    }

    #[test]
    fn dimension_mismatch_bridges_to_validation() {
        let app: AppError = EmbeddingError::DimensionMismatch {
            expected: 256,
            actual: 128,
        }
        .into();
        assert_eq!(app.class(), ErrorClass::Validation);
        assert!(!app.retryable());
    }
}

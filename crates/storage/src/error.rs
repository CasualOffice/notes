//! Storage error taxonomy. Implements the durable-storage slice of the
//! Architecture §10 / HLD §10 error model.
//!
//! Library code uses [`StorageError`] (`thiserror`); it converts into the
//! workspace-wide [`AppError`](app_domain::AppError) at the service boundary so
//! the WebView still sees the stable `{class, retryable, message}` wire shape.

use app_domain::AppError;

/// Errors raised by the `storage` crate.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// A SQLite / SQLCipher failure (open, key, migrate, or statement).
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    /// A filesystem / IO failure (journals, blobs, key file). Retryable.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// (De)serialization of an op payload / journal record failed.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// The DB `user_version` is newer than this binary understands — refuse to
    /// open (prevents old-binary + new-DB corruption; Data Model §13.2).
    #[error("database schema version {found} is newer than supported {supported}")]
    SchemaTooNew { found: i64, supported: i64 },

    /// OS keystore access failed and no dev fallback was permitted.
    #[error("keystore error: {0}")]
    Keystore(String),

    /// A content-addressed blob failed its SHA-256 integrity check on read.
    #[error("blob integrity failure: expected {expected}, got {actual}")]
    BlobIntegrity { expected: String, actual: String },

    /// An op payload named a table/column outside the Phase-1 allowlist.
    #[error("unsupported op target: {0}")]
    UnsupportedOp(String),

    /// The single-writer mutex was poisoned by a panic in another thread.
    #[error("writer lock poisoned")]
    LockPoisoned,

    /// An invariant the storage layer expected to hold was violated.
    #[error("storage invariant: {0}")]
    Invariant(String),
}

/// Convenience alias for storage-internal fallible paths.
pub type StorageResult<T> = Result<T, StorageError>;

impl From<StorageError> for AppError {
    fn from(e: StorageError) -> Self {
        match e {
            StorageError::Io(_) => AppError::TransientIo(e.to_string()),
            StorageError::Serde(_) => AppError::Serialization(e.to_string()),
            StorageError::UnsupportedOp(_) | StorageError::Invariant(_) => {
                AppError::Internal(e.to_string())
            }
            // Everything else is a durable-storage class failure.
            _ => AppError::Storage(e.to_string()),
        }
    }
}

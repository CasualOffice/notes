//! `links` error type (thiserror). `app-service` maps these into
//! `app_domain::AppError` (`ErrorClass::Storage` for DB failures) at the boundary.

use thiserror::Error;

/// Errors from link edge upsert / reconcile / query.
#[derive(Debug, Error)]
pub enum LinkError {
    /// An underlying SQLite / SQLCipher failure.
    #[error(transparent)]
    Db(#[from] rusqlite::Error),
    /// A stored id BLOB was not the expected 16 bytes.
    #[error("id blob has wrong length: {0} (expected 16)")]
    BadIdBlob(usize),
    /// A stored `link.rel` value did not match the CHECK constraint set.
    #[error("unknown link.rel value: {0}")]
    UnknownRel(String),
    /// A stored `link.origin` value was not one of the known origins.
    #[error("unknown link.origin value: {0}")]
    UnknownOrigin(String),
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, LinkError>;

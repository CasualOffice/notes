//! `notes` error type. Kept crate-local (thiserror) per the Architecture error
//! taxonomy; `app-service` maps these into `app_domain::AppError` at the command
//! boundary (`ErrorClass::Validation` for [`NotesError::Invalid`]).

use thiserror::Error;

/// Errors from doc_json validation and Markdown conversion.
#[derive(Debug, Error)]
pub enum NotesError {
    /// `doc_json` failed schema validation before persist (Data Model §4.1).
    /// `path` is a JSON-pointer-ish location; `reason` is human-readable.
    #[error("invalid doc_json at {path}: {reason}")]
    Invalid {
        /// Location of the offending node, e.g. `$.content[2]`.
        path: String,
        /// Why it is invalid.
        reason: String,
    },
    /// Malformed JSON while (de)serializing `doc_json`.
    #[error("doc_json (de)serialization failed: {0}")]
    Serde(#[from] serde_json::Error),
}

impl NotesError {
    /// Construct an [`NotesError::Invalid`].
    pub fn invalid(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

/// Crate result alias.
pub type Result<T> = std::result::Result<T, NotesError>;

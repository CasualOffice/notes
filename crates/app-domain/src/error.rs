//! Core error taxonomy. Implements HLD §10 (Error taxonomy) and Architecture §10.
//!
//! Every [`AppError`] classifies into an [`ErrorClass`] and reports a `retryable`
//! flag. Commands return `Result<T, AppError>` (HLD §6); the error serializes to a
//! stable wire shape `{ class, retryable, message }` so the WebView can react
//! (retry transient classes, surface terminal ones) and re-emit via
//! `AppEvent::Error` (HLD §7).

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize};

/// The classification axis for every error surfaced to the WebView.
///
/// Retryable classes (per HLD §10): transient IO, model-not-loaded, capture-glitch,
/// and network — these get bounded retry. All others are terminal and surface an
/// actionable UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorClass {
    /// Argument/schema validation failed at the command boundary.
    Validation,
    /// A referenced entity/row does not exist.
    NotFound,
    /// Optimistic-concurrency conflict (`notes.save` stale `base_version`).
    Conflict,
    /// Operation not permitted (capability file / scope denial).
    Permission,
    /// Platform capability genuinely absent (reported, never faked — HLD §9).
    Capability,
    /// Transient filesystem/IO hiccup. **Retryable.**
    TransientIo,
    /// Durable storage/SQLite/SQLCipher failure.
    Storage,
    /// A required model is not loaded/resident yet. **Retryable.**
    ModelNotLoaded,
    /// Recoverable audio-capture glitch. **Retryable.**
    CaptureGlitch,
    /// Speech-to-text failure (falls back to `CAPTURED`, never loses audio).
    Transcription,
    /// LLM structured-generation failure (one repair, then deterministic fallback).
    Generation,
    /// Natural-language parse failure in `app-nlp`.
    Nlp,
    /// (De)serialization failure at a JSON boundary.
    Serialization,
    /// Consented network path (model-download / updater) failure. **Retryable.**
    Network,
    /// The operation was cancelled.
    Cancelled,
    /// Unclassified internal invariant violation.
    Internal,
}

impl ErrorClass {
    /// Whether errors of this class should get bounded automatic retry (HLD §10).
    #[must_use]
    pub const fn retryable(self) -> bool {
        matches!(
            self,
            Self::TransientIo | Self::ModelNotLoaded | Self::CaptureGlitch | Self::Network
        )
    }
}

/// The unified error type returned by every command and library fallible path.
///
/// Libraries use `thiserror`; runtime paths never `unwrap()` on a fallible result
/// (CLAUDE.md conventions). Each variant maps 1:1 to an [`ErrorClass`].
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("validation error: {0}")]
    Validation(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("permission denied: {0}")]
    Permission(String),

    #[error("capability unavailable: {0}")]
    Capability(String),

    #[error("transient io error: {0}")]
    TransientIo(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("model not loaded: {0}")]
    ModelNotLoaded(String),

    #[error("capture glitch: {0}")]
    CaptureGlitch(String),

    #[error("transcription error: {0}")]
    Transcription(String),

    #[error("generation error: {0}")]
    Generation(String),

    #[error("nlp error: {0}")]
    Nlp(String),

    #[error("serialization error: {0}")]
    Serialization(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("operation cancelled")]
    Cancelled,

    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    /// The classification of this error.
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Validation(_) => ErrorClass::Validation,
            Self::NotFound(_) => ErrorClass::NotFound,
            Self::Conflict(_) => ErrorClass::Conflict,
            Self::Permission(_) => ErrorClass::Permission,
            Self::Capability(_) => ErrorClass::Capability,
            Self::TransientIo(_) => ErrorClass::TransientIo,
            Self::Storage(_) => ErrorClass::Storage,
            Self::ModelNotLoaded(_) => ErrorClass::ModelNotLoaded,
            Self::CaptureGlitch(_) => ErrorClass::CaptureGlitch,
            Self::Transcription(_) => ErrorClass::Transcription,
            Self::Generation(_) => ErrorClass::Generation,
            Self::Nlp(_) => ErrorClass::Nlp,
            Self::Serialization(_) => ErrorClass::Serialization,
            Self::Network(_) => ErrorClass::Network,
            Self::Cancelled => ErrorClass::Cancelled,
            Self::Internal(_) => ErrorClass::Internal,
        }
    }

    /// Whether this error should be retried (delegates to its class).
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.class().retryable()
    }
}

impl From<serde_json::Error> for AppError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serialization(e.to_string())
    }
}

/// Serialized as `{ "class": <snake>, "retryable": <bool>, "message": <string> }`
/// so the WebView gets a stable, typed error surface (HLD §6/§7).
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut st = s.serialize_struct("AppError", 3)?;
        st.serialize_field("class", &self.class())?;
        st.serialize_field("retryable", &self.retryable())?;
        st.serialize_field("message", &self.to_string())?;
        st.end()
    }
}

/// Convenience alias for library code.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retryable_set_matches_hld() {
        assert!(AppError::TransientIo("x".into()).retryable());
        assert!(AppError::ModelNotLoaded("x".into()).retryable());
        assert!(AppError::CaptureGlitch("x".into()).retryable());
        assert!(AppError::Network("x".into()).retryable());
        assert!(!AppError::Validation("x".into()).retryable());
        assert!(!AppError::Conflict("x".into()).retryable());
    }

    #[test]
    fn serializes_to_wire_shape() {
        let e = AppError::Conflict("stale base_version".into());
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["class"], "conflict");
        assert_eq!(v["retryable"], false);
        assert_eq!(v["message"], "conflict: stale base_version");
    }
}

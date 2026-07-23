//! Error taxonomy for `model-manager`.
//!
//! Every fallible path returns [`ModelError`] (thiserror; no `unwrap()` in
//! fallible code, per CLAUDE.md). Each variant maps onto the shared
//! [`ErrorClass`](app_domain::ErrorClass) so the network-owning service can
//! re-raise a wire-stable [`AppError`](app_domain::AppError) to the WebView
//! (Architecture §8 / §10). The mapping is intentionally conservative:
//! integrity and quota failures are **terminal** (refuse-and-report — Architecture
//! §9 "malicious model file"), while raw IO is treated as **transient**.

use std::path::PathBuf;

use app_domain::{AppError, ErrorClass};

/// The unified error for model distribution, verification, and registry ops.
#[derive(Debug, thiserror::Error)]
pub enum ModelError {
    /// Underlying filesystem IO failed (open/read/write/rename/stat).
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A manifest (or registry blob) failed to (de)serialize.
    #[error("manifest parse error: {0}")]
    ManifestParse(#[from] serde_json::Error),

    /// The manifest is structurally valid JSON but semantically invalid
    /// (e.g. malformed sha256 hex, empty id, zero size).
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),

    /// SHA-256 of the file on disk does not match the signed manifest.
    /// Terminal: refuse-and-report (Architecture §9).
    #[error("checksum mismatch: expected {expected}, computed {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    /// Byte size of the file on disk does not match the signed manifest.
    #[error("size mismatch: expected {expected} bytes, found {actual} bytes")]
    SizeMismatch { expected: u64, actual: u64 },

    /// The declared/observed size exceeds the caller's hard upper bound — a
    /// guard against unbounded/decompression-bomb downloads.
    #[error("size {size} bytes exceeds bound of {max} bytes")]
    SizeExceedsBound { size: u64, max: u64 },

    /// The manifest signature did not verify against the trusted key.
    /// Terminal: refuse-and-report (Architecture §9).
    #[error("signature verification failed: {0}")]
    SignatureInvalid(String),

    /// Preflight determined there is not enough free disk for the install.
    #[error("insufficient disk: need {required} bytes, {available} available")]
    InsufficientDisk { required: u64, available: u64 },

    /// The install would push the models directory over its configured quota.
    #[error("quota exceeded: need {required} bytes, only {remaining} remain in quota")]
    QuotaExceeded { required: u64, remaining: u64 },

    /// An offline-import source file was not found where the caller pointed.
    #[error("import source not found: {0}")]
    ImportSourceMissing(PathBuf),

    /// A model id was referenced that is not present in the registry.
    #[error("unknown model: {0}")]
    UnknownModel(String),

    /// A model id already exists in the registry (duplicate install).
    #[error("model already installed: {0}")]
    AlreadyInstalled(String),

    /// The `Downloader` backend failed (network, range unsupported, etc.).
    /// Retryable class (Architecture §8: one of two consented network owners).
    #[error("download failed: {0}")]
    Download(String),
}

impl ModelError {
    /// Map to the shared [`ErrorClass`] so callers can decide retry vs. surface.
    #[must_use]
    pub const fn class(&self) -> ErrorClass {
        match self {
            Self::Io(_) => ErrorClass::TransientIo,
            Self::Download(_) => ErrorClass::Network,
            Self::ManifestParse(_) => ErrorClass::Serialization,
            Self::InvalidManifest(_) => ErrorClass::Validation,
            Self::ImportSourceMissing(_) => ErrorClass::NotFound,
            Self::UnknownModel(_) => ErrorClass::NotFound,
            Self::AlreadyInstalled(_) => ErrorClass::Conflict,
            // Integrity / capacity failures are terminal and user-actionable.
            Self::ChecksumMismatch { .. }
            | Self::SizeMismatch { .. }
            | Self::SizeExceedsBound { .. }
            | Self::SignatureInvalid(_)
            | Self::InsufficientDisk { .. }
            | Self::QuotaExceeded { .. } => ErrorClass::Validation,
        }
    }

    /// Whether the error should get bounded automatic retry (delegates to class).
    #[must_use]
    pub const fn retryable(&self) -> bool {
        self.class().retryable()
    }
}

/// Bridge into the app-wide error surface. The network-owning service converts a
/// `ModelError` into the wire-stable [`AppError`] before it reaches the WebView.
impl From<ModelError> for AppError {
    fn from(e: ModelError) -> Self {
        let msg = e.to_string();
        match e.class() {
            ErrorClass::TransientIo => Self::TransientIo(msg),
            ErrorClass::Network => Self::Network(msg),
            ErrorClass::Serialization => Self::Serialization(msg),
            ErrorClass::NotFound => Self::NotFound(msg),
            ErrorClass::Conflict => Self::Conflict(msg),
            ErrorClass::Validation => Self::Validation(msg),
            // Remaining classes are not produced by `class()` above; map defensively.
            _ => Self::Internal(msg),
        }
    }
}

/// Convenience alias for library code.
pub type ModelResult<T> = Result<T, ModelError>;

//! On-disk directory layout. Implements Data Model §12 verbatim.
//!
//! Everything Casual Note persists lives under a single app-data root:
//!
//! ```text
//! <root>/
//! ├── casualnote.db            SQLCipher store (+ -wal / -shm)
//! ├── files/<ab>/<sha256>      content-addressed blobs (attachments + audio)
//! ├── journals/
//! │   ├── sessions/<id>.ndjson recording crash journals (later phase)
//! │   └── ops/<YYYY-MM-DD>.ndjson  rotated entity-op write-ahead
//! ├── models/                  downloaded / imported model files (later phase)
//! ├── exports/  backups/  logs/
//! ```

use std::path::{Path, PathBuf};

/// Resolves the canonical sub-paths of the app-data root (Data Model §12).
#[derive(Clone, Debug)]
pub struct Paths {
    root: PathBuf,
}

impl Paths {
    /// Root at `root`. Does not touch the filesystem; call [`Paths::ensure`].
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Create the directory skeleton (idempotent).
    pub fn ensure(&self) -> std::io::Result<()> {
        for dir in [
            self.files_dir(),
            self.ops_journal_dir(),
            self.sessions_journal_dir(),
            self.models_dir(),
            self.exports_dir(),
            self.backups_dir(),
            self.logs_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// The app-data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The main SQLCipher database file (`casualnote.db`).
    #[must_use]
    pub fn db_file(&self) -> PathBuf {
        self.root.join("casualnote.db")
    }

    /// Content-addressed blob directory (`files/`).
    #[must_use]
    pub fn files_dir(&self) -> PathBuf {
        self.root.join("files")
    }

    /// Rotated entity-op journal directory (`journals/ops/`).
    #[must_use]
    pub fn ops_journal_dir(&self) -> PathBuf {
        self.root.join("journals").join("ops")
    }

    /// Per-recording session journal directory (`journals/sessions/`).
    #[must_use]
    pub fn sessions_journal_dir(&self) -> PathBuf {
        self.root.join("journals").join("sessions")
    }

    /// Model files directory (`models/`).
    #[must_use]
    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    /// User export directory (`exports/`).
    #[must_use]
    pub fn exports_dir(&self) -> PathBuf {
        self.root.join("exports")
    }

    /// Local backup directory (`backups/`).
    #[must_use]
    pub fn backups_dir(&self) -> PathBuf {
        self.root.join("backups")
    }

    /// Local diagnostic logs directory (`logs/`).
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }
}

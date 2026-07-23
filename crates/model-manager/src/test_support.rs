//! Test-only scaffolding. Not compiled into the library.
//!
//! A tiny self-cleaning temp directory so the test suite needs no `tempfile`
//! dependency (keeping the supply chain minimal, per the crate's design rule).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// A unique temporary directory removed on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

impl TempDir {
    /// Create a fresh, uniquely-named directory under the system temp root.
    ///
    /// Uniqueness comes from pid + a process-wide atomic counter + a nanosecond
    /// timestamp, so parallel tests never collide.
    #[must_use]
    pub fn new(tag: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "casualnote-model-manager-{}-{tag}-{n}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    /// The directory path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

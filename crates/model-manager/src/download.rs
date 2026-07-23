//! The `Downloader` seam: resumable range fetch with progress (Architecture §8).
//!
//! Network I/O is owned by exactly two consented services (`model-download`,
//! `updater` — Architecture §8). This module defines the **contract** they
//! implement — [`Downloader`] — but ships only a [`MockDownloader`] that fetches
//! from a local "remote" file. The real HTTP backend (resumable `Range` requests
//! over the Tauri HTTP allowlist) is **deliberately deferred**: no `reqwest`/
//! `hyper` is pulled in this phase. When it lands it implements [`Downloader`] and
//! the rest of the pipeline (checksum → verify → registry insert) is unchanged.
//!
//! The trait models the two properties that matter for large weights:
//! *resumability* (a `kill -9` mid-download resumes from the partial file, not
//! from zero) and *progress* (byte-level callbacks feed the
//! `ModelDownloadProgress` AppEvent — HLD §7).

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::error::{ModelError, ModelResult};

/// What to fetch and the integrity bounds the caller expects.
///
/// The `Downloader` itself is *not* required to verify the checksum — that is the
/// pipeline's job after the bytes land (see [`crate::pipeline`]) — but it MUST
/// honour `max_bytes` as a hard ceiling so a lying server cannot fill the disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DownloadRequest {
    /// Artifact locator.
    pub url: String,
    /// Expected lowercase-hex SHA-256 (for the post-download verify step).
    pub expected_sha256: String,
    /// Expected exact size in bytes.
    pub expected_size: u64,
    /// Optional hard ceiling; exceeding it aborts with
    /// [`ModelError::SizeExceedsBound`].
    pub max_bytes: Option<u64>,
}

impl DownloadRequest {
    /// Construct a request from a manifest, with an optional independent ceiling.
    #[must_use]
    pub fn from_manifest(m: &crate::manifest::ModelManifest, max_bytes: Option<u64>) -> Self {
        Self {
            url: m.url.clone(),
            expected_sha256: m.sha256.clone(),
            expected_size: m.size_bytes,
            max_bytes,
        }
    }
}

/// Progress events emitted during a fetch. Feed these to the
/// `ModelDownloadProgress` AppEvent (HLD §7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DownloadEvent {
    /// Emitted once at the start: total size and the byte offset resumed from
    /// (0 for a fresh download).
    Started { total_bytes: u64, resumed_from: u64 },
    /// Emitted periodically: cumulative bytes on disk vs. total.
    Progress {
        downloaded_bytes: u64,
        total_bytes: u64,
    },
    /// Emitted once when the destination is fully written.
    Finished { downloaded_bytes: u64 },
}

/// Result of a completed fetch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DownloadOutcome {
    /// Final size of the destination file.
    pub bytes_written: u64,
    /// Whether the fetch resumed from a pre-existing partial file.
    pub resumed: bool,
}

/// A resumable, progress-reporting fetch backend.
///
/// Implementations MUST:
/// - resume from an existing partial `dest` when [`supports_resume`](Self::supports_resume)
///   is true (append from its current length; restart if it is longer than the source);
/// - honour `req.max_bytes`;
/// - emit at least a `Started` and a `Finished` event.
///
/// Implementations MUST NOT touch anything but the destination path, and MUST be
/// usable from the single network-owning service only.
pub trait Downloader {
    /// Whether this backend can resume partial downloads via range requests.
    fn supports_resume(&self) -> bool;

    /// Fetch `req.url` into `dest`, invoking `progress` as bytes arrive.
    fn download(
        &self,
        req: &DownloadRequest,
        dest: &Path,
        progress: &mut dyn FnMut(DownloadEvent),
    ) -> ModelResult<DownloadOutcome>;
}

/// A test/dev `Downloader` that copies from a registered local "remote" file.
///
/// It faithfully simulates the two behaviours the real backend must have —
/// resuming from a partial destination and streaming progress in fixed-size
/// chunks — without any network dependency, so the whole install pipeline is
/// testable offline.
#[derive(Clone, Debug)]
pub struct MockDownloader {
    /// Maps a URL to the local file standing in for the remote artifact.
    sources: HashMap<String, PathBuf>,
    /// Bytes copied per progress step.
    chunk_bytes: usize,
    /// When true, resume is honoured; when false, every fetch starts from zero.
    resumable: bool,
}

impl MockDownloader {
    /// A fresh mock with a default 64 KiB chunk and resume enabled.
    #[must_use]
    pub fn new() -> Self {
        Self {
            sources: HashMap::new(),
            chunk_bytes: 64 * 1024,
            resumable: true,
        }
    }

    /// Register a local file as the artifact served at `url`.
    #[must_use]
    pub fn with_source(mut self, url: impl Into<String>, local: impl Into<PathBuf>) -> Self {
        self.sources.insert(url.into(), local.into());
        self
    }

    /// Override the per-step chunk size (min 1).
    #[must_use]
    pub fn with_chunk_bytes(mut self, chunk_bytes: usize) -> Self {
        self.chunk_bytes = chunk_bytes.max(1);
        self
    }

    /// Toggle resume support (to exercise both trait behaviours).
    #[must_use]
    pub fn with_resumable(mut self, resumable: bool) -> Self {
        self.resumable = resumable;
        self
    }
}

impl Default for MockDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader for MockDownloader {
    fn supports_resume(&self) -> bool {
        self.resumable
    }

    fn download(
        &self,
        req: &DownloadRequest,
        dest: &Path,
        progress: &mut dyn FnMut(DownloadEvent),
    ) -> ModelResult<DownloadOutcome> {
        let source = self
            .sources
            .get(&req.url)
            .ok_or_else(|| ModelError::Download(format!("no mock source for url {}", req.url)))?;

        let mut src = std::fs::File::open(source)?;
        let total_bytes = src.metadata()?.len();

        if let Some(max) = req.max_bytes {
            if total_bytes > max {
                return Err(ModelError::SizeExceedsBound {
                    size: total_bytes,
                    max,
                });
            }
        }

        // Determine the resume offset from any pre-existing partial destination.
        let existing = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
        let mut resumed_from = if self.resumable { existing } else { 0 };
        // A partial longer than the source is corrupt — restart from zero.
        if resumed_from > total_bytes {
            resumed_from = 0;
        }
        let resumed = resumed_from > 0;

        // Open the destination: append when resuming, truncate otherwise.
        let mut out = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(!resumed)
            .open(dest)?;
        if resumed {
            out.seek(SeekFrom::Start(resumed_from))?;
            src.seek(SeekFrom::Start(resumed_from))?;
        }

        progress(DownloadEvent::Started {
            total_bytes,
            resumed_from,
        });

        let mut written = resumed_from;
        let mut buf = vec![0u8; self.chunk_bytes];
        loop {
            let n = src.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            written += n as u64;
            progress(DownloadEvent::Progress {
                downloaded_bytes: written,
                total_bytes,
            });
        }
        out.flush()?;

        progress(DownloadEvent::Finished {
            downloaded_bytes: written,
        });

        Ok(DownloadOutcome {
            bytes_written: written,
            resumed,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::io::Write;

    fn seed_source(dir: &TempDir, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.path().join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    fn req(url: &str, size: u64) -> DownloadRequest {
        DownloadRequest {
            url: url.into(),
            expected_sha256: String::new(),
            expected_size: size,
            max_bytes: None,
        }
    }

    #[test]
    fn downloads_full_file_with_progress() {
        let dir = TempDir::new("dl_full");
        let payload = vec![7u8; 1000];
        let src = seed_source(&dir, "remote.bin", &payload);
        let dl = MockDownloader::new()
            .with_chunk_bytes(256)
            .with_source("http://x/model", &src);

        let dest = dir.path().join("model.bin");
        let mut events = Vec::new();
        let outcome = dl
            .download(&req("http://x/model", 1000), &dest, &mut |e| events.push(e))
            .unwrap();

        assert_eq!(outcome.bytes_written, 1000);
        assert!(!outcome.resumed);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
        assert!(matches!(
            events.first(),
            Some(DownloadEvent::Started { .. })
        ));
        assert!(matches!(
            events.last(),
            Some(DownloadEvent::Finished { .. })
        ));
    }

    #[test]
    fn resumes_from_partial_file() {
        let dir = TempDir::new("dl_resume");
        let payload: Vec<u8> = (0..1000u32).map(|i| i as u8).collect();
        let src = seed_source(&dir, "remote.bin", &payload);
        let dl = MockDownloader::new()
            .with_chunk_bytes(128)
            .with_source("http://x/model", &src);

        // Pre-seed a partial destination with the correct first 400 bytes.
        let dest = dir.path().join("model.bin");
        std::fs::write(&dest, &payload[..400]).unwrap();

        let mut started: Option<DownloadEvent> = None;
        let outcome = dl
            .download(&req("http://x/model", 1000), &dest, &mut |e| {
                if let DownloadEvent::Started { .. } = e {
                    started = Some(e);
                }
            })
            .unwrap();

        assert!(outcome.resumed);
        assert_eq!(outcome.bytes_written, 1000);
        assert_eq!(
            started,
            Some(DownloadEvent::Started {
                total_bytes: 1000,
                resumed_from: 400
            })
        );
        // The reassembled file must equal the source bit-for-bit.
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
    }

    #[test]
    fn non_resumable_restarts() {
        let dir = TempDir::new("dl_noresume");
        let payload = vec![3u8; 500];
        let src = seed_source(&dir, "remote.bin", &payload);
        let dl = MockDownloader::new()
            .with_resumable(false)
            .with_source("http://x/model", &src);
        assert!(!dl.supports_resume());

        let dest = dir.path().join("model.bin");
        std::fs::write(&dest, vec![9u8; 200]).unwrap();
        let outcome = dl
            .download(&req("http://x/model", 500), &dest, &mut |_| {})
            .unwrap();
        assert!(!outcome.resumed);
        assert_eq!(std::fs::read(&dest).unwrap(), payload);
    }

    #[test]
    fn unknown_url_errors() {
        let dir = TempDir::new("dl_unknown");
        let dl = MockDownloader::new();
        let dest = dir.path().join("model.bin");
        let err = dl
            .download(&req("http://x/missing", 1), &dest, &mut |_| {})
            .unwrap_err();
        assert!(matches!(err, ModelError::Download(_)));
    }

    #[test]
    fn enforces_max_bytes() {
        let dir = TempDir::new("dl_bound");
        let src = seed_source(&dir, "remote.bin", &vec![0u8; 1000]);
        let dl = MockDownloader::new().with_source("http://x/model", &src);
        let dest = dir.path().join("model.bin");
        let mut r = req("http://x/model", 1000);
        r.max_bytes = Some(500);
        let err = dl.download(&r, &dest, &mut |_| {}).unwrap_err();
        assert!(matches!(err, ModelError::SizeExceedsBound { .. }));
    }
}

//! Content-addressed blob store. Implements Data Model §4.5 / §8.2 / §12:
//! attachment and captured-audio bytes live under `files/<sha256[0:2]>/<sha256>`,
//! named by their SHA-256 content address. De-dup is automatic (same bytes → same
//! name → one file), and reads verify integrity against the address.
//!
//! The store owns only the bytes-on-disk; the `attachment` / `audio_track` rows
//! that reference a hash are written through the normal op path.

use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{StorageError, StorageResult};

/// A lowercase-hex SHA-256 content address (64 chars).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Sha256Hex(pub String);

impl Sha256Hex {
    /// Compute the address of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut h = Sha256::new();
        h.update(bytes);
        let digest = h.finalize();
        let mut s = String::with_capacity(64);
        for b in digest {
            s.push_str(&format!("{b:02x}"));
        }
        Self(s)
    }

    /// The hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for Sha256Hex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A content-addressed blob directory (`files/`).
#[derive(Clone, Debug)]
pub struct BlobStore {
    root: PathBuf,
}

impl BlobStore {
    /// A store rooted at `files_dir`. Creates the directory.
    pub fn new(files_dir: impl Into<PathBuf>) -> StorageResult<Self> {
        let root = files_dir.into();
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// The on-disk path for `hash` (`<root>/<ab>/<hash>`), whether or not it exists.
    #[must_use]
    pub fn path_for(&self, hash: &Sha256Hex) -> PathBuf {
        let shard = &hash.0[0..2];
        self.root.join(shard).join(&hash.0)
    }

    /// Whether a blob with this address already exists.
    #[must_use]
    pub fn exists(&self, hash: &Sha256Hex) -> bool {
        self.path_for(hash).is_file()
    }

    /// Store `bytes`, returning their content address. Idempotent: if the blob
    /// already exists the bytes are not rewritten (dedup). The write is atomic
    /// (temp file + rename) so a crash never leaves a partial addressed file.
    pub fn put(&self, bytes: &[u8]) -> StorageResult<Sha256Hex> {
        let hash = Sha256Hex::of(bytes);
        let dst = self.path_for(&hash);
        if dst.is_file() {
            return Ok(hash);
        }
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Write to a unique temp file in the same shard dir, fsync, then rename.
        let tmp = dst.with_extension("tmp-partial");
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.flush()?;
            f.sync_all()?;
        }
        std::fs::rename(&tmp, &dst)?;
        Ok(hash)
    }

    /// Read a blob, verifying it hashes back to its address. A mismatch is a
    /// corruption error rather than silently-returned bad bytes.
    pub fn get(&self, hash: &Sha256Hex) -> StorageResult<Vec<u8>> {
        let bytes = std::fs::read(self.path_for(hash))?;
        let actual = Sha256Hex::of(&bytes);
        if &actual != hash {
            return Err(StorageError::BlobIntegrity {
                expected: hash.0.clone(),
                actual: actual.0,
            });
        }
        Ok(bytes)
    }

    /// The root `files/` directory.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).unwrap();
        let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
        std::env::temp_dir().join(format!("cn-blobs-{name}"))
    }

    #[test]
    fn put_get_dedup_and_shard() {
        let dir = temp_dir();
        let store = BlobStore::new(&dir).unwrap();

        let a = store.put(b"hello world").unwrap();
        let b = store.put(b"hello world").unwrap();
        assert_eq!(a, b, "same bytes → same address");
        // sharded path uses the first two hex chars as the directory
        let shard = store.path_for(&a);
        let shard_dir = shard
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(shard_dir, &a.0[0..2]);

        assert_eq!(store.get(&a).unwrap(), b"hello world");

        let other = store.put(b"different").unwrap();
        assert_ne!(a, other);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn known_sha256_vector() {
        // SHA-256("abc")
        assert_eq!(
            Sha256Hex::of(b"abc").as_str(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}

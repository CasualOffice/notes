//! SHA-256 integrity + size-bound verification (Architecture §8/§9).
//!
//! Before any downloaded or USB-imported artifact is admitted to the registry it
//! is verified against its signed manifest: exact byte size and lowercase-hex
//! SHA-256 must match. A mismatch is terminal — refuse-and-report, never load
//! (Architecture §9 "malicious model file"). A separate hard upper bound guards
//! against unbounded / decompression-bomb inputs before hashing.

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::{ModelError, ModelResult};
use crate::manifest::ModelManifest;

/// Read buffer size for streaming hash (avoids loading multi-GB files into RAM).
const HASH_BUF_BYTES: usize = 1 << 20; // 1 MiB

/// Stream a reader through SHA-256, returning the lowercase-hex digest and the
/// number of bytes consumed.
pub fn sha256_hex_reader<R: Read>(mut r: R) -> ModelResult<(String, u64)> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUF_BYTES];
    let mut total: u64 = 0;
    loop {
        let n = r.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        total += n as u64;
    }
    Ok((hex::encode(hasher.finalize()), total))
}

/// SHA-256 a file on disk, returning `(hex_digest, byte_len)`.
pub fn sha256_hex_file(path: &Path) -> ModelResult<(String, u64)> {
    let f = File::open(path)?;
    sha256_hex_reader(f)
}

/// Verify a file against an expected digest and size.
///
/// Order matters: the cheap size check runs first (and doubles as the bound
/// guard when `max_bytes` is supplied) so a wrong-length file is rejected without
/// hashing gigabytes. Then the SHA-256 must match exactly.
pub fn verify_file(
    path: &Path,
    expected_sha256: &str,
    expected_size: u64,
    max_bytes: Option<u64>,
) -> ModelResult<()> {
    let meta = std::fs::metadata(path)?;
    let actual_size = meta.len();

    if let Some(max) = max_bytes {
        if actual_size > max {
            return Err(ModelError::SizeExceedsBound {
                size: actual_size,
                max,
            });
        }
    }
    if actual_size != expected_size {
        return Err(ModelError::SizeMismatch {
            expected: expected_size,
            actual: actual_size,
        });
    }

    let (digest, _) = sha256_hex_file(path)?;
    // Constant-time-ish compare is unnecessary here (public integrity hash, not a
    // secret), but normalize case defensively.
    if !digest.eq_ignore_ascii_case(expected_sha256) {
        return Err(ModelError::ChecksumMismatch {
            expected: expected_sha256.to_ascii_lowercase(),
            actual: digest,
        });
    }
    Ok(())
}

/// Verify a file directly against its [`ModelManifest`] (size + digest).
///
/// `max_bytes` is an optional caller-supplied hard ceiling independent of the
/// manifest's declared size (defense in depth).
pub fn verify_against_manifest(
    path: &Path,
    manifest: &ModelManifest,
    max_bytes: Option<u64>,
) -> ModelResult<()> {
    verify_file(path, &manifest.sha256, manifest.size_bytes, max_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::io::Write;

    /// SHA-256 of the empty input — the canonical test vector.
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn hashes_known_vector() {
        let (digest, len) = sha256_hex_reader(&b"abc"[..]).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(len, 3);
    }

    #[test]
    fn empty_file_matches_empty_vector() {
        let dir = TempDir::new("checksum_empty");
        let p = dir.path().join("empty.bin");
        File::create(&p).unwrap();
        let (digest, len) = sha256_hex_file(&p).unwrap();
        assert_eq!(digest, EMPTY_SHA256);
        assert_eq!(len, 0);
    }

    fn write_bytes(dir: &TempDir, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.path().join(name);
        let mut f = File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        p
    }

    #[test]
    fn verify_passes_on_match() {
        let dir = TempDir::new("checksum_ok");
        let p = write_bytes(&dir, "m.bin", b"abc");
        let sha = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        verify_file(&p, sha, 3, None).unwrap();
    }

    #[test]
    fn verify_fails_on_checksum_mismatch() {
        let dir = TempDir::new("checksum_bad");
        let p = write_bytes(&dir, "m.bin", b"abc");
        // Right size, wrong digest.
        let err = verify_file(&p, EMPTY_SHA256, 3, None).unwrap_err();
        assert!(matches!(err, ModelError::ChecksumMismatch { .. }));
    }

    #[test]
    fn verify_fails_on_size_mismatch() {
        let dir = TempDir::new("checksum_size");
        let p = write_bytes(&dir, "m.bin", b"abcd");
        let err = verify_file(&p, EMPTY_SHA256, 3, None).unwrap_err();
        assert!(matches!(
            err,
            ModelError::SizeMismatch {
                expected: 3,
                actual: 4
            }
        ));
    }

    #[test]
    fn verify_fails_on_bound() {
        let dir = TempDir::new("checksum_bound");
        let p = write_bytes(&dir, "m.bin", b"abcdef");
        let err = verify_file(&p, EMPTY_SHA256, 6, Some(4)).unwrap_err();
        assert!(matches!(
            err,
            ModelError::SizeExceedsBound { size: 6, max: 4 }
        ));
    }
}

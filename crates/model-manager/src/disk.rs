//! Disk-space preflight and quota accounting (Architecture §8: "disk-space
//! preflight before write").
//!
//! Before committing to a download or import the manager checks two independent
//! ceilings:
//!
//! 1. **Free disk** — the artifact plus its working overhead (temp/resume file,
//!    scratch) must fit on the target volume, with a safety headroom margin so the
//!    volume is never driven to exactly full.
//! 2. **Quota** — an optional user/app cap on total bytes the `models/` directory
//!    may occupy, independent of the physical volume.
//!
//! ## The free-space probe is an OS seam
//! The Rust standard library exposes **no stable** free-space API, and pulling a
//! platform crate (`sysinfo`, `nix`, winapi) is out of scope for this dependency-
//! light phase. So free space is supplied by the caller via [`DiskBudget`], and the
//! *arithmetic* — the part with the interesting invariants — is verified here. The
//! real probe (`statvfs` / `GetDiskFreeSpaceEx`) lands with the OS adapter.
//! [`estimate_dir_size`] is provided as a std-only helper to measure current
//! `models/` usage for the quota side.

use std::path::Path;

use crate::error::{ModelError, ModelResult};
use crate::manifest::ModelManifest;

/// Default safety headroom applied on top of the raw requirement: keep at least
/// this fraction of the requested bytes free *after* the write. 5% guards against
/// filesystem metadata growth and rounding without being wasteful.
pub const DEFAULT_HEADROOM_NUM: u64 = 5;
/// Denominator for [`DEFAULT_HEADROOM_NUM`].
pub const DEFAULT_HEADROOM_DEN: u64 = 100;

/// A snapshot of the space situation the preflight reasons over.
///
/// `available_bytes` is the free space on the target volume (probed by the OS
/// adapter). `quota_bytes`/`used_bytes` describe the optional `models/` cap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DiskBudget {
    /// Free bytes on the volume that hosts `models/` (OS-probed).
    pub available_bytes: u64,
    /// Optional hard cap on total `models/` bytes. `None` = physical volume only.
    pub quota_bytes: Option<u64>,
    /// Bytes the `models/` directory currently occupies (counts toward quota).
    pub used_bytes: u64,
}

impl DiskBudget {
    /// A budget with only a physical free-space figure and no quota.
    #[must_use]
    pub const fn unlimited_quota(available_bytes: u64) -> Self {
        Self {
            available_bytes,
            quota_bytes: None,
            used_bytes: 0,
        }
    }

    /// Remaining quota headroom, or `None` when no quota is configured.
    #[must_use]
    pub fn quota_remaining(&self) -> Option<u64> {
        self.quota_bytes.map(|q| q.saturating_sub(self.used_bytes))
    }
}

/// The bytes an install requires, split into artifact and working overhead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpaceEstimate {
    /// Raw artifact size (the final file).
    pub artifact_bytes: u64,
    /// Transient working bytes needed *during* install (resume/temp/scratch).
    pub overhead_bytes: u64,
}

impl SpaceEstimate {
    /// Peak bytes that must be free at once. Both the final file and its temp
    /// working copy can coexist mid-install, so the peak is their sum.
    #[must_use]
    pub const fn peak_bytes(&self) -> u64 {
        self.artifact_bytes.saturating_add(self.overhead_bytes)
    }

    /// Bytes that persist after install completes (what counts toward quota).
    #[must_use]
    pub const fn resident_bytes(&self) -> u64 {
        self.artifact_bytes
    }

    /// Derive an estimate from a manifest, taking the working overhead from the
    /// manifest's declared `disk_overhead_bytes` (defaulting to 0 when unset).
    #[must_use]
    pub fn from_manifest(m: &ModelManifest) -> Self {
        Self {
            artifact_bytes: m.size_bytes,
            overhead_bytes: m.min_hardware.disk_overhead_bytes,
        }
    }
}

/// Add the default headroom margin to a raw byte requirement.
#[must_use]
fn with_headroom(bytes: u64) -> u64 {
    let margin = bytes / DEFAULT_HEADROOM_DEN * DEFAULT_HEADROOM_NUM;
    bytes.saturating_add(margin)
}

/// Preflight an install against a [`DiskBudget`].
///
/// Fails with [`ModelError::InsufficientDisk`] if peak-plus-headroom exceeds free
/// space, or [`ModelError::QuotaExceeded`] if the resident bytes would push
/// `models/` over its quota. Both ceilings are checked; the physical one first.
pub fn preflight(estimate: &SpaceEstimate, budget: &DiskBudget) -> ModelResult<()> {
    let peak = with_headroom(estimate.peak_bytes());
    if peak > budget.available_bytes {
        return Err(ModelError::InsufficientDisk {
            required: peak,
            available: budget.available_bytes,
        });
    }

    if let Some(remaining) = budget.quota_remaining() {
        let resident = estimate.resident_bytes();
        if resident > remaining {
            return Err(ModelError::QuotaExceeded {
                required: resident,
                remaining,
            });
        }
    }

    Ok(())
}

/// Std-only best-effort sum of regular-file sizes under `dir` (recursive).
///
/// Used to compute [`DiskBudget::used_bytes`] for the quota side. Symlinks are not
/// followed. A missing directory yields `Ok(0)` (nothing installed yet).
pub fn estimate_dir_size(dir: &Path) -> ModelResult<u64> {
    if !dir.exists() {
        return Ok(0);
    }
    let mut total: u64 = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let ft = entry.file_type()?;
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                total = total.saturating_add(entry.metadata()?.len());
            }
        }
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::TempDir;
    use std::fs::{self, File};
    use std::io::Write;

    #[test]
    fn passes_when_space_and_quota_ok() {
        let est = SpaceEstimate {
            artifact_bytes: 1000,
            overhead_bytes: 200,
        };
        let budget = DiskBudget {
            available_bytes: 10_000,
            quota_bytes: Some(5_000),
            used_bytes: 1_000,
        };
        preflight(&est, &budget).unwrap();
    }

    #[test]
    fn fails_when_disk_too_small() {
        let est = SpaceEstimate {
            artifact_bytes: 1000,
            overhead_bytes: 200,
        };
        // peak = 1200, +5% headroom = 1260; available 1250 < 1260.
        let budget = DiskBudget::unlimited_quota(1_250);
        let err = preflight(&est, &budget).unwrap_err();
        assert!(matches!(err, ModelError::InsufficientDisk { .. }));
    }

    #[test]
    fn fails_when_over_quota() {
        let est = SpaceEstimate {
            artifact_bytes: 4_000,
            overhead_bytes: 0,
        };
        let budget = DiskBudget {
            available_bytes: 1_000_000,
            quota_bytes: Some(5_000),
            used_bytes: 2_000, // remaining 3_000 < resident 4_000
        };
        let err = preflight(&est, &budget).unwrap_err();
        assert!(matches!(
            err,
            ModelError::QuotaExceeded {
                required: 4_000,
                remaining: 3_000
            }
        ));
    }

    #[test]
    fn no_quota_ignores_quota_ceiling() {
        let est = SpaceEstimate {
            artifact_bytes: 1_000_000,
            overhead_bytes: 0,
        };
        let budget = DiskBudget::unlimited_quota(2_000_000);
        preflight(&est, &budget).unwrap();
    }

    #[test]
    fn quota_remaining_saturates() {
        let budget = DiskBudget {
            available_bytes: 0,
            quota_bytes: Some(100),
            used_bytes: 250,
        };
        assert_eq!(budget.quota_remaining(), Some(0));
    }

    #[test]
    fn dir_size_sums_files_recursively() {
        let dir = TempDir::new("dirsize");
        let root = dir.path();
        let mut f = File::create(root.join("a.bin")).unwrap();
        f.write_all(&[0u8; 100]).unwrap();
        let sub = root.join("sub");
        fs::create_dir(&sub).unwrap();
        let mut g = File::create(sub.join("b.bin")).unwrap();
        g.write_all(&[0u8; 250]).unwrap();
        assert_eq!(estimate_dir_size(root).unwrap(), 350);
    }

    #[test]
    fn dir_size_missing_is_zero() {
        assert_eq!(
            estimate_dir_size(Path::new("/no/such/models/dir/xyz")).unwrap(),
            0
        );
    }
}

//! Install orchestration: offline USB import and network download (Architecture §8).
//!
//! Both entry points funnel through the same admission gauntlet before a model is
//! written to `models/` and recorded in the [`ModelRegistry`]:
//!
//! 1. **manifest signature** verified ([`SignatureVerifier`]);
//! 2. **disk/quota preflight** (when a [`DiskBudget`] is supplied);
//! 3. bytes obtained (copied from the USB source, or fetched via a [`Downloader`]);
//! 4. **SHA-256 + size** verified against the manifest — mismatch is terminal,
//!    refuse-and-report, and the just-written file is removed (Architecture §9);
//! 5. a [`ModelInstallation`] row inserted into the registry.
//!
//! The download path differs from the import path only in step 3, which is exactly
//! the [`Downloader`] seam — so swapping the mock for the real HTTP backend needs
//! no change here.

use std::path::{Path, PathBuf};

use app_domain::Timestamp;

use crate::checksum::verify_against_manifest;
use crate::disk::{preflight, DiskBudget, SpaceEstimate};
use crate::download::{DownloadEvent, DownloadRequest, Downloader};
use crate::error::{ModelError, ModelResult};
use crate::manifest::{ModelManifest, SignatureVerifier};
use crate::registry::{InstallSource, ModelInstallation, ModelRegistry};

/// Where and how to place an installed artifact.
#[derive(Clone, Debug)]
pub struct InstallConfig {
    /// The `models/` root directory.
    pub models_dir: PathBuf,
    /// Artifact location relative to `models_dir`. `None` derives a default from
    /// the manifest ([`default_rel_path`]).
    pub rel_path: Option<PathBuf>,
    /// Optional hard byte ceiling, independent of the manifest size.
    pub max_bytes: Option<u64>,
    /// Optional disk/quota budget to preflight against. `None` skips the space
    /// check (e.g. when the caller has already done it).
    pub budget: Option<DiskBudget>,
}

impl InstallConfig {
    /// A config for `models_dir` with all optional guards disabled.
    #[must_use]
    pub fn new(models_dir: impl Into<PathBuf>) -> Self {
        Self {
            models_dir: models_dir.into(),
            rel_path: None,
            max_bytes: None,
            budget: None,
        }
    }

    /// Set a hard byte ceiling.
    #[must_use]
    pub fn with_max_bytes(mut self, max: u64) -> Self {
        self.max_bytes = Some(max);
        self
    }

    /// Set the disk/quota budget to preflight against.
    #[must_use]
    pub fn with_budget(mut self, budget: DiskBudget) -> Self {
        self.budget = Some(budget);
        self
    }

    /// Set an explicit relative destination path.
    #[must_use]
    pub fn with_rel_path(mut self, rel: impl Into<PathBuf>) -> Self {
        self.rel_path = Some(rel.into());
        self
    }

    fn resolved_rel(&self, manifest: &ModelManifest) -> PathBuf {
        self.rel_path
            .clone()
            .unwrap_or_else(|| default_rel_path(manifest))
    }
}

/// The default on-disk layout for an artifact: `<engine>/<id>` under `models/`.
/// The record's `file_sha256` is the true content address; this is a stable,
/// human-legible path.
#[must_use]
pub fn default_rel_path(manifest: &ModelManifest) -> PathBuf {
    let engine = match manifest.engine {
        crate::manifest::ModelEngine::Stt => "stt",
        crate::manifest::ModelEngine::Llm => "llm",
        crate::manifest::ModelEngine::Embedder => "embedder",
        crate::manifest::ModelEngine::Reranker => "reranker",
    };
    PathBuf::from(engine).join(&manifest.id.0)
}

/// Verify signature and, if a budget is present, preflight disk/quota.
fn admit_pre_bytes(
    manifest: &ModelManifest,
    verifier: &dyn SignatureVerifier,
    config: &InstallConfig,
) -> ModelResult<()> {
    verifier.verify(manifest)?;
    if let Some(budget) = &config.budget {
        let estimate = SpaceEstimate::from_manifest(manifest);
        preflight(&estimate, budget)?;
    }
    Ok(())
}

/// Verify the written bytes against the manifest; on mismatch remove the file and
/// propagate the terminal error (refuse-and-report — Architecture §9).
fn verify_or_purge(
    abs_path: &Path,
    manifest: &ModelManifest,
    max_bytes: Option<u64>,
) -> ModelResult<()> {
    match verify_against_manifest(abs_path, manifest, max_bytes) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Best-effort cleanup; the verification error is what the caller sees.
            let _ = std::fs::remove_file(abs_path);
            Err(e)
        }
    }
}

/// Insert the admitted install into the registry and return the stored record.
fn record(
    manifest: &ModelManifest,
    rel_path: PathBuf,
    source: InstallSource,
    now: Timestamp,
    registry: &mut ModelRegistry,
) -> ModelResult<ModelInstallation> {
    let install = ModelInstallation::from_manifest(manifest, rel_path, source, now);
    registry.insert(install.clone())?;
    Ok(install)
}

/// Offline import: admit a local artifact (USB/local file) carried alongside its
/// signed manifest, with no network access.
///
/// `source_file` is copied into `config.models_dir` at the resolved relative path;
/// the copy is then verified against the manifest.
pub fn import_local(
    manifest: &ModelManifest,
    source_file: &Path,
    verifier: &dyn SignatureVerifier,
    config: &InstallConfig,
    now: Timestamp,
    registry: &mut ModelRegistry,
) -> ModelResult<ModelInstallation> {
    manifest.validate()?;
    if registry.contains(&manifest.id) {
        return Err(ModelError::AlreadyInstalled(manifest.id.0.clone()));
    }
    if !source_file.exists() {
        return Err(ModelError::ImportSourceMissing(source_file.to_path_buf()));
    }

    admit_pre_bytes(manifest, verifier, config)?;

    // Fail fast: verify the source bytes before copying gigabytes into place.
    verify_against_manifest(source_file, manifest, config.max_bytes)?;

    let rel = config.resolved_rel(manifest);
    let abs = config.models_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::copy(source_file, &abs)?;

    // Re-verify at the destination (guards against a copy that silently truncated).
    verify_or_purge(&abs, manifest, config.max_bytes)?;

    record(manifest, rel, InstallSource::UsbImport, now, registry)
}

/// Network install: fetch the artifact via a [`Downloader`], verify, and record.
///
/// The `progress` callback receives byte-level [`DownloadEvent`]s (feed the
/// `ModelDownloadProgress` AppEvent — HLD §7). This is one of only two consented
/// network paths (Architecture §8); the `Downloader` is the sole network actor.
pub fn install_from_download(
    manifest: &ModelManifest,
    downloader: &dyn Downloader,
    verifier: &dyn SignatureVerifier,
    config: &InstallConfig,
    now: Timestamp,
    registry: &mut ModelRegistry,
    progress: &mut dyn FnMut(DownloadEvent),
) -> ModelResult<ModelInstallation> {
    manifest.validate()?;
    if registry.contains(&manifest.id) {
        return Err(ModelError::AlreadyInstalled(manifest.id.0.clone()));
    }

    admit_pre_bytes(manifest, verifier, config)?;

    let rel = config.resolved_rel(manifest);
    let abs = config.models_dir.join(&rel);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let req = DownloadRequest::from_manifest(manifest, config.max_bytes);
    downloader.download(&req, &abs, progress)?;

    verify_or_purge(&abs, manifest, config.max_bytes)?;

    record(manifest, rel, InstallSource::Download, now, registry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checksum::sha256_hex_file;
    use crate::download::MockDownloader;
    use crate::manifest::{HardwareRequirements, ModelEngine, TrustAllVerifier};
    use crate::test_support::TempDir;
    use app_domain::ModelId;
    use std::io::Write;

    /// Write `bytes` to a temp file and build a manifest whose sha256/size match.
    fn seed_artifact(dir: &TempDir, name: &str, bytes: &[u8]) -> (PathBuf, ModelManifest) {
        let p = dir.path().join(name);
        let mut f = std::fs::File::create(&p).unwrap();
        f.write_all(bytes).unwrap();
        let (sha, len) = sha256_hex_file(&p).unwrap();
        let manifest = ModelManifest {
            id: ModelId::new("qwen3-8b-q4_k_m"),
            engine: ModelEngine::Llm,
            family: "qwen3".into(),
            variant: "8b".into(),
            version: "1.0.0".into(),
            quantization: Some("Q4_K_M".into()),
            sha256: sha,
            size_bytes: len,
            url: "http://x/model".into(),
            min_hardware: HardwareRequirements::none(),
            signature: "sig".into(),
        };
        (p, manifest)
    }

    #[test]
    fn offline_import_happy_path() {
        let dir = TempDir::new("import_ok");
        let (src, manifest) = seed_artifact(&dir, "usb.bin", b"model-weights-bytes");
        let models = dir.path().join("models");
        let mut reg = ModelRegistry::new();

        let config = InstallConfig::new(&models);
        let install = import_local(
            &manifest,
            &src,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(123),
            &mut reg,
        )
        .unwrap();

        assert_eq!(install.source, InstallSource::UsbImport);
        assert_eq!(
            install.file_path,
            PathBuf::from("llm").join("qwen3-8b-q4_k_m")
        );
        assert!(models.join(&install.file_path).exists());
        assert_eq!(reg.len(), 1);
        assert_eq!(install.installed_at, Timestamp::from_millis(123));
    }

    #[test]
    fn import_missing_source_errors() {
        let dir = TempDir::new("import_missing");
        let (_src, manifest) = seed_artifact(&dir, "usb.bin", b"bytes");
        let mut reg = ModelRegistry::new();
        let config = InstallConfig::new(dir.path().join("models"));
        let err = import_local(
            &manifest,
            &dir.path().join("does-not-exist.bin"),
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(0),
            &mut reg,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::ImportSourceMissing(_)));
    }

    #[test]
    fn import_checksum_mismatch_refuses_and_leaves_nothing() {
        let dir = TempDir::new("import_bad");
        let (src, mut manifest) = seed_artifact(&dir, "usb.bin", b"real-bytes");
        // Corrupt the expected digest (still valid hex shape).
        manifest.sha256 = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into();
        let models = dir.path().join("models");
        let mut reg = ModelRegistry::new();
        let config = InstallConfig::new(&models);

        let err = import_local(
            &manifest,
            &src,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(0),
            &mut reg,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::ChecksumMismatch { .. }));
        // Nothing admitted to the registry on refusal.
        assert!(reg.is_empty());
    }

    #[test]
    fn download_install_happy_path() {
        let dir = TempDir::new("dl_install");
        // The "remote" artifact lives on local disk; the mock serves it.
        let (remote, manifest) = seed_artifact(&dir, "remote.bin", b"downloaded-weights");
        let downloader = MockDownloader::new()
            .with_chunk_bytes(4)
            .with_source("http://x/model", &remote);
        let models = dir.path().join("models");
        let mut reg = ModelRegistry::new();
        let config = InstallConfig::new(&models);

        let mut events = Vec::new();
        let install = install_from_download(
            &manifest,
            &downloader,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(9),
            &mut reg,
            &mut |e| events.push(e),
        )
        .unwrap();

        assert_eq!(install.source, InstallSource::Download);
        assert!(models.join(&install.file_path).exists());
        assert_eq!(reg.len(), 1);
        assert!(events
            .iter()
            .any(|e| matches!(e, DownloadEvent::Finished { .. })));
    }

    #[test]
    fn download_size_bound_blocks_before_admission() {
        let dir = TempDir::new("dl_bound");
        let (remote, manifest) = seed_artifact(&dir, "remote.bin", &vec![1u8; 1000]);
        let downloader = MockDownloader::new().with_source("http://x/model", &remote);
        let models = dir.path().join("models");
        let mut reg = ModelRegistry::new();
        let config = InstallConfig::new(&models).with_max_bytes(100);

        let err = install_from_download(
            &manifest,
            &downloader,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(0),
            &mut reg,
            &mut |_| {},
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::SizeExceedsBound { .. }));
        assert!(reg.is_empty());
    }

    #[test]
    fn preflight_budget_blocks_install() {
        let dir = TempDir::new("dl_preflight");
        let (src, manifest) = seed_artifact(&dir, "usb.bin", &vec![0u8; 10_000]);
        let models = dir.path().join("models");
        let mut reg = ModelRegistry::new();
        // Free space far below the artifact size.
        let budget = DiskBudget::unlimited_quota(100);
        let config = InstallConfig::new(&models).with_budget(budget);

        let err = import_local(
            &manifest,
            &src,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(0),
            &mut reg,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::InsufficientDisk { .. }));
        assert!(reg.is_empty());
    }

    #[test]
    fn duplicate_install_rejected() {
        let dir = TempDir::new("dup");
        let (src, manifest) = seed_artifact(&dir, "usb.bin", b"bytes");
        let models = dir.path().join("models");
        let mut reg = ModelRegistry::new();
        let config = InstallConfig::new(&models);
        import_local(
            &manifest,
            &src,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(0),
            &mut reg,
        )
        .unwrap();
        let err = import_local(
            &manifest,
            &src,
            &TrustAllVerifier,
            &config,
            Timestamp::from_millis(0),
            &mut reg,
        )
        .unwrap_err();
        assert!(matches!(err, ModelError::AlreadyInstalled(_)));
    }
}

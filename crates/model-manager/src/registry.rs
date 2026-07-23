//! In-memory registry of installed models (Data Model §9.4 `model_installation`).
//!
//! [`ModelInstallation`] mirrors the `model_installation` row 1:1 (id, role,
//! family, variant, quant, `file_sha256`, `manifest_sig`, `file_path`, byte_size,
//! source, installed_at, is_active). Durable persistence is the `storage` crate's
//! job — this crate owns the *shape* and the in-memory invariants (unique id,
//! at most one active model per role), so the install/import pipeline can be
//! exercised and tested without a database.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use app_domain::{ModelId, Timestamp};

use crate::error::{ModelError, ModelResult};
use crate::manifest::{ModelEngine, ModelManifest};

/// How a model entered the registry. Mirrors `model_installation.source`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallSource {
    /// Fetched over the network by the consented `model-download` service.
    Download,
    /// Imported from a local file / USB drive (offline path).
    UsbImport,
}

/// One installed model — the on-device registry record (Data Model §9.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInstallation {
    /// Registry id (matches the manifest's `id`).
    pub id: ModelId,
    /// Role the model serves.
    pub role: ModelEngine,
    /// Model family (whisper|qwen3|...).
    pub family: String,
    /// Variant (base|8b|300m|...).
    pub variant: String,
    /// Quantization label, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quant: Option<String>,
    /// Verified lowercase-hex SHA-256 of the artifact (from the signed manifest).
    pub file_sha256: String,
    /// The manifest signature this install was admitted under.
    pub manifest_sig: String,
    /// Artifact location, relative to the `models/` root.
    pub file_path: PathBuf,
    /// Artifact size in bytes.
    pub byte_size: u64,
    /// Provenance.
    pub source: InstallSource,
    /// When the install completed.
    pub installed_at: Timestamp,
    /// Whether this model is the active selection for its role.
    pub is_active: bool,
}

impl ModelInstallation {
    /// Build a record from a verified manifest and a chosen relative path.
    ///
    /// The caller is responsible for having verified the bytes (checksum) and the
    /// signature *before* constructing this — the record asserts admission.
    #[must_use]
    pub fn from_manifest(
        manifest: &ModelManifest,
        file_path: PathBuf,
        source: InstallSource,
        installed_at: Timestamp,
    ) -> Self {
        Self {
            id: manifest.id.clone(),
            role: manifest.engine,
            family: manifest.family.clone(),
            variant: manifest.variant.clone(),
            quant: manifest.quantization.clone(),
            file_sha256: manifest.sha256.clone(),
            manifest_sig: manifest.signature.clone(),
            file_path,
            byte_size: manifest.size_bytes,
            source,
            installed_at,
            is_active: false,
        }
    }
}

/// The set of installed models, keyed by [`ModelId`], with the "one active per
/// role" invariant enforced on mutation.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ModelRegistry {
    installs: Vec<ModelInstallation>,
}

impl ModelRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of installed models.
    #[must_use]
    pub fn len(&self) -> usize {
        self.installs.len()
    }

    /// Whether the registry holds no installs.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.installs.is_empty()
    }

    /// All installs, in insertion order.
    #[must_use]
    pub fn list(&self) -> &[ModelInstallation] {
        &self.installs
    }

    /// Installs serving a given role.
    #[must_use]
    pub fn list_by_role(&self, role: ModelEngine) -> Vec<&ModelInstallation> {
        self.installs.iter().filter(|m| m.role == role).collect()
    }

    /// Look up an install by id.
    #[must_use]
    pub fn get(&self, id: &ModelId) -> Option<&ModelInstallation> {
        self.installs.iter().find(|m| &m.id == id)
    }

    /// Whether an install with this id is present.
    #[must_use]
    pub fn contains(&self, id: &ModelId) -> bool {
        self.get(id).is_some()
    }

    /// The active model for a role, if one is selected.
    #[must_use]
    pub fn active_for_role(&self, role: ModelEngine) -> Option<&ModelInstallation> {
        self.installs.iter().find(|m| m.role == role && m.is_active)
    }

    /// Insert a new install. Fails with [`ModelError::AlreadyInstalled`] if the id
    /// is already present.
    pub fn insert(&mut self, install: ModelInstallation) -> ModelResult<()> {
        if self.contains(&install.id) {
            return Err(ModelError::AlreadyInstalled(install.id.0.clone()));
        }
        self.installs.push(install);
        Ok(())
    }

    /// Remove an install by id, returning it. Fails with
    /// [`ModelError::UnknownModel`] if absent.
    pub fn remove(&mut self, id: &ModelId) -> ModelResult<ModelInstallation> {
        let pos = self
            .installs
            .iter()
            .position(|m| &m.id == id)
            .ok_or_else(|| ModelError::UnknownModel(id.0.clone()))?;
        Ok(self.installs.remove(pos))
    }

    /// Make `id` the active model for its role, deactivating any prior active model
    /// of the same role. Fails with [`ModelError::UnknownModel`] if absent.
    pub fn set_active(&mut self, id: &ModelId) -> ModelResult<()> {
        let role = self
            .get(id)
            .ok_or_else(|| ModelError::UnknownModel(id.0.clone()))?
            .role;
        for m in &mut self.installs {
            if m.role == role {
                m.is_active = &m.id == id;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install(id: &str, role: ModelEngine) -> ModelInstallation {
        ModelInstallation {
            id: ModelId::new(id),
            role,
            family: "fam".into(),
            variant: "v".into(),
            quant: None,
            file_sha256: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".into(),
            manifest_sig: "sig".into(),
            file_path: PathBuf::from(format!("{id}.bin")),
            byte_size: 10,
            source: InstallSource::Download,
            installed_at: Timestamp::from_millis(0),
            is_active: false,
        }
    }

    #[test]
    fn insert_get_remove() {
        let mut r = ModelRegistry::new();
        r.insert(install("a", ModelEngine::Llm)).unwrap();
        assert!(r.contains(&ModelId::new("a")));
        assert_eq!(r.len(), 1);
        let removed = r.remove(&ModelId::new("a")).unwrap();
        assert_eq!(removed.id.0, "a");
        assert!(r.is_empty());
    }

    #[test]
    fn rejects_duplicate_id() {
        let mut r = ModelRegistry::new();
        r.insert(install("a", ModelEngine::Llm)).unwrap();
        let err = r.insert(install("a", ModelEngine::Stt)).unwrap_err();
        assert!(matches!(err, ModelError::AlreadyInstalled(_)));
    }

    #[test]
    fn remove_absent_errors() {
        let mut r = ModelRegistry::new();
        assert!(matches!(
            r.remove(&ModelId::new("nope")).unwrap_err(),
            ModelError::UnknownModel(_)
        ));
    }

    #[test]
    fn one_active_per_role() {
        let mut r = ModelRegistry::new();
        r.insert(install("llm1", ModelEngine::Llm)).unwrap();
        r.insert(install("llm2", ModelEngine::Llm)).unwrap();
        r.insert(install("stt1", ModelEngine::Stt)).unwrap();

        r.set_active(&ModelId::new("llm1")).unwrap();
        assert_eq!(r.active_for_role(ModelEngine::Llm).unwrap().id.0, "llm1");

        // Switching within the role deactivates the previous one.
        r.set_active(&ModelId::new("llm2")).unwrap();
        assert_eq!(r.active_for_role(ModelEngine::Llm).unwrap().id.0, "llm2");
        assert!(!r.get(&ModelId::new("llm1")).unwrap().is_active);

        // A different role is unaffected and can be active independently.
        r.set_active(&ModelId::new("stt1")).unwrap();
        assert_eq!(r.active_for_role(ModelEngine::Stt).unwrap().id.0, "stt1");
        assert_eq!(r.active_for_role(ModelEngine::Llm).unwrap().id.0, "llm2");
    }

    #[test]
    fn set_active_absent_errors() {
        let mut r = ModelRegistry::new();
        assert!(matches!(
            r.set_active(&ModelId::new("nope")).unwrap_err(),
            ModelError::UnknownModel(_)
        ));
    }

    #[test]
    fn registry_serde_round_trip() {
        let mut r = ModelRegistry::new();
        r.insert(install("a", ModelEngine::Embedder)).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let back: ModelRegistry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.len(), 1);
        assert!(back.contains(&ModelId::new("a")));
    }
}

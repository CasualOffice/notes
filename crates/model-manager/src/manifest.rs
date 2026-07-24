//! Signed model manifest schema (Architecture §8, Data Model §9.4).
//!
//! A [`ModelManifest`] is the signed record that authorizes installing one model
//! artifact. It carries everything the registry row (`model_installation`) needs
//! plus the network locator and the integrity/compatibility metadata: model id,
//! engine/role, family/variant, version, quantization, `sha256`, size, url,
//! minimum-hardware requirements, and a detached `signature`.
//!
//! ## Signing seam
//! The real ed25519 (or minisign) verification lives behind [`SignatureVerifier`].
//! No crypto crate is pulled in this phase — [`TrustAllVerifier`] is the documented
//! stub. Both the stub and any future real verifier operate over
//! [`ModelManifest::signing_payload`], the canonical bytes of every field **except**
//! `signature`, so swapping in a real verifier requires no schema change.

use serde::{Deserialize, Serialize};

use app_domain::ModelId;

use crate::error::{ModelError, ModelResult};
use crate::tier::HardwareTier;

/// The engine/role a model plays. Mirrors `model_installation.role`
/// (Data Model §9.4): `stt | llm | embedder | reranker`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelEngine {
    /// Speech-to-text (whisper / parakeet).
    Stt,
    /// Large language model (qwen3 / llama family).
    Llm,
    /// Text embedder (embeddinggemma / bge).
    Embedder,
    /// Cross-encoder reranker.
    Reranker,
}

/// Which natural languages a model handles (Architecture §8 — language-aware
/// selection). STT and embedders come in multilingual and language-specialized
/// variants (e.g. Whisper `small` is multilingual, `small.en` is English-only and
/// faster/better for English); most instruct LLMs are broadly multilingual. The
/// registry uses this to pick and download the right pack for the *user's own
/// language* (see [`crate::select`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LanguageSupport {
    /// Handles many languages with auto-detection (multilingual Whisper, Qwen3,
    /// bge-m3, …). The safe default.
    Multilingual,
    /// Specialized to a fixed set of BCP-47 primary subtags, e.g. `["en"]` for a
    /// Whisper `*.en` model. Preferred over a multilingual model *when it covers
    /// the user's language* because it is smaller/faster/more accurate there.
    Only(Vec<String>),
}

impl LanguageSupport {
    /// Whether this model can serve `lang` (a BCP-47 tag; matched on the primary
    /// subtag, so `en-US` matches `en`).
    #[must_use]
    pub fn supports(&self, lang: &str) -> bool {
        match self {
            Self::Multilingual => true,
            Self::Only(tags) => tags
                .iter()
                .any(|t| primary_subtag(t) == primary_subtag(lang)),
        }
    }

    /// True for the broad multilingual variant.
    #[must_use]
    pub const fn is_multilingual(&self) -> bool {
        matches!(self, Self::Multilingual)
    }
}

/// The lowercase primary language subtag of a BCP-47 tag (`en-US` → `en`).
#[must_use]
pub fn primary_subtag(tag: &str) -> String {
    tag.split(['-', '_'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Default when a manifest omits `languages`: assume multilingual (safe — never
/// wrongly excludes a user's language).
fn default_language_support() -> LanguageSupport {
    LanguageSupport::Multilingual
}

/// Minimum hardware a model needs to run acceptably (Architecture §8 tier probe).
///
/// `min_ram_bytes` gates load; `min_tier` is the coarse recommendation bucket the
/// hardware probe compares against; `disk_overhead_bytes` is extra scratch space
/// (mmap/kv-cache spill / temp resume file) beyond the raw artifact size.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HardwareRequirements {
    /// Minimum installed system RAM to load and run the model.
    pub min_ram_bytes: u64,
    /// Coarsest [`HardwareTier`] this model is appropriate for (inclusive floor).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_tier: Option<HardwareTier>,
    /// Working disk beyond the raw artifact bytes (temp/resume/scratch).
    #[serde(default)]
    pub disk_overhead_bytes: u64,
}

impl HardwareRequirements {
    /// A permissive default (no explicit floor) for tests/manual manifests.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            min_ram_bytes: 0,
            min_tier: None,
            disk_overhead_bytes: 0,
        }
    }
}

/// A signed manifest authorizing one model install.
///
/// Field order is stable and load-bearing: [`signing_payload`](Self::signing_payload)
/// re-serializes every field except `signature`, and that order is what a real
/// verifier signs over. Do not reorder without bumping the signing scheme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelManifest {
    /// Opaque registry id, e.g. `"qwen3-8b-q4_k_m"`.
    pub id: ModelId,
    /// The role this model plays (`stt | llm | embedder | reranker`).
    pub engine: ModelEngine,
    /// Model family, e.g. `whisper | qwen3 | embeddinggemma | bge`.
    pub family: String,
    /// Variant within the family, e.g. `base | small | 4b | 8b | 14b | 300m`.
    pub variant: String,
    /// Publisher version string for this artifact (free-form, e.g. `"1.2.0"`).
    pub version: String,
    /// Quantization label, e.g. `Q4_K_M | int8`. `None` for full-precision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantization: Option<String>,
    /// Lowercase hex SHA-256 of the artifact bytes (content address on disk).
    pub sha256: String,
    /// Exact artifact size in bytes.
    pub size_bytes: u64,
    /// Network locator for the artifact (only ever fetched by the consented
    /// `model-download` service — Architecture §8).
    pub url: String,
    /// Minimum hardware / compatibility floor.
    #[serde(default = "HardwareRequirements::none")]
    pub min_hardware: HardwareRequirements,
    /// Which natural languages this model serves. Defaults to multilingual when
    /// omitted; drives language-aware selection ([`crate::select`]).
    #[serde(default = "default_language_support")]
    pub languages: LanguageSupport,
    /// Detached signature over [`signing_payload`](Self::signing_payload), as hex
    /// or base64 depending on the (future) scheme. Persisted to
    /// `model_installation.manifest_sig`.
    pub signature: String,
}

/// The subset of manifest fields covered by the signature — everything but the
/// `signature` itself. Serialized to canonical bytes for verification.
#[derive(Serialize)]
struct SigningPayload<'a> {
    id: &'a ModelId,
    engine: ModelEngine,
    family: &'a str,
    variant: &'a str,
    version: &'a str,
    quantization: &'a Option<String>,
    sha256: &'a str,
    size_bytes: u64,
    url: &'a str,
    min_hardware: &'a HardwareRequirements,
    languages: &'a LanguageSupport,
}

impl ModelManifest {
    /// Parse a manifest from JSON bytes and validate its structural invariants.
    pub fn from_json(bytes: &[u8]) -> ModelResult<Self> {
        let m: Self = serde_json::from_slice(bytes)?;
        m.validate()?;
        Ok(m)
    }

    /// Serialize the manifest to pretty JSON (for writing `models/manifests/*`).
    pub fn to_json(&self) -> ModelResult<String> {
        Ok(serde_json::to_string_pretty(self)?)
    }

    /// The canonical bytes a signature is computed over (all fields but `signature`).
    ///
    /// Uses `serde_json` with the struct's stable field order. A future real scheme
    /// may substitute a strict canonical-JSON/CBOR encoder here without touching the
    /// public schema.
    pub fn signing_payload(&self) -> ModelResult<Vec<u8>> {
        let payload = SigningPayload {
            id: &self.id,
            engine: self.engine,
            family: &self.family,
            variant: &self.variant,
            version: &self.version,
            quantization: &self.quantization,
            sha256: &self.sha256,
            size_bytes: self.size_bytes,
            url: &self.url,
            min_hardware: &self.min_hardware,
            languages: &self.languages,
        };
        Ok(serde_json::to_vec(&payload)?)
    }

    /// Structural validation independent of the file bytes: non-empty id/family/
    /// variant, well-formed 64-char lowercase-hex sha256, non-zero size,
    /// non-empty signature.
    pub fn validate(&self) -> ModelResult<()> {
        if self.id.0.trim().is_empty() {
            return Err(ModelError::InvalidManifest("empty model id".into()));
        }
        if self.family.trim().is_empty() {
            return Err(ModelError::InvalidManifest("empty family".into()));
        }
        if self.variant.trim().is_empty() {
            return Err(ModelError::InvalidManifest("empty variant".into()));
        }
        if !is_sha256_hex(&self.sha256) {
            return Err(ModelError::InvalidManifest(format!(
                "sha256 must be 64 lowercase hex chars, got {:?}",
                self.sha256
            )));
        }
        if self.size_bytes == 0 {
            return Err(ModelError::InvalidManifest("size_bytes must be > 0".into()));
        }
        if self.signature.trim().is_empty() {
            return Err(ModelError::InvalidManifest("empty signature".into()));
        }
        Ok(())
    }
}

/// True iff `s` is exactly 64 lowercase hexadecimal characters.
#[must_use]
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Pluggable manifest-signature verifier (the deferred crypto seam).
///
/// The real implementation (ed25519 / minisign against a pinned publisher key)
/// lives in a later phase; it must verify `signature` over
/// [`ModelManifest::signing_payload`]. Keeping this a trait lets the pipeline be
/// wired and tested now without a crypto dependency.
pub trait SignatureVerifier {
    /// Verify the manifest's detached signature. Return
    /// [`ModelError::SignatureInvalid`] on mismatch.
    fn verify(&self, manifest: &ModelManifest) -> ModelResult<()>;
}

/// Stub verifier that accepts any non-empty signature. **Not secure** — it exists
/// only so the install/import pipeline is exercisable before the crypto backend
/// lands. `validate()` already guarantees the signature is non-empty; this makes
/// the trust decision explicit and swappable.
#[derive(Clone, Copy, Debug, Default)]
pub struct TrustAllVerifier;

impl SignatureVerifier for TrustAllVerifier {
    fn verify(&self, manifest: &ModelManifest) -> ModelResult<()> {
        if manifest.signature.trim().is_empty() {
            return Err(ModelError::SignatureInvalid("empty signature".into()));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_json() -> &'static str {
        r#"{
            "id": "qwen3-8b-q4_k_m",
            "engine": "llm",
            "family": "qwen3",
            "variant": "8b",
            "version": "1.0.0",
            "quantization": "Q4_K_M",
            "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "size_bytes": 4800000000,
            "url": "https://models.example/qwen3-8b-q4_k_m.gguf",
            "min_hardware": { "min_ram_bytes": 17179869184, "min_tier": "tier2" },
            "signature": "deadbeef"
        }"#
    }

    #[test]
    fn parses_full_manifest() {
        let m = ModelManifest::from_json(sample_json().as_bytes()).unwrap();
        assert_eq!(m.id.0, "qwen3-8b-q4_k_m");
        assert_eq!(m.engine, ModelEngine::Llm);
        assert_eq!(m.quantization.as_deref(), Some("Q4_K_M"));
        assert_eq!(m.size_bytes, 4_800_000_000);
        assert_eq!(m.min_hardware.min_tier, Some(HardwareTier::Tier2));
    }

    #[test]
    fn round_trips_through_json() {
        let m = ModelManifest::from_json(sample_json().as_bytes()).unwrap();
        let s = m.to_json().unwrap();
        let m2 = ModelManifest::from_json(s.as_bytes()).unwrap();
        assert_eq!(m, m2);
    }

    #[test]
    fn rejects_bad_sha256() {
        let bad = sample_json().replace(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "NOTHEX",
        );
        let err = ModelManifest::from_json(bad.as_bytes()).unwrap_err();
        assert!(matches!(err, ModelError::InvalidManifest(_)));
    }

    #[test]
    fn rejects_zero_size() {
        let bad = sample_json().replace("4800000000", "0");
        assert!(matches!(
            ModelManifest::from_json(bad.as_bytes()).unwrap_err(),
            ModelError::InvalidManifest(_)
        ));
    }

    #[test]
    fn quantization_optional() {
        let json = r#"{
            "id": "whisper-base",
            "engine": "stt",
            "family": "whisper",
            "variant": "base",
            "version": "1",
            "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "size_bytes": 142000000,
            "url": "file:///dev/null",
            "signature": "sig"
        }"#;
        let m = ModelManifest::from_json(json.as_bytes()).unwrap();
        assert!(m.quantization.is_none());
        assert!(m.min_hardware.min_tier.is_none());
    }

    #[test]
    fn signing_payload_excludes_signature() {
        let m = ModelManifest::from_json(sample_json().as_bytes()).unwrap();
        let payload = m.signing_payload().unwrap();
        let text = String::from_utf8(payload).unwrap();
        assert!(!text.contains("signature"));
        assert!(text.contains("qwen3-8b-q4_k_m"));
    }

    #[test]
    fn trust_all_rejects_empty_signature() {
        let v = TrustAllVerifier;
        let mut m = ModelManifest::from_json(sample_json().as_bytes()).unwrap();
        assert!(v.verify(&m).is_ok());
        m.signature = "   ".into();
        assert!(matches!(
            v.verify(&m).unwrap_err(),
            ModelError::SignatureInvalid(_)
        ));
    }
}

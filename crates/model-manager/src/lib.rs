//! # model-manager
//!
//! Local model distribution for Casual Note — the on-device half of the signed
//! model registry described in **Architecture §8 ("Network Isolation & Model
//! Management")** and **Data Model §9.4 (`model_installation`)**. It is one of only
//! two components (`model-download` here, plus `updater`) permitted to own a
//! socket, and only under explicit user consent.
//!
//! This crate is deliberately **dependency-light**: no HTTP client is pulled in
//! this phase. Network fetching is expressed as the [`Downloader`] trait, shipped
//! here with a local-file [`MockDownloader`] only; the real resumable-`Range` HTTP
//! backend is a documented, deferred seam. Everything else — manifest schema,
//! integrity verification, disk/quota preflight, tier recommendation, offline
//! import, and the registry — is pure Rust over `app-domain` + `serde` + `sha2`.
//!
//! ## Modules
//! - [`manifest`] — the signed [`ModelManifest`] schema (id, engine, version,
//!   quantization, sha256, size, url, min-hardware, signature) and the
//!   [`SignatureVerifier`](manifest::SignatureVerifier) seam (Architecture §8,
//!   Data Model §9.4).
//! - [`checksum`] — streaming **SHA-256** + size-bound verification; refuse-and-
//!   report on mismatch (Architecture §9).
//! - [`disk`] — disk-space **preflight** and a **quota** notion (Architecture §8).
//! - [`tier`] — **hardware-tier recommendation** (8 GB → 3-4B, 16 GB → 7-8B,
//!   24-32 GB → 12-14B) with a manual-override hook (Architecture §8, PRD P4).
//! - [`download`] — the [`Downloader`](download::Downloader) trait (resumable,
//!   progress) + mock impl.
//! - [`registry`] — [`ModelInstallation`](registry::ModelInstallation) rows and the
//!   in-memory [`ModelRegistry`](registry::ModelRegistry) (Data Model §9.4);
//!   durable persistence is the `storage` crate's job.
//! - [`pipeline`] — install orchestration: **offline USB import** and network
//!   download, both funnelled through signature → preflight → verify → record.
//!
//! ## Security posture (Architecture §9)
//! A model file is admitted only after its manifest signature verifies and its
//! bytes match the manifest's SHA-256 and exact size. Any mismatch is terminal:
//! the partial/imported file is removed and the error surfaces — a malicious or
//! corrupt model is never loaded.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod checksum;
pub mod disk;
pub mod download;
pub mod error;
pub mod manifest;
pub mod pipeline;
pub mod registry;
pub mod select;
pub mod tier;

#[cfg(test)]
mod test_support;

// --- Flat re-exports of the primary API surface ---
pub use checksum::{sha256_hex_file, sha256_hex_reader, verify_against_manifest, verify_file};
pub use disk::{estimate_dir_size, preflight, DiskBudget, SpaceEstimate};
pub use download::{DownloadEvent, DownloadOutcome, DownloadRequest, Downloader, MockDownloader};
pub use error::{ModelError, ModelResult};
pub use manifest::{
    is_sha256_hex, primary_subtag, HardwareRequirements, LanguageSupport, ModelEngine,
    ModelManifest, SignatureVerifier, TrustAllVerifier,
};
pub use pipeline::{default_rel_path, import_local, install_from_download, InstallConfig};
pub use registry::{InstallSource, ModelInstallation, ModelRegistry};
pub use select::{select_for_language, select_pack, LanguagePreference, Selection};
pub use tier::{tier_for_ram, HardwareTier, TierProfile, TierRecommendation};

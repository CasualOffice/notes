//! # capture-api — per-application audio-capture contract
//!
//! Defines the [`ApplicationCaptureAdapter`] trait and its honest capability /
//! configuration / permission report types, implementing the **HLD §9.1 Capture
//! trait** and **Architecture §5 "Cross-Platform Capture Architecture"**.
//!
//! Per-application audio capture semantics differ fundamentally across OSes
//! (macOS ScreenCaptureKit, Windows WASAPI process-loopback, Linux PipeWire), so
//! capture lives behind this single Rust trait with native adapters and **honest
//! capability reporting** — the app never pretends to a capability the adapter
//! reports absent (CLAUDE.md "Capability honesty"; Quality Gate G9).
//!
//! This crate is the **pure contract layer only**: the heavy native backends
//! (`capture-macos`/`capture-windows`/`capture-linux`) implement the trait over
//! FFI and own the RT-audio ring sink. Raw PCM never crosses this boundary as
//! JSON/IPC — it stays in native memory (CLAUDE.md "No RT-callback sin", N13).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

use app_domain::{Id, Platform, SessionId, Timestamp};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

// ---------------------------------------------------------------------------
// Identifiers
// ---------------------------------------------------------------------------

/// An opaque, platform-specific handle for a capturable application.
///
/// The string is meaningful only to the adapter that produced it:
/// - **macOS** — an `SCRunningApplication` bundle identifier.
/// - **Windows** — a process id / process-tree root token.
/// - **Linux** — a PipeWire node id.
///
/// The UI treats it as an opaque token and echoes it back in a [`CaptureConfig`].
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PlatformAppId(pub String);

impl PlatformAppId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PlatformAppId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Capability / health reporting (honest, per-platform)
// ---------------------------------------------------------------------------

/// How well a capability is supported on the current platform — reported, never
/// faked (CLAUDE.md "Capability honesty").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportLevel {
    /// First-class support (e.g. macOS/Windows app-level audio).
    Supported,
    /// Available but not guaranteed for every source (e.g. Linux node-level).
    BestEffort,
    /// Not available on this platform at all.
    Unsupported,
}

/// Policy for capturing the whole system mix when per-application capture is not
/// possible. Casual Note **never silently** falls back to system-wide audio
/// (CLAUDE.md "Capability honesty"; HLD §9.1 Windows row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemFallback {
    /// System-wide capture is not a concept on this platform.
    NotApplicable,
    /// Available only with an explicit, separate user choice — never silent.
    ExplicitOnly,
    /// System-wide fallback is unavailable.
    Unavailable,
}

/// Live health of the capture subsystem — the "capability/health" enum surfaced
/// to the UI health panel (Architecture §5; Observability without telemetry).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum CaptureHealth {
    /// Ready to arm a capture immediately.
    Ready,
    /// An OS permission grant is required before capture can start.
    PermissionRequired,
    /// The backend is present but currently degraded (reason surfaced honestly).
    Degraded { reason: String },
    /// No capture backend is available on this platform/target.
    Unavailable,
}

/// Honest per-platform capability report, surfaced to the UI so it never exposes
/// a capability the adapter lacks (HLD §9.1; Quality Gate G9).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaptureCapabilities {
    pub platform: Platform,
    /// Whether per-application audio capture is available, and how well.
    pub app_level_audio: SupportLevel,
    /// Whether the adapter can exclude its own audio (macOS `excludesCurrentProcessAudio`).
    pub exclude_self: bool,
    /// Whether a separate microphone track can be captured alongside app audio.
    pub microphone: bool,
    /// System-wide capture policy — never a silent fallback.
    pub system_fallback: SystemFallback,
    /// Current live health.
    pub health: CaptureHealth,
}

// ---------------------------------------------------------------------------
// Enumeration
// ---------------------------------------------------------------------------

/// An application that can be selected as a capture source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapturableApp {
    pub app_id: PlatformAppId,
    /// Human-readable name for the picker UI.
    pub display_name: String,
    /// Executable / bundle path, when the platform exposes one.
    pub executable: Option<String>,
    /// Whether the app is currently producing audio (best-effort hint).
    pub produces_audio: bool,
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

/// The grant state of a single OS permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionState {
    /// Granted by the user / OS.
    Granted,
    /// Explicitly denied — the user must change it in system settings.
    Denied,
    /// Not yet requested (no decision recorded).
    NotDetermined,
    /// Not required on this platform for the requested configuration.
    NotRequired,
}

/// The outcome of a permission preflight/request for a given [`CaptureConfig`]
/// (HLD §9.1 `preflight`; macOS TCC / Wayland portal flows).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PermissionReport {
    /// Screen-capture / audio-capture entitlement (macOS TCC, portal on Wayland).
    pub screen_capture: PermissionState,
    /// Microphone permission (only relevant when `capture_microphone` is set).
    pub microphone: PermissionState,
    /// Wayland/PipeWire portal consent (Linux); `NotRequired` elsewhere.
    pub portal: PermissionState,
    /// Convenience: true iff nothing is missing for the requested config.
    pub all_granted: bool,
}

// ---------------------------------------------------------------------------
// Capture request + handle
// ---------------------------------------------------------------------------

/// Everything needed to arm a capture. Echoes the [`PlatformAppId`]s the UI chose
/// from [`ApplicationCaptureAdapter::list_applications`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// The applications to capture. Empty is rejected by adapters that require an
    /// explicit target (no silent system-wide capture).
    pub targets: Vec<PlatformAppId>,
    /// Also capture the default input device (microphone) as a separate track.
    pub capture_microphone: bool,
    /// Exclude Casual Note's own audio from the capture (macOS exclude-self).
    pub exclude_self: bool,
    /// Requested PCM sample rate in Hz (adapters normalise downstream to 16 kHz
    /// mono in `media-pipeline`).
    pub sample_rate_hz: u32,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            targets: Vec::new(),
            capture_microphone: false,
            exclude_self: true,
            sample_rate_hz: 48_000,
        }
    }
}

/// An opaque handle to a running capture, returned by
/// [`ApplicationCaptureAdapter::start`] and consumed by `stop`.
///
/// Contract-level identity only: the native ring buffer, callback thread and OS
/// stream objects live inside the adapter and are keyed by `capture_id`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureHandle {
    /// The meeting session this capture feeds (`kind='session'`).
    pub session_id: SessionId,
    /// Adapter-private correlation id for the native capture instance.
    pub capture_id: Id,
    /// When the capture was armed.
    pub started_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failures raised across the capture contract (typed per the Architecture error
/// taxonomy; libs use `thiserror`).
#[derive(Debug, Error)]
pub enum CaptureError {
    /// A required OS permission was denied by the user.
    #[error("capture permission denied: {0}")]
    PermissionDenied(String),

    /// A required OS permission has not been granted yet (preflight before start).
    #[error("capture permission required: {0}")]
    PermissionRequired(String),

    /// The requested application is no longer present / capturable.
    #[error("capturable application not found: {0}")]
    AppNotFound(PlatformAppId),

    /// No target was supplied but this platform forbids silent system-wide capture.
    #[error("no capture target selected (system-wide fallback is never silent)")]
    NoTargetSelected,

    /// Per-process loopback is unavailable (e.g. Windows without process-loopback);
    /// reported honestly rather than degrading to system-wide audio.
    #[error("per-application loopback unavailable on this platform/build")]
    LoopbackUnavailable,

    /// The audio device or stream could not be opened.
    #[error("capture device unavailable: {0}")]
    DeviceUnavailable(String),

    /// A capture is already running for this adapter.
    #[error("a capture is already active")]
    AlreadyActive,

    /// The supplied handle does not correspond to an active capture.
    #[error("no active capture for the supplied handle")]
    NotActive,

    /// Transient native-backend error; retryable per the taxonomy.
    #[error("capture glitch (retryable): {0}")]
    Glitch(String),

    /// Catch-all for a native/backend failure.
    #[error("capture backend error: {0}")]
    Backend(String),
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// The unified per-application audio-capture adapter (HLD §9.1).
///
/// Implemented natively per platform behind FFI (`capture-macos`,
/// `capture-windows`, `capture-linux`). All methods that touch the OS are async
/// so the caller (`app-service`) can drive them from its Tokio runtime without
/// blocking; the RT audio callback itself never appears here (N13).
#[async_trait]
pub trait ApplicationCaptureAdapter: Send + Sync {
    /// Honest, synchronous capability snapshot for the UI capability report.
    fn capabilities(&self) -> CaptureCapabilities;

    /// Enumerate applications currently available as capture sources.
    async fn list_applications(&self) -> Result<Vec<CapturableApp>, CaptureError>;

    /// Preflight / request the OS permissions the given config needs (macOS TCC,
    /// Wayland portal). Reports state honestly; never starts capture.
    async fn request_permissions(
        &self,
        config: &CaptureConfig,
    ) -> Result<PermissionReport, CaptureError>;

    /// Arm a capture for the given config, returning a handle. The adapter wires
    /// its native ring sink internally; PCM never crosses this boundary.
    async fn start(&self, config: CaptureConfig) -> Result<CaptureHandle, CaptureError>;

    /// Stop a running capture and release native resources.
    async fn stop(&self, handle: CaptureHandle) -> Result<(), CaptureError>;
}

// ---------------------------------------------------------------------------
// Test double
// ---------------------------------------------------------------------------

/// A no-op [`ApplicationCaptureAdapter`] that reports *no* capture capability.
///
/// Used to exercise the trait in tests and to stand in on unsupported targets
/// where the app must still degrade honestly (health = `Unavailable`).
#[derive(Clone, Debug, Default)]
pub struct NullCaptureAdapter;

#[async_trait]
impl ApplicationCaptureAdapter for NullCaptureAdapter {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            platform: Platform::current().unwrap_or(Platform::Linux),
            app_level_audio: SupportLevel::Unsupported,
            exclude_self: false,
            microphone: false,
            system_fallback: SystemFallback::Unavailable,
            health: CaptureHealth::Unavailable,
        }
    }

    async fn list_applications(&self) -> Result<Vec<CapturableApp>, CaptureError> {
        Ok(Vec::new())
    }

    async fn request_permissions(
        &self,
        _config: &CaptureConfig,
    ) -> Result<PermissionReport, CaptureError> {
        // Nothing to grant: this adapter has no capture backend to authorise.
        Ok(PermissionReport {
            screen_capture: PermissionState::NotRequired,
            microphone: PermissionState::NotRequired,
            portal: PermissionState::NotRequired,
            all_granted: true,
        })
    }

    async fn start(&self, config: CaptureConfig) -> Result<CaptureHandle, CaptureError> {
        if config.targets.is_empty() {
            return Err(CaptureError::NoTargetSelected);
        }
        // The null adapter cannot actually capture.
        Err(CaptureError::LoopbackUnavailable)
    }

    async fn stop(&self, _handle: CaptureHandle) -> Result<(), CaptureError> {
        Err(CaptureError::NotActive)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_adapter_reports_no_capability() {
        let caps = NullCaptureAdapter.capabilities();
        assert_eq!(caps.app_level_audio, SupportLevel::Unsupported);
        assert!(matches!(caps.health, CaptureHealth::Unavailable));
        assert!(!caps.exclude_self);
    }

    #[tokio::test]
    async fn null_adapter_exercises_full_trait() {
        let adapter: &dyn ApplicationCaptureAdapter = &NullCaptureAdapter;

        assert!(adapter.list_applications().await.unwrap().is_empty());

        let cfg = CaptureConfig::default();
        let report = adapter.request_permissions(&cfg).await.unwrap();
        assert!(report.all_granted);
        assert_eq!(report.screen_capture, PermissionState::NotRequired);

        // Empty target set is rejected: no silent system-wide capture.
        assert!(matches!(
            adapter.start(CaptureConfig::default()).await,
            Err(CaptureError::NoTargetSelected)
        ));

        // With a target, the null backend degrades honestly.
        let targeted = CaptureConfig {
            targets: vec![PlatformAppId::new("com.example.app")],
            ..CaptureConfig::default()
        };
        assert!(matches!(
            adapter.start(targeted).await,
            Err(CaptureError::LoopbackUnavailable)
        ));
    }

    #[test]
    fn capability_report_serialises_honestly() {
        let caps = CaptureCapabilities {
            platform: Platform::Linux,
            app_level_audio: SupportLevel::BestEffort,
            exclude_self: false,
            microphone: true,
            system_fallback: SystemFallback::NotApplicable,
            health: CaptureHealth::Degraded {
                reason: "portal consent pending".into(),
            },
        };
        let v: serde_json::Value = serde_json::to_value(&caps).unwrap();
        assert_eq!(v["platform"], "linux");
        assert_eq!(v["app_level_audio"], "best_effort");
        assert_eq!(v["health"]["state"], "degraded");
        assert_eq!(v["health"]["reason"], "portal consent pending");
    }
}

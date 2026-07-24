//! # Meeting intelligence pipeline (M2)
//!
//! The session **state machine** and **[`SessionCoordinator`]** that drive a meeting
//! from `NEW` to `COMPLETE`, implementing **HLD §8.4** (capture → transcribe →
//! artifact → action-items → tasks), the **Architecture §10** state-machine backbone
//! (`NEW→PREFLIGHT→READY→RECORDING↔PAUSED→STOPPING→CAPTURED→FINAL_TRANSCRIBING→`
//! `GENERATING→INDEXING→COMPLETE` + `DEGRADED`/`FAILED`/`RECOVERING`), and the
//! **Data Model §8** session/track/segment/artifact/action-item tables.
//!
//! The coordinator composes the Phase-2 engine **traits** — `capture-api`
//! ([`ApplicationCaptureAdapter`]), `speech-api` ([`SpeechEngine`]), `llm-api`
//! ([`ConstrainedLlm`]) — over the `media-pipeline` DSP. The real whisper.cpp /
//! llama.cpp / OS-capture backends plug into the *same* trait seam later; here the
//! default doubles are the mock engines so the whole flow is testable with no native
//! deps.
//!
//! ## Non-negotiable invariants honoured here
//! - **The LLM never owns recording state.** A generation failure routes `GENERATING
//!   → DEGRADED` (recoverable) and the transcript is preserved; capture/STT already
//!   completed and are never rolled back.
//! - **Evidence or nothing.** Every persisted artifact fact carries
//!   `evidence_segment_ids` that resolve to real `transcript_segment` rows; facts
//!   whose evidence does not resolve are dropped before persistence.
//! - **Structured output validates or falls back deterministically.** Generation goes
//!   through `llm-api`'s repair→deterministic-fallback contract (never malformed).
//! - **Op-log seam.** Every session/track/segment/artifact/action-item/link/note
//!   mutation appends to `entity_op`, so the meeting rebuilds bit-identically from the
//!   log (the correctness oracle).
//! - **PCM never crosses the WebView / JSON boundary.** Audio stays as borrowed
//!   `&[f32]` fed to the DSP; only content-addressed metadata is persisted (N13).

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use app_domain::{
    AppError, AppEvent, AppResult, Id, LinkRel, Platform, SegmentId, SessionId, SessionState,
    Timestamp, TranscriptPass, TranscriptSegment as EventSegment,
};
use capture_api::{
    ApplicationCaptureAdapter, CapturableApp, CaptureCapabilities, CaptureConfig, CaptureError,
    CaptureHandle, CaptureHealth, PermissionReport, PermissionState, PlatformAppId, SupportLevel,
    SystemFallback,
};
use llm_api::{
    generate_structured, ActionItem, ConstrainedLlm, GenerationOutcome, GenerationPath,
    GenerationRequest, Grammar, MeetingArtifactV1, Topic,
};
use media_pipeline::{Chunk, Pipeline, PipelineConfig};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use speech_api::{AudioChunk, AudioSpan, MockSpeechEngine, SpeechEngine, SpeechModelProfile};
use storage::{AudioTrackRow, DetailTable, EntityOp, LinkRow, OpBody, TranscriptSegmentRow};

use crate::dto::NewTask;
use crate::notes::parse_id;
use crate::util::{self, Columns};
use crate::Service;

// ===========================================================================
// State machine — exhaustive legal transitions
// ===========================================================================

/// Whether `from → to` is a legal session-state transition (Architecture §10).
///
/// Terminal states (`Complete`, `Failed`) have no outgoing edges. `Degraded` is
/// recoverable (→ `Recovering`); `Recovering` re-enters the pipeline at the stage
/// that degraded. The `Generating → Degraded` edge is the invariant seam: a failed
/// LLM step degrades without stopping capture/STT (the transcript is already durable).
#[must_use]
pub fn legal_transition(from: SessionState, to: SessionState) -> bool {
    use SessionState as S;
    matches!(
        (from, to),
        (S::New, S::Preflight)
            | (S::Preflight, S::Ready | S::Failed | S::Degraded)
            | (S::Ready, S::Recording | S::Failed | S::Degraded)
            | (
                S::Recording,
                S::Paused | S::Stopping | S::Degraded | S::Failed
            )
            | (S::Paused, S::Recording | S::Stopping | S::Failed)
            | (S::Stopping, S::Captured | S::Failed)
            | (S::Captured, S::FinalTranscribing | S::Failed | S::Degraded)
            | (
                S::FinalTranscribing,
                S::Generating | S::Degraded | S::Failed
            )
            | (S::Generating, S::Indexing | S::Degraded | S::Failed)
            | (S::Indexing, S::Complete | S::Degraded | S::Failed)
            | (S::Degraded, S::Recovering | S::Failed)
            | (
                S::Recovering,
                S::FinalTranscribing | S::Generating | S::Indexing | S::Complete | S::Failed
            )
    )
}

/// A live session runner: current state + the one-writer facade it persists through.
/// Enforces [`legal_transition`], persists the new `state`, and emits
/// [`AppEvent::SessionStateChanged`] on every hop.
struct Machine<'a> {
    service: &'a Service,
    id: SessionId,
    state: SessionState,
    started_at: i64,
}

impl<'a> Machine<'a> {
    /// Transition to `to`, merging any extra session detail columns into the same op.
    fn to(&mut self, to: SessionState, extra: Columns, degraded: Option<String>) -> AppResult<()> {
        if !legal_transition(self.state, to) {
            return Err(AppError::Internal(format!(
                "illegal session transition {:?} -> {:?}",
                self.state, to
            )));
        }
        let from = self.state;
        self.state = to;

        let mut cols = extra;
        cols.insert("state".into(), Value::String(to.as_str().into()));
        if let Some(reason) = &degraded {
            cols.insert("degraded_reason".into(), Value::String(reason.clone()));
        }
        self.service.session_update(self.id, cols)?;

        self.service.emit(AppEvent::SessionStateChanged {
            session_id: self.id,
            from,
            to,
            degraded,
        });
        Ok(())
    }
}

// ===========================================================================
// Public DTOs / injection points
// ===========================================================================

/// `meeting.preflight` result — the honest capability + permission report the UI
/// gates the arm affordance on (HLD §9.1; capability honesty).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PreflightReport {
    pub capabilities: CaptureCapabilities,
    pub permissions: PermissionReport,
    /// True iff capture can be armed immediately for the requested config.
    pub ready: bool,
}

/// Configuration for one meeting session (echoes the `meeting.start` payload).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MeetingConfig {
    /// Opaque per-application capture targets chosen in the picker.
    pub sources: Vec<String>,
    /// Also capture the default microphone as a separate track.
    pub capture_microphone: bool,
    /// Exclude Casual Note's own audio (macOS exclude-self).
    pub exclude_self: bool,
    /// Requested native capture sample rate (Hz); the DSP normalises to 16 kHz mono.
    pub sample_rate_hz: u32,
    /// Optional note-title binding for the meeting-as-note.
    pub title: Option<String>,
}

/// The terminal result of driving a session to rest (`COMPLETE` or `DEGRADED`).
#[derive(Clone, Debug, Serialize)]
pub struct SessionOutcome {
    pub session_id: String,
    pub state: SessionState,
    /// The current (cleaned, evidence-resolved) artifact, when generation succeeded.
    pub artifact: Option<MeetingArtifactV1>,
    /// The meeting-as-note id (INDEXING), when the session reached COMPLETE.
    pub note_id: Option<String>,
    /// Which repair→fallback path produced the artifact (`direct`/`repaired`/
    /// `deterministic_fallback`) — provenance for observability.
    pub generation_path: Option<String>,
    pub segment_count: usize,
    pub action_item_count: usize,
    /// Populated in DEGRADED/FAILED.
    pub degraded_reason: Option<String>,
}

/// A `session` row projection (`meeting.get`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionView {
    pub id: String,
    pub state: String,
    pub note_id: Option<String>,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub platform: String,
    pub degraded_reason: Option<String>,
}

/// One suggested `action_item` (the review surface before promotion to a Task).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ActionItemView {
    pub id: String,
    pub idx: i64,
    pub task_text: String,
    pub owner_text: Option<String>,
    pub due_date: Option<String>,
    pub evidence_segment_ids: Vec<String>,
    pub status: String,
    pub promoted_task_id: Option<String>,
}

/// Optional user overrides applied when promoting an action item to a Task
/// (`meeting.actionItemToTask`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct ActionItemOverrides {
    pub title: Option<String>,
    pub deadline_on: Option<String>,
    pub project_id: Option<String>,
    pub area_id: Option<String>,
}

/// A block of native interleaved PCM handed to the coordinator's DSP. In production
/// this is fed from the `capture-*` native ring; here it is injectable so the flow
/// is testable with canned audio. PCM never crosses IPC/JSON (N13).
#[derive(Clone, Debug)]
pub struct CaptureBlock {
    pub interleaved: Vec<f32>,
    pub sample_rate_hz: u32,
    pub channels: u16,
}

/// A source of native PCM blocks for a session. `next_block` returns `None` at
/// end-of-capture. The default double is [`CannedAudioSource`].
pub trait AudioSource: Send {
    fn next_block(&mut self) -> Option<CaptureBlock>;
}

/// A deterministic [`AudioSource`] that replays a fixed list of blocks (tests/dev).
#[derive(Clone, Debug)]
pub struct CannedAudioSource {
    blocks: std::collections::VecDeque<CaptureBlock>,
}

impl CannedAudioSource {
    #[must_use]
    pub fn new(blocks: Vec<CaptureBlock>) -> Self {
        Self {
            blocks: blocks.into_iter().collect(),
        }
    }

    /// A single-tone mono/stereo source: `seconds` of a `freq`-Hz sine at `rate`/
    /// `channels`, chunked into `block_ms` blocks. Deterministic and speech-VAD
    /// friendly for exercising the pipeline.
    #[must_use]
    pub fn tone(freq: f64, rate: u32, channels: u16, seconds: f64, block_ms: u32) -> Self {
        let total = (f64::from(rate) * seconds) as usize;
        let per_block = ((u64::from(rate) * u64::from(block_ms)) / 1000) as usize;
        let mut blocks = Vec::new();
        let mut i = 0usize;
        while i < total {
            let end = (i + per_block).min(total);
            let mut interleaved = Vec::with_capacity((end - i) * channels as usize);
            for n in i..end {
                let s =
                    0.6 * (2.0 * std::f64::consts::PI * freq * n as f64 / f64::from(rate)).sin();
                for _ in 0..channels {
                    interleaved.push(s as f32);
                }
            }
            blocks.push(CaptureBlock {
                interleaved,
                sample_rate_hz: rate,
                channels,
            });
            i = end;
        }
        Self::new(blocks)
    }
}

impl AudioSource for CannedAudioSource {
    fn next_block(&mut self) -> Option<CaptureBlock> {
        self.blocks.pop_front()
    }
}

// ===========================================================================
// Mock capture adapter (the capture-side test double capture-api lacks)
// ===========================================================================

/// A working [`ApplicationCaptureAdapter`] test double that reports first-class
/// capability and arms/stops successfully. Pairs with a [`CannedAudioSource`] so the
/// coordinator can run a whole session with no native backend. (`capture-api` only
/// ships `NullCaptureAdapter`, which reports *no* capability and cannot start.)
#[derive(Clone, Debug, Default)]
pub struct MockCaptureAdapter;

#[async_trait::async_trait]
impl ApplicationCaptureAdapter for MockCaptureAdapter {
    fn capabilities(&self) -> CaptureCapabilities {
        CaptureCapabilities {
            platform: Platform::current().unwrap_or(Platform::Linux),
            app_level_audio: SupportLevel::Supported,
            exclude_self: true,
            microphone: true,
            system_fallback: SystemFallback::ExplicitOnly,
            health: CaptureHealth::Ready,
        }
    }

    async fn list_applications(&self) -> Result<Vec<CapturableApp>, CaptureError> {
        Ok(vec![CapturableApp {
            app_id: PlatformAppId::new("mock.app"),
            display_name: "Mock App".into(),
            executable: None,
            produces_audio: true,
        }])
    }

    async fn request_permissions(
        &self,
        _config: &CaptureConfig,
    ) -> Result<PermissionReport, CaptureError> {
        Ok(PermissionReport {
            screen_capture: PermissionState::Granted,
            microphone: PermissionState::Granted,
            portal: PermissionState::NotRequired,
            all_granted: true,
        })
    }

    async fn start(&self, _config: CaptureConfig) -> Result<CaptureHandle, CaptureError> {
        Ok(CaptureHandle {
            session_id: Id::new(),
            capture_id: Id::new(),
            started_at: Timestamp::now(),
        })
    }

    async fn stop(&self, _handle: CaptureHandle) -> Result<(), CaptureError> {
        Ok(())
    }
}

// ===========================================================================
// The coordinator
// ===========================================================================

/// Drives one meeting session over the injected engine traits (HLD §8.4). Holds the
/// capture adapter, the (interior-mutable) speech engine, and the constrained LLM,
/// plus a small Tokio runtime for the async capture lifecycle calls. The engines are
/// the swappable seam: defaults are the mock doubles, so the pipeline runs with no
/// native deps.
pub struct SessionCoordinator {
    capture: Arc<dyn ApplicationCaptureAdapter>,
    // `SpeechEngine` needs `&mut self` and is `Send` (not `Sync`); held behind a
    // Mutex so the coordinator itself stays `Send + Sync`.
    speech: Mutex<Box<dyn SpeechEngine>>,
    llm: Arc<dyn ConstrainedLlm>,
    rt: tokio::runtime::Runtime,
}

impl std::fmt::Debug for SessionCoordinator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionCoordinator")
            .field("capture", &self.capture.capabilities().platform)
            .field("llm_model", &self.llm.model_id())
            .finish_non_exhaustive()
    }
}

impl SessionCoordinator {
    /// Build a coordinator over injected engines. The speech engine is owned (its
    /// methods take `&mut self`); capture and LLM are shared `Arc`s.
    ///
    /// # Errors
    /// Returns [`AppError::Internal`] if the internal Tokio runtime cannot be built.
    pub fn new(
        capture: Arc<dyn ApplicationCaptureAdapter>,
        speech: Box<dyn SpeechEngine>,
        llm: Arc<dyn ConstrainedLlm>,
    ) -> AppResult<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|e| AppError::Internal(format!("session runtime: {e}")))?;
        Ok(Self {
            capture,
            speech: Mutex::new(speech),
            llm,
            rt,
        })
    }

    /// A coordinator wired to the default mock engines (mock capture + mock speech +
    /// a dumb LLM that always drives the deterministic topics-only fallback). Suitable
    /// for headless flow tests and for the app before native backends are plugged in.
    ///
    /// # Errors
    /// Propagates a runtime-build failure from [`SessionCoordinator::new`].
    pub fn with_mocks() -> AppResult<Self> {
        Self::new(
            Arc::new(MockCaptureAdapter),
            Box::new(MockSpeechEngine::new()),
            Arc::new(llm_api::MockLlm::always("{}")), // invalid artifact -> fallback path
        )
    }

    /// `meeting.preflight` — honest capability + permission report (HLD §9.1). Never
    /// starts capture.
    ///
    /// # Errors
    /// Returns a mapped [`AppError`] if the permission preflight fails.
    pub fn preflight(&self, sources: Vec<String>) -> AppResult<PreflightReport> {
        let caps = self.capture.capabilities();
        let cfg = self.capture_config(&MeetingConfig {
            sources,
            exclude_self: caps.exclude_self,
            ..MeetingConfig::default()
        });
        let permissions = self
            .rt
            .block_on(self.capture.request_permissions(&cfg))
            .map_err(map_capture_err)?;
        let ready = permissions.all_granted && matches!(caps.health, CaptureHealth::Ready);
        Ok(PreflightReport {
            capabilities: caps,
            permissions,
            ready,
        })
    }

    /// Drive a whole session `NEW → COMPLETE` (or `→ DEGRADED` on a generation
    /// failure), persisting Session / AudioTrack(s) / TranscriptSegment / Artifact /
    /// ActionItem rows and writing the meeting into the spine + FTS + link graph
    /// (INDEXING). This is the headless driver behind the `meeting.*` command surface;
    /// real capture threading (start/pause/resume/stop as discrete commands) plugs the
    /// native backend into the same trait seam later.
    ///
    /// # Errors
    /// Returns an [`AppError`] only on a hard failure that routes the session to
    /// `FAILED` (e.g. capture cannot arm). A generation failure is *not* an error — it
    /// returns `Ok` with `state = DEGRADED` and the transcript preserved.
    pub fn run_session(
        &self,
        service: &Service,
        config: &MeetingConfig,
        audio: &mut dyn AudioSource,
    ) -> AppResult<SessionOutcome> {
        let session_id = Id::new();
        let caps = self.capture.capabilities();
        let platform = caps.platform;
        let capture_source = capture_source_json(config, &caps);

        service.session_create(session_id, platform, &capture_source, config.title.clone())?;
        let mut m = Machine {
            service,
            id: session_id,
            state: SessionState::New,
            started_at: 0,
        };

        // NEW → PREFLIGHT → (permission gate) → READY
        m.to(SessionState::Preflight, Columns::new(), None)?;
        let cfg = self.capture_config(config);
        let perms = match self.rt.block_on(self.capture.request_permissions(&cfg)) {
            Ok(p) => p,
            Err(e) => return self.fail(&mut m, session_id, &format!("preflight: {e}")),
        };
        if !perms.all_granted {
            return self.fail(&mut m, session_id, "capture permission not granted");
        }
        m.to(SessionState::Ready, Columns::new(), None)?;

        // READY → RECORDING (arm capture; the LLM never owns this state)
        let handle = match self.rt.block_on(self.capture.start(cfg)) {
            Ok(h) => h,
            Err(e) => return self.fail(&mut m, session_id, &format!("capture start: {e}")),
        };
        let started_at = service.now_ms();
        m.started_at = started_at;
        m.to(
            SessionState::Recording,
            util::col1("started_at", Value::Number(started_at.into())),
            None,
        )?;

        // Persist the capture track(s), then run the DSP + live pass.
        let track_id = service.audio_track_persist(&AudioTrackRow {
            id: Id::new(),
            session_id,
            source_kind: "app_audio".into(),
            source_label: Some("app audio".into()),
            sample_rate: i64::from(config.sample_rate_hz.max(16_000)),
            channels: 2,
            audio_sha256: None,
            byte_size: None,
        })?;
        if config.capture_microphone {
            service.audio_track_persist(&AudioTrackRow {
                id: Id::new(),
                session_id,
                source_kind: "mic".into(),
                source_label: Some("microphone".into()),
                sample_rate: i64::from(config.sample_rate_hz.max(16_000)),
                channels: 1,
                audio_sha256: None,
                byte_size: None,
            })?;
        }
        let chunks = self.record(&m, audio)?;

        // RECORDING → STOPPING → CAPTURED
        m.to(SessionState::Stopping, Columns::new(), None)?;
        let _ = self.rt.block_on(self.capture.stop(handle)); // best-effort release
        m.to(SessionState::Captured, Columns::new(), None)?;

        // CAPTURED → FINAL_TRANSCRIBING (pass-2, the authoritative evidence)
        m.to(SessionState::FinalTranscribing, Columns::new(), None)?;
        let segs = self.finalize(service, session_id, track_id, &chunks)?;

        // FINAL_TRANSCRIBING → GENERATING (repair→deterministic fallback; the LLM
        // never blocks recording — a backend failure degrades and keeps the transcript)
        m.to(SessionState::Generating, Columns::new(), None)?;
        let generated = match self.generate(service, session_id, &segs) {
            Ok(g) => g,
            Err(e) => {
                let reason = format!("generation failed: {e}");
                m.to(SessionState::Degraded, Columns::new(), Some(reason.clone()))?;
                return Ok(SessionOutcome {
                    session_id: session_id.to_string(),
                    state: SessionState::Degraded,
                    artifact: None,
                    note_id: None,
                    generation_path: None,
                    segment_count: segs.len(),
                    action_item_count: 0,
                    degraded_reason: Some(reason),
                });
            }
        };
        service.emit(AppEvent::ArtifactReady { session_id });

        // GENERATING → INDEXING → COMPLETE (meeting-as-note + action items + links)
        m.to(SessionState::Indexing, Columns::new(), None)?;
        let note_id = service.index_meeting(
            session_id,
            generated.artifact_id,
            &generated.artifact,
            &segs,
        )?;

        let ended_at = service.now_ms();
        let mut done = Columns::new();
        done.insert("ended_at".into(), Value::Number(ended_at.into()));
        done.insert(
            "duration_ms".into(),
            Value::Number((ended_at - started_at).max(0).into()),
        );
        done.insert("note_id".into(), Value::String(note_id.to_string()));
        m.to(SessionState::Complete, done, None)?;

        Ok(SessionOutcome {
            session_id: session_id.to_string(),
            state: SessionState::Complete,
            action_item_count: generated.artifact.action_items.len(),
            segment_count: segs.len(),
            note_id: Some(note_id.to_string()),
            generation_path: Some(path_name(generated.path).to_string()),
            artifact: Some(generated.artifact),
            degraded_reason: None,
        })
    }

    /// `meeting.actionItemToTask` — the cross-pillar bridge (Data Model §8.5). Creates
    /// a Task from a suggested action item, records provenance on the `spawned_from`
    /// edge (with copied `evidence_segment_ids`) + an `about` edge to the meeting note,
    /// carries owner/due only if extracted from evidence, and flips the action item to
    /// `promoted`.
    ///
    /// # Errors
    /// Returns [`AppError::NotFound`] if the action item does not exist, or a storage
    /// error if persistence fails.
    pub fn action_item_to_task(
        &self,
        service: &Service,
        action_item_id: &str,
        overrides: &ActionItemOverrides,
    ) -> AppResult<String> {
        service.action_item_to_task(action_item_id, overrides)
    }

    // -- internal stage helpers ---------------------------------------------

    /// Build the `capture-api` config from the meeting config.
    fn capture_config(&self, config: &MeetingConfig) -> CaptureConfig {
        CaptureConfig {
            targets: config
                .sources
                .iter()
                .map(|s| PlatformAppId::new(s.clone()))
                .collect(),
            capture_microphone: config.capture_microphone,
            exclude_self: config.exclude_self,
            sample_rate_hz: if config.sample_rate_hz == 0 {
                48_000
            } else {
                config.sample_rate_hz
            },
        }
    }

    /// Route the session to FAILED (terminal; the journal/rows are retained) and
    /// return the failed outcome.
    fn fail(
        &self,
        m: &mut Machine<'_>,
        session_id: SessionId,
        reason: &str,
    ) -> AppResult<SessionOutcome> {
        m.to(
            SessionState::Failed,
            Columns::new(),
            Some(reason.to_string()),
        )?;
        Ok(SessionOutcome {
            session_id: session_id.to_string(),
            state: SessionState::Failed,
            artifact: None,
            note_id: None,
            generation_path: None,
            segment_count: 0,
            action_item_count: 0,
            degraded_reason: Some(reason.to_string()),
        })
    }

    /// RECORDING loop: pull native blocks, run the DSP, run the live (pass-1) STT and
    /// stream `LiveTranscript`, and retain the recoverable chunks for the final pass.
    fn record(&self, m: &Machine<'_>, audio: &mut dyn AudioSource) -> AppResult<Vec<Chunk>> {
        let mut pipeline: Option<Pipeline> = None;
        let mut chunks: Vec<Chunk> = Vec::new();

        // Live pass runs at the fast profile (best-effort; ignore if unsupported).
        {
            let mut sp = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
            let _ = sp.set_profile(SpeechModelProfile::Fast);
        }

        while let Some(block) = audio.next_block() {
            self.service_emit_level(m, &block);
            let p = match pipeline.as_mut() {
                Some(p) => p,
                None => {
                    let cfg = PipelineConfig::new(block.sample_rate_hz, block.channels)
                        .map_err(|e| AppError::CaptureGlitch(e.to_string()))?;
                    let built =
                        Pipeline::new(cfg).map_err(|e| AppError::CaptureGlitch(e.to_string()))?;
                    pipeline.get_or_insert(built)
                }
            };
            let out = p
                .ingest(&block.interleaved)
                .map_err(|e| AppError::CaptureGlitch(e.to_string()))?;
            for chunk in out.chunks {
                self.emit_live(m, &chunk);
                chunks.push(chunk);
            }
        }
        if let Some(mut p) = pipeline {
            for chunk in p.finish().chunks {
                self.emit_live(m, &chunk);
                chunks.push(chunk);
            }
        }
        Ok(chunks)
    }

    /// Emit a throttled capture level meter for a native block (RMS in dBFS).
    fn service_emit_level(&self, m: &Machine<'_>, block: &CaptureBlock) {
        m.service.emit(AppEvent::CaptureLevel {
            session_id: m.id,
            rms_dbfs: rms_dbfs(&block.interleaved),
        });
    }

    /// Run the live pass over one chunk and stream a `LiveTranscript` per hypothesis.
    fn emit_live(&self, m: &Machine<'_>, chunk: &Chunk) {
        let mut sp = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
        let ac = AudioChunk {
            samples: chunk.samples.as_slice(),
            sample_rate_hz: media_pipeline::TARGET_SAMPLE_RATE_HZ,
            t_start_ms: chunk.t_start_ms,
        };
        for h in sp.transcribe_live(&ac) {
            m.service.emit(AppEvent::LiveTranscript {
                session_id: m.id,
                // Live rows are provisional (superseded by final); a throwaway id.
                segment: EventSegment {
                    segment_id: Id::new(),
                    t_start_ms: h.t_start_ms,
                    t_end_ms: h.t_end_ms,
                    speaker: None,
                    text: h.text,
                    pass: TranscriptPass::Live,
                    confidence: h.confidence,
                },
            });
        }
    }

    /// FINAL_TRANSCRIBING: run the authoritative (pass-2) STT over each retained chunk
    /// and persist the resulting `transcript_segment` rows (the evidence anchors).
    fn finalize(
        &self,
        service: &Service,
        session_id: SessionId,
        track_id: Id,
        chunks: &[Chunk],
    ) -> AppResult<Vec<PersistedSeg>> {
        let mut sp = self.speech.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = sp.set_profile(SpeechModelProfile::Quality);

        let mut out = Vec::new();
        let mut seq: i64 = 0;
        for chunk in chunks {
            let span = AudioSpan {
                samples: chunk.samples.as_slice(),
                sample_rate_hz: media_pipeline::TARGET_SAMPLE_RATE_HZ,
                t_start_ms: chunk.t_start_ms,
                t_end_ms: chunk.t_end_ms,
            };
            // A per-chunk decode glitch must never lose the rest of the audio; skip it.
            let segments = match sp.transcribe_final(&span) {
                Ok(s) => s,
                Err(_) => continue,
            };
            for fs in segments {
                let row = TranscriptSegmentRow {
                    id: fs.segment_id,
                    session_id,
                    track_id: Some(track_id),
                    seq,
                    t_start_ms: fs.t_start_ms,
                    t_end_ms: fs.t_end_ms,
                    speaker: fs.speaker.clone(),
                    person_id: None,
                    text: fs.text.clone(),
                    pass: "final".into(),
                    confidence: fs.confidence.map(f64::from),
                };
                service.transcript_segment_persist(&row)?;
                out.push(PersistedSeg {
                    id: fs.segment_id,
                    t_start_ms: fs.t_start_ms,
                    speaker: fs.speaker,
                    text: fs.text,
                });
                seq += 1;
            }
        }
        Ok(out)
    }

    /// GENERATING: build the constrained request, run the repair→fallback contract,
    /// resolve every fact's evidence against the persisted segments (dropping facts
    /// with no resolvable evidence), and persist the immutable `artifact` row.
    fn generate(
        &self,
        service: &Service,
        session_id: SessionId,
        segs: &[PersistedSeg],
    ) -> AppResult<Generated> {
        let prompt = build_prompt(segs);
        let req = GenerationRequest::deterministic(prompt, 1024);
        let grammar = Grammar::new(""); // the backend owns the concrete GBNF; mock ignores it
        let session = session_id;
        let seg_snapshot = segs.to_vec();

        // An LlmError here (transport/backend, not a schema violation) is mapped and
        // propagated so the caller routes GENERATING → DEGRADED, keeping the
        // transcript. Schema violations never reach here — they are absorbed by the
        // repair→deterministic-fallback contract inside `generate_structured`.
        let outcome: GenerationOutcome<MeetingArtifactV1> =
            generate_structured(&*self.llm, &req, &grammar, || {
                fallback_artifact(session, &seg_snapshot)
            })
            .map_err(|e| AppError::Generation(e.to_string()))?;

        let resolvable = service.resolvable_segments(session_id)?;
        let artifact = clean_artifact(outcome.value, session_id, &resolvable);

        let artifact_id = Id::new();
        let artifact_json = serde_json::to_string(&artifact)?;
        service.artifact_persist(
            artifact_id,
            session_id,
            self.llm.model_id().as_str(),
            &artifact_json,
        )?;

        Ok(Generated {
            artifact,
            artifact_id,
            path: outcome.path,
        })
    }
}

/// A persisted final transcript segment (the evidence anchor + display text).
#[derive(Clone, Debug)]
struct PersistedSeg {
    id: SegmentId,
    t_start_ms: i64,
    speaker: Option<String>,
    text: String,
}

/// The result of the GENERATING stage.
struct Generated {
    artifact: MeetingArtifactV1,
    artifact_id: Id,
    path: GenerationPath,
}

// ===========================================================================
// Pure helpers
// ===========================================================================

/// Root-mean-square level of an interleaved block, in dBFS (−∞ clamped to −120).
fn rms_dbfs(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return -120.0;
    }
    let sum_sq: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    let rms = (sum_sq / samples.len() as f64).sqrt();
    if rms <= 1e-9 {
        -120.0
    } else {
        (20.0 * rms.log10()) as f32
    }
}

/// The capture-source descriptor persisted on `session.capture_source` (Data Model
/// §8.1): honest booleans + the chosen targets. No PCM, ever.
fn capture_source_json(config: &MeetingConfig, caps: &CaptureCapabilities) -> String {
    json!({
        "app_audio": !config.sources.is_empty(),
        "mic": config.capture_microphone,
        "exclude_self": config.exclude_self && caps.exclude_self,
        "targets": config.sources,
        "sample_rate_hz": config.sample_rate_hz,
    })
    .to_string()
}

/// Build the GENERATING prompt: an instruction plus a machine-readable transcript
/// manifest so the constrained decoder cites only real `segment_id`s from evidence.
fn build_prompt(segs: &[PersistedSeg]) -> String {
    let manifest: Vec<Value> = segs
        .iter()
        .map(|s| {
            json!({
                "segment_id": s.id.to_string(),
                "t_start_ms": s.t_start_ms,
                "speaker": s.speaker,
                "text": s.text,
            })
        })
        .collect();
    format!(
        "Summarize this meeting as a MeetingArtifactV1. Every fact MUST carry \
         evidence_segment_ids drawn ONLY from the transcript below. Do not invent \
         owners, dates, or citations.\nTRANSCRIPT_SEGMENTS_JSON:\n{}",
        serde_json::to_string(&manifest).unwrap_or_else(|_| "[]".into())
    )
}

/// The deterministic fallback (Data Model §14.1 "topics-only"): one topic citing the
/// whole transcript, executive summary from the first segment. Invents nothing; if
/// there are no segments it returns the empty artifact.
fn fallback_artifact(session_id: SessionId, segs: &[PersistedSeg]) -> MeetingArtifactV1 {
    if segs.is_empty() {
        return MeetingArtifactV1::empty(session_id);
    }
    let evidence: Vec<SegmentId> = segs.iter().map(|s| s.id).collect();
    let mut a = MeetingArtifactV1::empty(session_id);
    a.executive_summary = segs.first().map(|s| s.text.clone()).unwrap_or_default();
    a.topics = vec![Topic {
        title: "Meeting transcript".into(),
        summary: "Automatically summarized transcript (topics-only fallback).".into(),
        evidence_segment_ids: evidence,
    }];
    a
}

/// Enforce "evidence or nothing": keep only facts whose evidence resolves to a real
/// persisted segment, filtering each fact's `evidence_segment_ids` to the resolvable
/// subset and dropping facts left with none. Also stamps the real `session_id`.
fn clean_artifact(
    mut a: MeetingArtifactV1,
    session_id: SessionId,
    resolvable: &HashSet<SegmentId>,
) -> MeetingArtifactV1 {
    a.schema = MeetingArtifactV1::SCHEMA.to_string();
    a.session_id = session_id;

    let keep = |ev: &mut Vec<SegmentId>| {
        ev.retain(|id| resolvable.contains(id));
        !ev.is_empty()
    };
    a.topics.retain_mut(|t| keep(&mut t.evidence_segment_ids));
    a.decisions
        .retain_mut(|d| keep(&mut d.evidence_segment_ids));
    a.action_items
        .retain_mut(|i| !i.task.trim().is_empty() && keep(&mut i.evidence_segment_ids));
    a.risks.retain_mut(|r| keep(&mut r.evidence_segment_ids));
    a.open_questions
        .retain_mut(|q| keep(&mut q.evidence_segment_ids));
    a
}

/// Map a `capture-api` error into the app error taxonomy.
fn map_capture_err(e: CaptureError) -> AppError {
    match e {
        CaptureError::PermissionDenied(m) | CaptureError::PermissionRequired(m) => {
            AppError::Permission(m)
        }
        CaptureError::Glitch(m) => AppError::CaptureGlitch(m),
        other => AppError::Capability(other.to_string()),
    }
}

// ===========================================================================
// Service — persistence + reads for the meeting pillar
// ===========================================================================

impl Service {
    /// Create the `session` entity (spine `kind='session'`) at state `NEW`.
    fn session_create(
        &self,
        id: SessionId,
        platform: Platform,
        capture_source: &str,
        title: Option<String>,
    ) -> AppResult<()> {
        let now = self.now_ms();
        let mut cols = Columns::new();
        cols.insert(
            "state".into(),
            Value::String(SessionState::New.as_str().into()),
        );
        cols.insert(
            "platform".into(),
            Value::String(platform_str(platform).into()),
        );
        cols.insert(
            "capture_source".into(),
            Value::String(capture_source.to_string()),
        );
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "session",
            title,
            None,
            now,
            Some((DetailTable::Session, cols)),
        ))
    }

    /// Patch `session` detail columns (state and friends), preserving the spine.
    fn session_update(&self, id: SessionId, cols: Columns) -> AppResult<()> {
        let now = self.now_ms();
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("session {id}")))?;
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            None,
            now,
            Some((DetailTable::Session, cols)),
        ))
    }

    /// Persist an `audio_track` row through the op-log; returns its id.
    fn audio_track_persist(&self, track: &AudioTrackRow) -> AppResult<Id> {
        self.commit(&EntityOp::new(
            track.session_id,
            self.next_hlc(),
            OpBody::AudioTrackSet {
                track: track.clone(),
            },
        ))?;
        Ok(track.id)
    }

    /// Persist one `transcript_segment` row through the op-log.
    fn transcript_segment_persist(&self, seg: &TranscriptSegmentRow) -> AppResult<()> {
        self.commit(&EntityOp::new(
            seg.session_id,
            self.next_hlc(),
            OpBody::TranscriptSegmentSet {
                segment: seg.clone(),
            },
        ))
    }

    /// The set of persisted final `transcript_segment` ids for a session (evidence
    /// resolution target).
    fn resolvable_segments(&self, session_id: SessionId) -> AppResult<HashSet<Id>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT id FROM transcript_segment WHERE session_id = ?1 AND pass = 'final'",
            )?;
            let rows = stmt
                .query_map(params![session_id.as_bytes().as_slice()], |r| {
                    r.get::<_, Vec<u8>>(0)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(|b| Id::from_bytes(to16(&b))).collect())
        })
    }

    /// Persist the immutable-per-generation `artifact` entity (`kind='artifact'`).
    fn artifact_persist(
        &self,
        artifact_id: Id,
        session_id: SessionId,
        llm_model: &str,
        artifact_json: &str,
    ) -> AppResult<()> {
        let now = self.now_ms();
        let mut cols = Columns::new();
        cols.insert("session_id".into(), Value::String(session_id.to_string()));
        cols.insert("schema_version".into(), Value::Number(1.into()));
        cols.insert("generation".into(), Value::Number(1.into()));
        cols.insert("is_current".into(), Value::Number(1.into()));
        cols.insert("llm_model".into(), Value::String(llm_model.to_string()));
        cols.insert(
            "artifact_json".into(),
            Value::String(artifact_json.to_string()),
        );
        cols.insert("generated_at".into(), Value::Number(now.into()));
        self.commit(&util::create_op(
            artifact_id,
            self.next_hlc(),
            "artifact",
            None,
            None,
            now,
            Some((DetailTable::Artifact, cols)),
        ))
    }

    /// INDEXING: write the meeting into the unified spine — the meeting-as-note (from
    /// the artifact), the `note → session` provenance edge, and the suggested
    /// `action_item` rows. Returns the note id.
    fn index_meeting(
        &self,
        session_id: SessionId,
        artifact_id: Id,
        artifact: &MeetingArtifactV1,
        segs: &[PersistedSeg],
    ) -> AppResult<Id> {
        self.emit(AppEvent::IndexingProgress {
            session_id,
            stage: "note".into(),
            pct: 0.3,
        });

        // 1) The meeting becomes a note.
        let title = self
            .read(|c| Ok(session_title(c, session_id)?))?
            .unwrap_or_else(|| "Meeting".to_string());
        let doc_json = meeting_doc_json(artifact, segs);
        let note = self.create_note(title, Some(doc_json))?;
        let note_id = parse_id(&note.id)?;

        // 2) Provenance edge note → session (survives later edits; graph-visible).
        self.link_meeting(note_id, session_id, LinkRel::About, None, "meeting")?;

        // 3) Suggested action items (the projected, actionable extraction).
        self.emit(AppEvent::IndexingProgress {
            session_id,
            stage: "action_items".into(),
            pct: 0.7,
        });
        for (idx, ai) in artifact.action_items.iter().enumerate() {
            self.action_item_persist(&NewActionItem {
                artifact_id,
                session_id,
                idx: idx as i64,
                item: ai,
            })?;
        }

        self.emit(AppEvent::IndexingProgress {
            session_id,
            stage: "complete".into(),
            pct: 1.0,
        });
        Ok(note_id)
    }

    /// Persist one suggested `action_item` entity (`kind='action_item'`).
    fn action_item_persist(&self, new: &NewActionItem<'_>) -> AppResult<Id> {
        let now = self.now_ms();
        let id = Id::new();
        let evidence: Vec<String> = new
            .item
            .evidence_segment_ids
            .iter()
            .map(ToString::to_string)
            .collect();
        let mut cols = Columns::new();
        cols.insert(
            "artifact_id".into(),
            Value::String(new.artifact_id.to_string()),
        );
        cols.insert(
            "session_id".into(),
            Value::String(new.session_id.to_string()),
        );
        cols.insert("idx".into(), Value::Number(new.idx.into()));
        cols.insert("task_text".into(), Value::String(new.item.task.clone()));
        cols.insert(
            "evidence_segment_ids".into(),
            Value::String(serde_json::to_string(&evidence)?),
        );
        cols.insert("status".into(), Value::String("suggested".into()));
        if let Some(owner) = &new.item.owner {
            cols.insert("owner_text".into(), Value::String(owner.clone()));
        }
        if let Some(due) = &new.item.due_date {
            cols.insert("due_date".into(), Value::String(due.to_string()));
        }
        let title: String = new.item.task.chars().take(120).collect();
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "action_item",
            Some(title),
            None,
            now,
            Some((DetailTable::ActionItem, cols)),
        ))?;
        Ok(id)
    }

    /// The cross-pillar bridge (Data Model §8.5) — promote a suggested action item to
    /// a Task with `spawned_from` provenance (copied evidence) + `about` edge.
    fn action_item_to_task(
        &self,
        action_item_id: &str,
        overrides: &ActionItemOverrides,
    ) -> AppResult<String> {
        let ai_id = parse_id(action_item_id)?;
        let ai = self
            .read(|c| Ok(read_action_item(c, ai_id)?))?
            .ok_or_else(|| AppError::NotFound(format!("action_item {action_item_id}")))?;

        // Idempotent: if already promoted, return the existing task.
        if let Some(existing) = ai.promoted_task_id {
            return Ok(existing);
        }

        let title = overrides.title.clone().unwrap_or(ai.task_text);
        let deadline_on = overrides.deadline_on.clone().or(ai.due_date); // only if extracted
        let new = NewTask {
            title,
            project_id: overrides.project_id.clone(),
            area_id: overrides.area_id.clone(),
            notes_md: None,
            start_on: None,
            deadline_on,
            someday: None,
            priority: None,
        };
        let view = self.tasks_create(new)?; // emits TaskChanged
        let task_id = parse_id(&view.id)?;
        let session_id = parse_id(&ai.session_id)?;

        // Provenance rides the edge (survives later task edits): task → session.
        self.link_meeting(
            task_id,
            session_id,
            LinkRel::SpawnedFrom,
            Some(ai.evidence_json.clone()),
            "meeting",
        )?;
        // If the meeting has a note, also relate the task to it.
        if let Some(note_id) = self.read(|c| Ok(session_note_id(c, session_id)?))? {
            self.link_meeting(task_id, note_id, LinkRel::About, None, "meeting")?;
        }

        // Flip the action item to promoted.
        let spine = self
            .read(|c| Ok(util::read_spine(c, ai_id)?))?
            .ok_or_else(|| AppError::NotFound(format!("action_item {action_item_id}")))?;
        let mut cols = Columns::new();
        cols.insert(
            "promoted_task_id".into(),
            Value::String(task_id.to_string()),
        );
        cols.insert("status".into(), Value::String("promoted".into()));
        self.commit(&util::update_op(
            ai_id,
            self.next_hlc(),
            &spine,
            None,
            self.now_ms(),
            Some((DetailTable::ActionItem, cols)),
        ))?;

        Ok(task_id.to_string())
    }

    /// Upsert a meeting-provenance `link` edge through the op-log.
    fn link_meeting(
        &self,
        src: Id,
        dst: Id,
        rel: LinkRel,
        evidence_json: Option<String>,
        origin: &str,
    ) -> AppResult<()> {
        let now = self.now_ms();
        let hlc = self.next_hlc();
        let row = LinkRow {
            id: Id::new(),
            src_entity: src,
            dst_entity: dst,
            rel: rel.as_str().to_string(),
            src_block_id: None,
            dst_block_id: None,
            evidence_segment_ids: evidence_json,
            data_json: None,
            origin: origin.to_string(),
            created_at: now,
            hlc: hlc.to_string(),
        };
        self.commit(&EntityOp::new(src, hlc, OpBody::LinkSet { link: row }))
    }

    /// `meeting.get` — a `session` row projection.
    pub fn session_get(&self, session_id: &str) -> AppResult<SessionView> {
        let id = parse_id(session_id)?;
        self.read(|c| {
            c.query_row(
                "SELECT state, note_id, started_at, ended_at, duration_ms, platform, degraded_reason \
                 FROM session WHERE entity_id = ?1",
                params![id.as_bytes().as_slice()],
                |r| {
                    Ok(SessionView {
                        id: id.to_string(),
                        state: r.get(0)?,
                        note_id: r
                            .get::<_, Option<Vec<u8>>>(1)?
                            .map(|b| Id::from_bytes(to16(&b)).to_string()),
                        started_at: r.get(2)?,
                        ended_at: r.get(3)?,
                        duration_ms: r.get(4)?,
                        platform: r.get(5)?,
                        degraded_reason: r.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
        })?
        .ok_or_else(|| AppError::NotFound(format!("session {session_id}")))
    }

    /// The suggested action items for a session (the review surface).
    pub fn session_action_items(&self, session_id: &str) -> AppResult<Vec<ActionItemView>> {
        let id = parse_id(session_id)?;
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT ai.entity_id, ai.idx, ai.task_text, ai.owner_text, ai.due_date, \
                        ai.evidence_segment_ids, ai.status, ai.promoted_task_id \
                 FROM action_item ai JOIN entity e ON e.id = ai.entity_id \
                 WHERE ai.session_id = ?1 AND e.deleted_at IS NULL \
                 ORDER BY ai.idx",
            )?;
            let rows = stmt
                .query_map(params![id.as_bytes().as_slice()], |r| {
                    let ev: String = r.get(5)?;
                    Ok(ActionItemView {
                        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)).to_string(),
                        idx: r.get(1)?,
                        task_text: r.get(2)?,
                        owner_text: r.get(3)?,
                        due_date: r.get(4)?,
                        evidence_segment_ids: serde_json::from_str(&ev).unwrap_or_default(),
                        status: r.get(6)?,
                        promoted_task_id: r
                            .get::<_, Option<Vec<u8>>>(7)?
                            .map(|b| Id::from_bytes(to16(&b)).to_string()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }
}

/// Inputs for persisting one suggested action item.
struct NewActionItem<'a> {
    artifact_id: Id,
    session_id: SessionId,
    idx: i64,
    item: &'a ActionItem,
}

/// A read-back `action_item` row for the bridge.
struct ActionItemRow {
    task_text: String,
    due_date: Option<String>,
    evidence_json: String,
    session_id: String,
    promoted_task_id: Option<String>,
}

fn read_action_item(
    conn: &rusqlite::Connection,
    id: Id,
) -> rusqlite::Result<Option<ActionItemRow>> {
    conn.query_row(
        "SELECT task_text, due_date, evidence_segment_ids, session_id, promoted_task_id \
         FROM action_item WHERE entity_id = ?1",
        params![id.as_bytes().as_slice()],
        |r| {
            Ok(ActionItemRow {
                task_text: r.get(0)?,
                due_date: r.get(1)?,
                evidence_json: r.get(2)?,
                session_id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(3)?)).to_string(),
                promoted_task_id: r
                    .get::<_, Option<Vec<u8>>>(4)?
                    .map(|b| Id::from_bytes(to16(&b)).to_string()),
            })
        },
    )
    .optional()
}

fn session_note_id(conn: &rusqlite::Connection, session_id: Id) -> rusqlite::Result<Option<Id>> {
    conn.query_row(
        "SELECT note_id FROM session WHERE entity_id = ?1",
        params![session_id.as_bytes().as_slice()],
        |r| r.get::<_, Option<Vec<u8>>>(0),
    )
    .optional()
    .map(|o| o.flatten().map(|b| Id::from_bytes(to16(&b))))
}

fn session_title(conn: &rusqlite::Connection, session_id: Id) -> rusqlite::Result<Option<String>> {
    conn.query_row(
        "SELECT title FROM entity WHERE id = ?1",
        params![session_id.as_bytes().as_slice()],
        |r| r.get::<_, Option<String>>(0),
    )
    .optional()
    .map(Option::flatten)
}

/// Build the meeting-as-note `doc_json` (Tiptap): the executive summary followed by
/// each final transcript segment as a paragraph. Evidence lives in the segments/
/// artifact; this is the human-readable projection.
fn meeting_doc_json(artifact: &MeetingArtifactV1, segs: &[PersistedSeg]) -> String {
    let mut content: Vec<Value> = Vec::new();
    if !artifact.executive_summary.trim().is_empty() {
        content.push(json!({
            "type": "heading",
            "attrs": { "level": 2 },
            "content": [{ "type": "text", "text": "Summary" }]
        }));
        content.push(json!({
            "type": "paragraph",
            "content": [{ "type": "text", "text": artifact.executive_summary }]
        }));
    }
    if !segs.is_empty() {
        content.push(json!({
            "type": "heading",
            "attrs": { "level": 2 },
            "content": [{ "type": "text", "text": "Transcript" }]
        }));
        for s in segs {
            let speaker = s.speaker.clone().unwrap_or_else(|| "Speaker".into());
            content.push(json!({
                "type": "paragraph",
                "content": [{ "type": "text", "text": format!("{speaker}: {}", s.text) }]
            }));
        }
    }
    if content.is_empty() {
        content.push(json!({ "type": "paragraph" }));
    }
    json!({ "type": "doc", "content": content }).to_string()
}

/// The observability name of a repair→fallback generation path.
fn path_name(path: GenerationPath) -> &'static str {
    match path {
        GenerationPath::Direct => "direct",
        GenerationPath::Repaired => "repaired",
        GenerationPath::DeterministicFallback => "deterministic_fallback",
    }
}

fn platform_str(p: Platform) -> &'static str {
    match p {
        Platform::Macos => "macos",
        Platform::Windows => "windows",
        Platform::Linux => "linux",
    }
}

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitions_are_exhaustive_and_reject_illegal() {
        use SessionState as S;
        // Happy path is legal end to end.
        let happy = [
            S::New,
            S::Preflight,
            S::Ready,
            S::Recording,
            S::Stopping,
            S::Captured,
            S::FinalTranscribing,
            S::Generating,
            S::Indexing,
            S::Complete,
        ];
        for w in happy.windows(2) {
            assert!(legal_transition(w[0], w[1]), "{:?}->{:?}", w[0], w[1]);
        }
        // The LLM never owns recording state: Generating degrades, never rewinds.
        assert!(legal_transition(S::Generating, S::Degraded));
        assert!(!legal_transition(S::Generating, S::Recording));
        // Terminal states have no exits.
        assert!(!legal_transition(S::Complete, S::Indexing));
        assert!(!legal_transition(S::Failed, S::Recovering));
        // Degraded is recoverable.
        assert!(legal_transition(S::Degraded, S::Recovering));
        assert!(legal_transition(S::Recovering, S::Generating));
        // A random illegal jump is rejected.
        assert!(!legal_transition(S::New, S::Complete));
        assert!(!legal_transition(S::Ready, S::Generating));
    }

    #[test]
    fn clean_artifact_drops_unresolved_evidence() {
        let session = Id::new();
        let good: SegmentId = Id::new();
        let bad: SegmentId = Id::new();
        let mut resolvable = HashSet::new();
        resolvable.insert(good);

        let mut a = MeetingArtifactV1::empty(session);
        a.topics = vec![
            Topic {
                title: "kept".into(),
                summary: "has real evidence".into(),
                evidence_segment_ids: vec![good, bad],
            },
            Topic {
                title: "dropped".into(),
                summary: "no real evidence".into(),
                evidence_segment_ids: vec![bad],
            },
        ];
        a.action_items = vec![ActionItem {
            task: "do it".into(),
            owner: None,
            due_date: None,
            evidence_segment_ids: vec![bad],
        }];

        let cleaned = clean_artifact(a, session, &resolvable);
        assert_eq!(cleaned.topics.len(), 1, "only the evidenced topic survives");
        assert_eq!(cleaned.topics[0].evidence_segment_ids, vec![good]);
        assert!(
            cleaned.action_items.is_empty(),
            "an action item with no resolvable evidence is dropped"
        );
    }
}

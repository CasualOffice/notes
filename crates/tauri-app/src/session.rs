//! The M2 meeting-intelligence command surface + the host-side session runner.
//!
//! This module is the WebView↔Core door for the meeting pillar (HLD §6 `meeting.*`,
//! §7 event model, §8.4 pipeline). It exposes the discrete session commands the UI
//! drives — `run_preflight`, `start_session`, `pause_session`, `resume_session`,
//! `stop_session`, `cancel_job`, `regenerate_artifact`, plus `list_capture_apps` and
//! the action-item→Task bridge — and delegates each to the `app-service`
//! [`SessionCoordinator`], which owns transactions and `AppEvent` emission.
//!
//! ## Why a host-side runner
//! `app-service`'s coordinator drives a whole meeting `NEW → COMPLETE` synchronously
//! off an injected [`AudioSource`] (the native-capture trait seam). The UI, however,
//! wants *discrete* start/pause/resume/stop control. This module provides exactly the
//! mock capture-threading the coordinator's own docs anticipate: [`start_session`]
//! launches `run_session` on a background thread fed by a [`ControllableAudioSource`],
//! and pause/resume/stop/cancel cooperatively steer that source. The engines default
//! to the **mock** doubles ([`SessionCoordinator::with_mocks`]) so the flow runs with
//! no native capture / whisper / llama backends.
//!
//! ## Invariants honoured
//! - **The LLM never owns recording state.** Recording is steered here; generation
//!   runs entirely inside the coordinator and can only *degrade* the session.
//! - **PCM never crosses IPC.** The controllable source produces PCM on the session
//!   thread and hands it to the DSP as `&[f32]`; only metadata + evidence-resolved
//!   artifacts ever reach the WebView (as `AppEvent`s / typed command returns).
//! - **Events carry `session_id` + a monotonic `seq`.** Every `AppEvent` the
//!   coordinator emits flows through the single `EventSink` (installed in `lib.rs`) to
//!   the WebView's `"app-event"` channel; the UI demuxes per `session_id` and uses
//!   `seq` for gap detection (HLD §7).

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use app_domain::{AppError, Id};
use app_service::{
    stubs, ActionItemOverrides, ActionItemView, AudioSource, CaptureBlock, MeetingConfig,
    PreflightReport, Service, SessionCoordinator, SessionOutcome, SessionView,
};
use capture_api::{ApplicationCaptureAdapter, CapturableApp};
use tauri::State;

// ===========================================================================
// Controllable mock capture source (the discrete-control seam)
// ===========================================================================

/// Cooperative control flags shared between a running session thread's
/// [`ControllableAudioSource`] and the [`SessionManager`] commands that steer it.
#[derive(Debug, Default)]
struct JobControl {
    /// While set, the source blocks (holds capture, emits nothing new).
    paused: AtomicBool,
    /// A user `stop` — capture ends cleanly and the pipeline drains to `COMPLETE`.
    stop: AtomicBool,
    /// A user `cancel` — capture ends and the (discarded) outcome is drained off.
    cancelled: AtomicBool,
}

impl JobControl {
    fn ending(&self) -> bool {
        self.stop.load(Ordering::SeqCst) || self.cancelled.load(Ordering::SeqCst)
    }
}

/// The mock capture ring: an [`AudioSource`] that yields deterministic speech-VAD
/// friendly tone blocks until the user stops/cancels (or a safety cap is reached), and
/// honours pause by parking. This is the stand-in for the native `capture-*` ring; the
/// real backend plugs into the identical [`AudioSource`] seam.
struct ControllableAudioSource {
    control: Arc<JobControl>,
    /// Pre-seeded head blocks (kept small); further blocks are synthesised lazily so
    /// memory stays bounded regardless of meeting length.
    seeded: VecDeque<CaptureBlock>,
    emitted: u32,
    /// Safety cap on synthesised blocks so a forgotten session cannot record forever.
    max_blocks: u32,
    rate: u32,
    block_ms: u32,
}

impl ControllableAudioSource {
    /// 16 kHz mono, 0.5 s blocks, capped at ~10 min of mock audio.
    fn new(control: Arc<JobControl>) -> Self {
        Self {
            control,
            seeded: VecDeque::new(),
            emitted: 0,
            max_blocks: 1_200,
            rate: 16_000,
            block_ms: 500,
        }
    }

    /// Synthesize one 220 Hz tone block (deterministic; drives the VAD/DSP so the
    /// mock speech engine yields at least one final segment per chunk).
    fn synth(&self) -> CaptureBlock {
        let per_block = ((u64::from(self.rate) * u64::from(self.block_ms)) / 1000) as usize;
        let base = self.emitted as usize * per_block;
        let mut interleaved = Vec::with_capacity(per_block);
        for n in 0..per_block {
            let t = (base + n) as f64;
            let s = 0.6 * (2.0 * std::f64::consts::PI * 220.0 * t / f64::from(self.rate)).sin();
            interleaved.push(s as f32);
        }
        CaptureBlock {
            interleaved,
            sample_rate_hz: self.rate,
            channels: 1,
        }
    }
}

impl AudioSource for ControllableAudioSource {
    fn next_block(&mut self) -> Option<CaptureBlock> {
        loop {
            // Stop/cancel wins over everything: end capture so the coordinator drains
            // STOPPING → CAPTURED → … deterministically.
            if self.control.ending() {
                return None;
            }
            if self.control.paused.load(Ordering::SeqCst) {
                // Park briefly; recording is held without spinning a core.
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            if let Some(b) = self.seeded.pop_front() {
                self.emitted += 1;
                return Some(b);
            }
            if self.emitted >= self.max_blocks {
                return None; // safety cap reached — end the mock meeting
            }
            let b = self.synth();
            self.emitted += 1;
            return Some(b);
        }
    }
}

// ===========================================================================
// Session manager (managed Tauri state)
// ===========================================================================

/// One in-flight (or finished-awaiting-collection) session driven on a background
/// thread. The `handle` yields the coordinator's [`SessionOutcome`] on join.
struct Job {
    control: Arc<JobControl>,
    handle: Option<JoinHandle<Result<SessionOutcome, AppError>>>,
}

/// Host-side registry of running meeting sessions plus the shared mock coordinator.
/// Managed as `Arc<SessionManager>` Tauri state (see `lib.rs`).
pub struct SessionManager {
    coordinator: Arc<SessionCoordinator>,
    service: Arc<Service>,
    jobs: Mutex<HashMap<String, Job>>,
}

impl SessionManager {
    /// Build a manager over the mock coordinator (mock capture + mock speech + a
    /// deterministic-fallback LLM), sharing the host's single [`Service`].
    ///
    /// # Errors
    /// Propagates a runtime-build failure from [`SessionCoordinator::with_mocks`].
    pub fn new_with_mocks(service: Arc<Service>) -> Result<Self, AppError> {
        Ok(Self {
            coordinator: Arc::new(SessionCoordinator::with_mocks()?),
            service,
            jobs: Mutex::new(HashMap::new()),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.jobs
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// `meeting.preflight` — honest capability + permission report (never records).
    fn preflight(&self, sources: Vec<String>) -> Result<PreflightReport, AppError> {
        self.coordinator.preflight(sources)
    }

    /// `meeting.start` — arm the mock capture ring and drive `NEW → …` on a worker
    /// thread; returns an opaque `job_id` the discrete controls address. The session's
    /// own `SessionId` reaches the UI on the first `SessionStateChanged` event and in
    /// the terminal [`SessionOutcome`] returned by [`SessionManager::stop`].
    fn start(&self, config: MeetingConfig) -> Result<String, AppError> {
        let job_id = Id::new().to_string();
        let control = Arc::new(JobControl::default());

        let coordinator = self.coordinator.clone();
        let service = self.service.clone();
        let thread_ctl = control.clone();
        let handle = std::thread::Builder::new()
            .name(format!("meeting-{job_id}"))
            .spawn(move || {
                let mut audio = ControllableAudioSource::new(thread_ctl);
                coordinator.run_session(&service, &config, &mut audio)
            })
            .map_err(|e| AppError::Internal(format!("spawn session thread: {e}")))?;

        self.lock().insert(
            job_id.clone(),
            Job {
                control,
                handle: Some(handle),
            },
        );
        Ok(job_id)
    }

    /// `meeting.pause` — hold the mock capture ring (recording is retained).
    fn pause(&self, job_id: &str) -> Result<(), AppError> {
        self.with_control(job_id, |c| c.paused.store(true, Ordering::SeqCst))
    }

    /// `meeting.resume` — release a held capture ring.
    fn resume(&self, job_id: &str) -> Result<(), AppError> {
        self.with_control(job_id, |c| c.paused.store(false, Ordering::SeqCst))
    }

    /// `meeting.stop` — end capture and drain the pipeline to its terminal state,
    /// returning the coordinator's [`SessionOutcome`] (artifact + note + counts).
    fn stop(&self, job_id: &str) -> Result<SessionOutcome, AppError> {
        let job = self.take(job_id)?;
        job.control.paused.store(false, Ordering::SeqCst);
        job.control.stop.store(true, Ordering::SeqCst);
        Self::join(job)
    }

    /// `meeting.cancel` — end capture and drain, discarding the outcome.
    fn cancel(&self, job_id: &str) -> Result<(), AppError> {
        let job = self.take(job_id)?;
        job.control.paused.store(false, Ordering::SeqCst);
        job.control.cancelled.store(true, Ordering::SeqCst);
        let _ = Self::join(job)?;
        Ok(())
    }

    /// The action-item → Task bridge (`meeting.actionItemToTask`): writes the
    /// `spawned_from` (evidence-carrying) + `about` provenance edges and promotes the
    /// item. Returns the new `TaskId`.
    fn action_item_to_task(
        &self,
        action_item_id: &str,
        overrides: ActionItemOverrides,
    ) -> Result<String, AppError> {
        self.coordinator
            .action_item_to_task(&self.service, action_item_id, &overrides)
    }

    fn with_control(&self, job_id: &str, f: impl FnOnce(&JobControl)) -> Result<(), AppError> {
        let jobs = self.lock();
        let job = jobs
            .get(job_id)
            .ok_or_else(|| AppError::NotFound(format!("session job {job_id}")))?;
        f(&job.control);
        Ok(())
    }

    fn take(&self, job_id: &str) -> Result<Job, AppError> {
        self.lock()
            .remove(job_id)
            .ok_or_else(|| AppError::NotFound(format!("session job {job_id}")))
    }

    fn join(mut job: Job) -> Result<SessionOutcome, AppError> {
        let handle = job
            .handle
            .take()
            .ok_or_else(|| AppError::Internal("session job already collected".into()))?;
        handle
            .join()
            .map_err(|_| AppError::Internal("session thread panicked".into()))?
    }
}

// ===========================================================================
// Command surface — the WebView-facing #[tauri::command]s
// ===========================================================================
//
// `State` MUST appear literally in each signature — the `#[tauri::command]` macro
// recognizes it by path segment (a type alias would be misread as an argument).

/// `meeting.listApplications` — enumerate the applications available as capture
/// sources (mock: a single stand-in app until a native `capture-*` backend is wired).
#[tauri::command]
pub async fn list_capture_apps() -> Result<Vec<CapturableApp>, AppError> {
    // The mock adapter mirrors the coordinator's default capture engine.
    app_service::MockCaptureAdapter
        .list_applications()
        .await
        .map_err(|e| AppError::Capability(e.to_string()))
}

/// `meeting.preflight` — honest capability + permission report gating the arm
/// affordance (capability honesty; never records).
#[tauri::command]
pub fn run_preflight(
    manager: State<'_, Arc<SessionManager>>,
    sources: Vec<String>,
) -> Result<PreflightReport, AppError> {
    manager.preflight(sources)
}

/// `meeting.start` — arm mock capture and begin driving the session; returns the
/// `job_id` used by the discrete controls below.
#[tauri::command]
pub fn start_session(
    manager: State<'_, Arc<SessionManager>>,
    config: MeetingConfig,
) -> Result<String, AppError> {
    manager.start(config)
}

/// `meeting.pause` — hold capture for `job_id` (recording retained).
#[tauri::command]
pub fn pause_session(
    manager: State<'_, Arc<SessionManager>>,
    job_id: String,
) -> Result<(), AppError> {
    manager.pause(&job_id)
}

/// `meeting.resume` — release a held capture for `job_id`.
#[tauri::command]
pub fn resume_session(
    manager: State<'_, Arc<SessionManager>>,
    job_id: String,
) -> Result<(), AppError> {
    manager.resume(&job_id)
}

/// `meeting.stop` — end capture, drain to terminal state, return the outcome
/// (artifact + meeting-as-note + action-item counts).
#[tauri::command]
pub fn stop_session(
    manager: State<'_, Arc<SessionManager>>,
    job_id: String,
) -> Result<SessionOutcome, AppError> {
    manager.stop(&job_id)
}

/// `meeting.cancel` — end capture and drain, discarding the outcome.
#[tauri::command]
pub fn cancel_job(manager: State<'_, Arc<SessionManager>>, job_id: String) -> Result<(), AppError> {
    manager.cancel(&job_id)
}

/// `meeting.regenerate` — re-run artifact generation for an existing session.
///
/// The `app-service` coordinator exposes no regeneration seam in this phase (audio
/// PCM is never persisted, so regeneration must run inside the pipeline over the
/// retained transcript). Rather than fabricate a second, divergent generation path in
/// the host, this reports a typed capability error (capability honesty) until the
/// seam lands in `app-service`.
#[tauri::command]
pub fn regenerate_artifact(
    _manager: State<'_, Arc<SessionManager>>,
    _session_id: String,
) -> Result<serde_json::Value, AppError> {
    Err(stubs::not_implemented("meeting.regenerate"))
}

/// `meeting.get` — the `session` row projection (state / note binding / timing).
#[tauri::command]
pub fn get_session(
    service: State<'_, Arc<Service>>,
    session_id: String,
) -> Result<SessionView, AppError> {
    service.session_get(&session_id)
}

/// The suggested `action_item` review surface for a session.
#[tauri::command]
pub fn list_action_items(
    service: State<'_, Arc<Service>>,
    session_id: String,
) -> Result<Vec<ActionItemView>, AppError> {
    service.session_action_items(&session_id)
}

/// `meeting.actionItemToTask` — promote a suggested action item to a Task, writing
/// `spawned_from` (evidence-carrying) + `about` provenance edges. Returns the TaskId.
#[tauri::command]
pub fn action_item_to_task(
    manager: State<'_, Arc<SessionManager>>,
    action_item_id: String,
    overrides: Option<ActionItemOverrides>,
) -> Result<String, AppError> {
    manager.action_item_to_task(&action_item_id, overrides.unwrap_or_default())
}

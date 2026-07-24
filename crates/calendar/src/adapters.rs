//! Native (Tier A) adapter **stubs** and a [`MockSyncAdapter`] test double.
//!
//! See `docs/casual-note-calendar.md` §3 (three tiers) and §8 (delivery: "engine
//! trait first, native backend second", the same pattern as audio capture / STT).
//!
//! The three native adapters — macOS **EventKit**, Linux **Evolution-Data-Server**,
//! Windows **AppointmentManager** — are present as types that **compile on every
//! platform** so the trait surface and capability reporting are exercised
//! everywhere. The real OS FFI is deferred behind these seams. Crucially there is
//! **no silent downgrade** (doc §3, §9): while the FFI backend is unimplemented,
//! [`capability`](CalendarSyncAdapter::capability) reports
//! [`Unavailable`](CalendarCapability::Unavailable) and every operation returns an
//! honest [`SyncError::Unsupported`] — the app-service then offers CalDAV/ICS.
//! Each adapter also advertises its
//! [`planned_capability`](NativeBackend::planned_capability) so the UI's
//! per-platform "capability honesty" banner can state what the native tier *will*
//! provide once wired, and on which OS it is even applicable.

use crate::error::{SyncError, SyncResult};
use crate::sync::{
    CalId, CalendarCapability, CalendarSyncAdapter, ChangeSet, EventOp, PushResult, RemoteCalendar,
    SyncToken,
};

/// Shared behavior of a native backend stub: it knows the OS it targets, the
/// capability it will expose once its FFI lands, and whether that FFI is present.
pub trait NativeBackend {
    /// The tier this backend will provide once its native FFI is implemented
    /// *and* it is running on its target OS (for the UI capability banner).
    fn planned_capability(&self) -> CalendarCapability;

    /// Whether the real OS FFI backend is compiled/available in this build.
    /// Deferred everywhere for now, so this is `false`.
    fn backend_ready(&self) -> bool {
        false
    }

    /// Whether this build is running on the backend's target OS.
    fn on_target_os(&self) -> bool;

    /// A short, human-readable status for logs / the capability banner.
    fn status(&self) -> &'static str;
}

/// The honest current capability for any native stub: `Unavailable` until both
/// the FFI backend is ready and we are on its target OS. No silent downgrade.
fn current_capability(b: &impl NativeBackend) -> CalendarCapability {
    if b.backend_ready() && b.on_target_os() {
        b.planned_capability()
    } else {
        CalendarCapability::Unavailable
    }
}

/// Uniform "not yet implemented" error for a deferred native op.
fn not_implemented(name: &str) -> SyncError {
    SyncError::Unsupported(format!(
        "native calendar backend '{name}' is not yet implemented (FFI deferred; \
         use CalDAV or ICS) — docs/casual-note-calendar.md §3, §8"
    ))
}

// ---------------------------------------------------------------------------
// macOS — EventKit (EKEventStore). Full two-way where permitted.
// ---------------------------------------------------------------------------

/// macOS **EventKit** native adapter stub (doc §3 Tier A). Planned capability is
/// full native read+write (permission-gated at runtime by the OS).
#[derive(Clone, Copy, Debug, Default)]
pub struct EventKitAdapter;

impl NativeBackend for EventKitAdapter {
    fn planned_capability(&self) -> CalendarCapability {
        CalendarCapability::Native {
            read: true,
            write: true,
        }
    }
    fn on_target_os(&self) -> bool {
        cfg!(target_os = "macos")
    }
    fn status(&self) -> &'static str {
        "macOS EventKit: native two-way (planned); FFI backend deferred"
    }
}

impl CalendarSyncAdapter for EventKitAdapter {
    fn capability(&self) -> CalendarCapability {
        current_capability(self)
    }
    async fn list_calendars(&self) -> SyncResult<Vec<RemoteCalendar>> {
        Err(not_implemented("EventKit"))
    }
    async fn pull(&self, _cal: &CalId, _since: &SyncToken) -> SyncResult<ChangeSet> {
        Err(not_implemented("EventKit"))
    }
    async fn push(&self, _cal: &CalId, _ops: &[EventOp]) -> SyncResult<Vec<PushResult>> {
        Err(not_implemented("EventKit"))
    }
}

// ---------------------------------------------------------------------------
// Linux — Evolution-Data-Server (D-Bus) / GNOME Online Accounts.
// ---------------------------------------------------------------------------

/// Linux **Evolution-Data-Server** native adapter stub (doc §3 Tier A). Planned
/// capability is full native read+write via the EDS D-Bus API.
#[derive(Clone, Copy, Debug, Default)]
pub struct EdsAdapter;

impl NativeBackend for EdsAdapter {
    fn planned_capability(&self) -> CalendarCapability {
        CalendarCapability::Native {
            read: true,
            write: true,
        }
    }
    fn on_target_os(&self) -> bool {
        cfg!(target_os = "linux")
    }
    fn status(&self) -> &'static str {
        "Linux Evolution-Data-Server: native two-way (planned); D-Bus backend deferred"
    }
}

impl CalendarSyncAdapter for EdsAdapter {
    fn capability(&self) -> CalendarCapability {
        current_capability(self)
    }
    async fn list_calendars(&self) -> SyncResult<Vec<RemoteCalendar>> {
        Err(not_implemented("EvolutionDataServer"))
    }
    async fn pull(&self, _cal: &CalId, _since: &SyncToken) -> SyncResult<ChangeSet> {
        Err(not_implemented("EvolutionDataServer"))
    }
    async fn push(&self, _cal: &CalId, _ops: &[EventOp]) -> SyncResult<Vec<PushResult>> {
        Err(not_implemented("EvolutionDataServer"))
    }
}

// ---------------------------------------------------------------------------
// Windows — AppointmentManager. Read + limited write (doc §3 table).
// ---------------------------------------------------------------------------

/// Windows **AppointmentManager** native adapter stub (doc §3 Tier A). The doc's
/// tier table describes it as "read + *limited* write"; to avoid promising write
/// access it cannot honestly deliver, its planned capability is native **read
/// only** — the truthful, non-downgrading report (connect CalDAV for two-way).
#[derive(Clone, Copy, Debug, Default)]
pub struct AppointmentManagerAdapter;

impl NativeBackend for AppointmentManagerAdapter {
    fn planned_capability(&self) -> CalendarCapability {
        CalendarCapability::Native {
            read: true,
            write: false,
        }
    }
    fn on_target_os(&self) -> bool {
        cfg!(target_os = "windows")
    }
    fn status(&self) -> &'static str {
        "Windows AppointmentManager: native read-only (planned); connect CalDAV for two-way"
    }
}

impl CalendarSyncAdapter for AppointmentManagerAdapter {
    fn capability(&self) -> CalendarCapability {
        current_capability(self)
    }
    async fn list_calendars(&self) -> SyncResult<Vec<RemoteCalendar>> {
        Err(not_implemented("AppointmentManager"))
    }
    async fn pull(&self, _cal: &CalId, _since: &SyncToken) -> SyncResult<ChangeSet> {
        Err(not_implemented("AppointmentManager"))
    }
    async fn push(&self, _cal: &CalId, _ops: &[EventOp]) -> SyncResult<Vec<PushResult>> {
        // Even once implemented, write is not offered — report it honestly.
        Err(SyncError::Unsupported(
            "Windows AppointmentManager is read-only; connect a CalDAV account to \
             write events — docs/casual-note-calendar.md §3"
                .to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// MockSyncAdapter — scripted test double for the full sync loop.
// ---------------------------------------------------------------------------

use std::collections::{HashSet, VecDeque};
use std::sync::Mutex;

use crate::sync::{PushOutcome, RemoteEvent};

/// A scripted [`CalendarSyncAdapter`] for exercising the whole sync loop offline.
///
/// - `capability` is whatever the test configures (to check honest reporting).
/// - `list_calendars` returns a fixed set.
/// - each `pull` pops the next scripted [`ChangeSet`] (empty once exhausted).
/// - each `push` records the ops and returns [`PushOutcome::Written`] per op —
///   except for UIDs registered via [`fail_push_for`](Self::fail_push_for), which
///   yield [`PushOutcome::Conflict`] (simulating a 412) so tests can drive the
///   "losing local edit preserved" path.
#[derive(Debug)]
pub struct MockSyncAdapter {
    capability: CalendarCapability,
    calendars: Vec<RemoteCalendar>,
    pull_scripts: Mutex<VecDeque<ChangeSet>>,
    push_log: Mutex<Vec<EventOp>>,
    conflict_uids: Mutex<HashSet<String>>,
}

impl MockSyncAdapter {
    /// A mock reporting `capability`, with no calendars and no scripted pulls.
    #[must_use]
    pub fn new(capability: CalendarCapability) -> Self {
        Self {
            capability,
            calendars: Vec::new(),
            pull_scripts: Mutex::new(VecDeque::new()),
            push_log: Mutex::new(Vec::new()),
            conflict_uids: Mutex::new(HashSet::new()),
        }
    }

    /// A full two-way CalDAV-tier mock (the common test default).
    #[must_use]
    pub fn caldav() -> Self {
        Self::new(CalendarCapability::CalDav {
            read: true,
            write: true,
        })
    }

    /// Set the calendars returned by [`list_calendars`](CalendarSyncAdapter::list_calendars).
    #[must_use]
    pub fn with_calendars(mut self, calendars: Vec<RemoteCalendar>) -> Self {
        self.calendars = calendars;
        self
    }

    /// Queue a [`ChangeSet`] for the next `pull`.
    pub fn script_pull(&self, changeset: ChangeSet) {
        self.lock(&self.pull_scripts).push_back(changeset);
    }

    /// Make future pushes of `uid` return [`PushOutcome::Conflict`] (a 412).
    pub fn fail_push_for(&self, uid: impl Into<String>) {
        self.lock(&self.conflict_uids).insert(uid.into());
    }

    /// Every op passed to `push`, in order — for assertions (e.g. `If-Match` intent).
    #[must_use]
    pub fn pushed_ops(&self) -> Vec<EventOp> {
        self.lock(&self.push_log).clone()
    }

    fn lock<'a, U>(&self, m: &'a Mutex<U>) -> std::sync::MutexGuard<'a, U> {
        m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl CalendarSyncAdapter for MockSyncAdapter {
    fn capability(&self) -> CalendarCapability {
        self.capability
    }

    async fn list_calendars(&self) -> SyncResult<Vec<RemoteCalendar>> {
        Ok(self.calendars.clone())
    }

    async fn pull(&self, _cal: &CalId, _since: &SyncToken) -> SyncResult<ChangeSet> {
        Ok(self
            .lock(&self.pull_scripts)
            .pop_front()
            .unwrap_or_default())
    }

    async fn push(&self, _cal: &CalId, ops: &[EventOp]) -> SyncResult<Vec<PushResult>> {
        if !self.capability.can_write() {
            return Err(SyncError::Unsupported(
                "MockSyncAdapter configured read-only".to_string(),
            ));
        }
        let conflicts = self.lock(&self.conflict_uids);
        let mut results = Vec::with_capacity(ops.len());
        for op in ops {
            self.lock(&self.push_log).push(op.clone());
            let uid = op.uid().to_string();
            let outcome = if conflicts.contains(&uid) {
                PushOutcome::Conflict { local_etag: None }
            } else {
                match op {
                    EventOp::Delete { .. } => PushOutcome::Deleted,
                    EventOp::Create(e) | EventOp::Update { event: e, .. } => PushOutcome::Written {
                        href: format!("mock/{uid}.ics"),
                        etag: Some(format!("\"etag-{}\"", e.sequence)),
                    },
                }
            };
            results.push(PushResult { uid, outcome });
        }
        Ok(results)
    }
}

/// Convenience: build a [`RemoteEvent`] from an event + href (test ergonomics).
#[must_use]
pub fn remote_event(href: impl Into<String>, event: crate::model::CalendarEvent) -> RemoteEvent {
    RemoteEvent {
        href: href.into(),
        event,
    }
}

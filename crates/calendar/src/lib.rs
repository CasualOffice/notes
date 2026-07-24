//! # calendar — Casual Note calendar engine (Workstream W13)
//!
//! Implements the pure-Rust core of the calendar surface described in
//! **`docs/casual-note-calendar.md`**: the domain model (§4), RFC 5545 ICS
//! import/export (§2, §9), and projection of Casual Note items to events (§5).
//!
//! This crate is deliberately **standalone and side-effect-free**: it has no
//! network, no filesystem, no database, and no OS FFI. Real CalDAV/HTTP transport
//! (RFC 4791/6578) and the native OS calendar adapters (EventKit / Evolution-Data-
//! Server / AppointmentManager) are added in the sync phase behind the
//! `CalendarSyncAdapter` seam sketched in doc §3.1 — they are out of scope here.
//! It depends only on `app-domain` (shared [`Id`](app_domain::Id) /
//! [`Timestamp`](app_domain::Timestamp)) and never on the `tasks`/`reminders`/
//! `notes` crates: projection takes plain input structs instead ([`project`]).
//!
//! Privacy invariants it upholds (doc §1, §7): no credential is ever stored here —
//! [`CalendarAccount`] holds connection metadata only, and secrets are passed to
//! the (future) sync layer as parameters from the OS keystore.
//!
//! ## Modules
//!
//! *Offline core:*
//! - [`model`]   — the domain types ([`Calendar`], [`CalendarEvent`],
//!   [`CalendarAccount`], [`EventAlarm`], …).
//! - [`ical`]    — RFC 5545 parse ([`ical::parse_ics`]) and serialize
//!   ([`ical::write_ics`]).
//! - [`project`] — [`task_to_event`](project::task_to_event),
//!   [`reminder_to_event`](project::reminder_to_event),
//!   [`meeting_to_event`](project::meeting_to_event), and the reverse
//!   [`detect_source_ref`](project::detect_source_ref) marker helper.
//! - [`error`]   — the [`CalendarError`] / [`SyncError`] taxonomies.
//!
//! *Sync layer (doc §3) — protocol logic only; the socket lives behind a seam:*
//! - [`sync`]      — the [`CalendarSyncAdapter`] trait, [`CalendarCapability`]
//!   tiers, the shared value types, and a crash-safe [`LocalCalendarState`]
//!   reconciler.
//! - [`transport`] — the async [`Transport`] I/O seam + an in-memory
//!   [`MockTransport`](transport::MockTransport).
//! - [`caldav`]    — RFC 4791 + RFC 6578 CalDAV over [`Transport`](transport::Transport).
//! - [`conflict`]  — last-writer-wins conflict resolution (doc §3.2), preserving
//!   the losing local edit.
//! - [`adapters`]  — native (Tier A) EventKit / EDS / AppointmentManager stubs
//!   with honest capability reporting, plus a [`MockSyncAdapter`](adapters::MockSyncAdapter).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod adapters;
pub mod caldav;
pub mod conflict;
pub mod error;
pub mod ical;
pub mod model;
pub mod project;
pub mod sync;
pub mod transport;

pub use error::{CalendarError, CalendarResult, SyncError, SyncResult};
pub use ical::{event_to_vevent, parse_ics, write_ics, PRODID, X_SOURCE};
pub use model::{
    AccountKind, AlarmAction, AlarmTrigger, Calendar, CalendarAccount, CalendarEvent,
    CalendarSource, EventAlarm, EventStatus, RecurrenceId, SourceKind, SourceRef, Transparency,
};
pub use project::{
    detect_source_ref, meeting_to_event, projected_uid, reminder_to_event, task_to_event,
    MeetingInput, ReminderInput, TaskInput,
};

pub use adapters::{
    AppointmentManagerAdapter, EdsAdapter, EventKitAdapter, MockSyncAdapter, NativeBackend,
};
pub use caldav::CalDavClient;
pub use conflict::{resolve as resolve_conflict, ConflictOutcome, Winner};
pub use sync::{
    CalId, CalendarCapability, CalendarSyncAdapter, ChangeSet, EventOp, LocalCalendarState,
    PushOutcome, PushResult, RemoteCalendar, RemoteEvent, SyncToken,
};
pub use transport::{HttpRequest, HttpResponse, MockTransport, Transport};

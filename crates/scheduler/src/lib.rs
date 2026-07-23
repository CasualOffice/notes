//! # scheduler
//!
//! The dual-layer reminder notifier. Implements **HLD §8.3** and **Architecture §6**:
//!
//! - **Layer A** ([`wheel`]) — an in-memory min-heap timer wheel keyed on `fire_at`,
//!   owned by one Tokio task ([`service`]), rebuilt from SQLite on boot, firing
//!   within tolerance (N7: ± 1 s while running).
//! - **Layer B** ([`backend`]) — a per-OS one-shot handoff: macOS
//!   `UNCalendarNotificationTrigger` / Windows `ScheduledToastNotification` (Phase-1
//!   stubs) within a rolling 14-day [`horizon`]; **Linux has no OS layer** and
//!   reports [`SchedulerCapability::RunningOnly`] honestly (CLAUDE.md
//!   capability-honesty invariant, HLD §9.3).
//! - **Catch-up sweep** ([`catchup`]) — on launch/wake, coalesces past-due
//!   reminders (`state='pending' AND fire_at < now`) into one grouped notification.
//!
//! De-dup is **state-gated, not timer-gated** ([`gate`]): whichever layer claims a
//! reminder first flips `pending|snoozed → fired`; the other no-ops, so a reminder
//! fires **exactly once across both layers**.
//!
//! Mutation invariant (Architecture §6.1): write SQLite first → patch Layer A
//! ([`SchedulerHandle::arm`]/[`disarm`](SchedulerHandle::disarm)) → reconcile Layer
//! B's horizon ([`horizon::reconcile`]).
//!
//! ## Modules
//! - [`capability`] — [`SchedulerCapability`], [`ScheduledReminder`], [`OsHandle`].
//! - [`backend`]    — [`OsNotificationBackend`] (aka [`SchedulerAdapter`]) + Linux/macOS/Windows.
//! - [`wheel`]      — [`TimerWheel`] (Layer A, pure & simulated-clock-testable).
//! - [`service`]    — the async driver + [`SchedulerHandle`] + [`DeliverySink`].
//! - [`gate`]       — [`FireGate`] state-gated de-dup + [`InMemoryGate`].
//! - [`catchup`]    — the missed-reminder [`sweep`](catchup::sweep).
//! - [`horizon`]    — Layer-B 14-day horizon reconcile helpers.
//! - [`clock`]      — [`Clock`] abstraction ([`SystemClock`]/[`SimClock`]).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod backend;
pub mod capability;
pub mod catchup;
pub mod clock;
pub mod error;
pub mod gate;
pub mod horizon;
pub mod service;
pub mod wheel;

/// The OS-backend trait under its command-surface role name (HLD). Same trait as
/// [`OsNotificationBackend`] (Architecture §6.1).
pub use backend::OsNotificationBackend as SchedulerAdapter;
pub use backend::{
    platform_backend, LinuxBackend, MacosBackend, OsNotificationBackend, WindowsBackend,
};
pub use capability::{OsHandle, ScheduledReminder, SchedulerCapability, DEFAULT_HORIZON_DAYS};
pub use catchup::{sweep as catch_up_sweep, CatchUp};
pub use clock::{Clock, SimClock, SystemClock};
pub use error::SchedulerError;
pub use gate::{FireGate, InMemoryGate};
pub use service::{
    spawn as spawn_scheduler, DeliverySink, FiredReminder, SchedulerConfig, SchedulerHandle,
};
pub use wheel::TimerWheel;

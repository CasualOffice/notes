//! # reminders
//!
//! First-class polymorphic reminders. Implements **Data Model §7** (`reminder`
//! state machine + `recurrence_rule`). Recurrence uses the `rrule` crate with
//! **materialize-on-completion** (not pre-expansion): a template plus exactly one
//! next instance; `fixed` advances from the scheduled date (`every`),
//! `after_completion` from the completion date (`every!`).
//!
//! `fire_at` is absolute UTC ms plus an IANA `tz` so DST math is reconstructable.
//! Delivery is gated on `state` (HLD §8.3); any schedule-changing mutation clears
//! the stored `os_handle` first to prevent stale OS fires.
//!
//! This crate is pure domain logic — no DB, IO, OS, or async. `storage` maps the
//! [`Reminder`] / [`RecurrenceRule`] projections to/from SQLite rows; `scheduler`
//! consumes [`Reminder::effective_fire_at`] and [`ReminderState`] for de-dup.
//!
//! ## Modules
//! - [`state`]      — [`ReminderState`] machine + [`OsLayer`] tag.
//! - [`reminder`]   — the [`Reminder`] entity, [`ReminderTarget`], mutating ops.
//! - [`recurrence`] — [`RecurrenceRule`] + [`RecurrenceMode`] + [`advance`](RecurrenceRule::advance).
//! - [`error`]      — [`ReminderError`] (maps into `app_domain::AppError`).

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod error;
pub mod recurrence;
pub mod reminder;
pub mod state;

pub use error::ReminderError;
pub use recurrence::{Advance, RecurrenceMode, RecurrenceRule};
pub use reminder::{BlockRef, Reminder, ReminderTarget, ReminderTargetKind};
pub use state::{OsLayer, ReminderState};

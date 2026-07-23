//! # tasks
//!
//! The planning pillar. Implements **Data Model §6** (`area`/`project`/`task`/
//! `heading`/`checklist_item`). Buckets (Today/Upcoming/Anytime/Someday, plus the
//! closed-task Logbook) are **derived queries, never stored** (§6.3, Feature
//! Specs §3.1): `start_on` hides (When), `deadline_on` shows a due badge but
//! never hides, alert timing is a separate `reminder` — these three are never
//! conflated.
//!
//! Reorder uses a fractional index (`order_key`, LexoRank-style, base-62); a
//! single moved row's key is rewritten to the midpoint of its neighbours — O(1),
//! no bulk renumber (Feature Specs §3.4). Per-field LWW-by-HLC for structured
//! updates (HLD §10) is applied by the writer, not here.
//!
//! This crate is pure domain logic: it holds **no** database handle and emits
//! **no** SQL to the WebView. It provides the SQL *text* for `storage` to prepare
//! (the op-log write and derived-table rebuild stay Rust-side), the fractional
//! index math, and pure status-transition helpers.
//!
//! ## Modules
//! - [`domain`]     — [`Area`], [`Project`], [`Task`], [`Heading`],
//!   [`ChecklistItem`], and the [`TaskStatus`]/[`ProjectStatus`]/[`Priority`] enums.
//! - [`bucket`]     — per-bucket SQL builders ([`QueryBucket`]) and the in-memory
//!   [`classify`] twin.
//! - [`order_key`]  — the base-62 fractional index ([`key_between`],
//!   [`key_after`], [`key_before`], [`rebalance`]).
//! - [`transition`] — completion/cancel/reopen helpers for tasks and projects.
//! - [`error`]      — [`TaskError`] and its bridge to `app_domain::AppError`.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod bucket;
pub mod domain;
pub mod error;
pub mod order_key;
pub mod transition;

// Flat re-exports of the most-used items.
pub use bucket::{classify, QueryBucket};
pub use domain::{
    Area, ChecklistItem, ChecklistItemId, Heading, HeadingId, Priority, Project, ProjectStatus,
    Task, TaskStatus,
};
pub use error::{TaskError, TaskResult};
pub use order_key::{key_after, key_before, key_between, rebalance};
pub use transition::{
    cancel_task, complete_task, project_transition, reopen_task, task_transition,
    ProjectStatusChange, TaskStatusChange,
};

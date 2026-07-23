//! # app-domain
//!
//! Shared, dependency-light domain vocabulary for Casual Note. Implements the
//! cross-cutting types named in **HLD §5** ("shared types, ID (UUIDv7/ULID), HLC,
//! error taxonomy, event enums"), the **Data Model §1/§3** ID/time/concurrency
//! conventions, the enumerations of **Data Model §3.1/§5.1/§6.3/§8.1**, and the
//! **HLD §7** `AppEvent` push model.
//!
//! This crate contains **no DB, IO, or async code** — it is the stable vocabulary
//! every other crate depends on downward (HLD §5: "dependency direction is strictly
//! downward"). It must stay stable across phases so later crates need no re-model.
//!
//! ## Modules
//! - [`id`]    — [`Id`](id::Id) (UUIDv7 entity id), [`OpId`](id::OpId) (ULID op-log
//!   id), [`BlockId`](id::BlockId), [`ModelId`](id::ModelId), and semantic aliases.
//! - [`time`]  — [`Timestamp`](time::Timestamp) (epoch-ms UTC) and [`Day`](time::Day)
//!   (`YYYY-MM-DD` wall-date).
//! - [`hlc`]   — the [`Hlc`](hlc::Hlc) hybrid logical clock (the dormant sync seam).
//! - [`kind`]  — [`EntityKind`](kind::EntityKind), [`LinkRel`](kind::LinkRel),
//!   [`SessionState`](kind::SessionState), [`Bucket`](kind::Bucket),
//!   [`Platform`](kind::Platform), [`EntityRef`](kind::EntityRef).
//! - [`error`] — the [`AppError`](error::AppError) taxonomy and
//!   [`ErrorClass`](error::ErrorClass) (typed + retryable).
//! - [`event`] — the [`AppEvent`](event::AppEvent) enum and
//!   [`SequencedEvent`](event::SequencedEvent) envelope.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod error;
pub mod event;
pub mod hlc;
pub mod id;
pub mod kind;
pub mod time;

// Flat re-exports of the most-used types, so downstream crates can
// `use app_domain::{Id, Hlc, EntityKind, AppError, AppEvent, ...}`.
pub use error::{AppError, AppResult, ErrorClass};
pub use event::{
    AppEvent, PlatformCaps, SearchSource, SequencedEvent, TranscriptPass, TranscriptSegment,
};
pub use hlc::Hlc;
pub use id::{
    ActionItemId, AreaId, ArtifactId, BatchId, BlockId, ChunkId, Id, LinkId, ModelId, NoteId,
    NotebookId, OpId, PersonId, ProjectId, QueryId, RecurrenceRuleId, ReminderId, SegmentId,
    SessionId, TagId, TaskId,
};
pub use kind::{Bucket, EntityKind, EntityRef, LinkRel, Platform, SessionState};
pub use time::{Day, Timestamp};

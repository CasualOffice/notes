//! The `AppEvent` push model. Implements HLD §7 (Event Model) verbatim.
//!
//! Events are **derived facts**, never commands: the Core pushes them via
//! `tauri::Window::emit` and the WebView reconciles its local view. Every event is
//! wrapped in a [`SequencedEvent`] carrying a monotonic `seq`; variants carry the
//! originating `entity_ref`/`target_ref` where applicable.

use serde::{Deserialize, Serialize};

use crate::error::ErrorClass;
use crate::id::{
    BatchId, BlockId, ModelId, NoteId, ProjectId, QueryId, ReminderId, SessionId, TagId, TaskId,
};
use crate::kind::{Bucket, EntityRef, Platform, SessionState};
use crate::time::Timestamp;

/// A single derived-fact event pushed to the WebView (HLD §7).
///
/// Serialized as an internally-tagged object: `{ "type": "<Variant>", ... }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AppEvent {
    // --- Notes / knowledge ---
    NoteSaved {
        note_id: NoteId,
        version: u64,
        changed_block_ids: Vec<BlockId>,
    },
    /// Block-index / FTS / links rebuilt for a note.
    NoteProjected {
        note_id: NoteId,
    },
    BacklinksChanged {
        target_ref: EntityRef,
        count: u32,
    },
    TagChanged {
        tag_id: TagId,
    },

    // --- Tasks / planning ---
    TaskChanged {
        task_id: TaskId,
        bucket_hint: Option<Bucket>,
    },
    TaskCompleted {
        task_id: TaskId,
        recurrence_spawned: Option<TaskId>,
    },
    ProjectChanged {
        project_id: ProjectId,
    },

    // --- Reminders / scheduler ---
    ReminderScheduled {
        reminder_id: ReminderId,
        fire_at: Timestamp,
        /// Whether an OS one-shot (Layer B) was registered (false on Linux — HLD §9.3).
        os_layer: bool,
    },
    ReminderFired {
        reminder_id: ReminderId,
        target_ref: Option<EntityRef>,
        grouped: bool,
    },
    /// Catch-up sweep result: reminders that fired while the app was closed.
    ReminderMissedSwept {
        reminder_ids: Vec<ReminderId>,
    },

    // --- Meeting lifecycle ---
    SessionStateChanged {
        session_id: SessionId,
        from: SessionState,
        to: SessionState,
        degraded: Option<String>,
    },
    /// Throttled UI level meter.
    CaptureLevel {
        session_id: SessionId,
        rms_dbfs: f32,
    },
    /// Pass-1 live transcript stream.
    LiveTranscript {
        session_id: SessionId,
        segment: TranscriptSegment,
    },
    ArtifactReady {
        session_id: SessionId,
    },
    IndexingProgress {
        session_id: SessionId,
        stage: String,
        pct: f32,
    },

    // --- AI / search ---
    /// Streamed + re-fused search hits (FTS first, vector streams in).
    SearchPartial {
        query_id: QueryId,
        hits: u32,
        source: SearchSource,
    },
    AnswerReady {
        query_id: QueryId,
    },
    SuggestionsReady {
        batch_id: BatchId,
        count: u32,
    },

    // --- System / models / health ---
    ModelDownloadProgress {
        model_id: ModelId,
        pct: f32,
        resumable: bool,
    },
    CapabilityReport {
        platform_caps: PlatformCaps,
    },
    OfflineReady {
        ok: bool,
    },
    Error {
        taxonomy: ErrorClass,
        retryable: bool,
        context: String,
    },
}

/// The two retrieval sources fused by RRF (HLD §8.5, Data Model §10.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchSource {
    Fts,
    Vector,
}

/// A transcript segment as pushed on `LiveTranscript` (mirrors Data Model §8.3;
/// the full authoritative row lives in `transcript_segment`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSegment {
    pub segment_id: crate::id::SegmentId,
    pub t_start_ms: i64,
    pub t_end_ms: i64,
    pub speaker: Option<String>,
    pub text: String,
    /// `"live"` (pass-1) or `"final"` (pass-2). Live rows are superseded by final.
    pub pass: TranscriptPass,
    pub confidence: Option<f32>,
}

/// Which STT pass produced a segment (Data Model §8.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptPass {
    Live,
    Final,
}

/// Honest per-platform capability report surfaced to the UI (HLD §9,
/// Architecture §11). The app never exposes a capability the adapter reports absent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlatformCaps {
    pub platform: Platform,
    /// Whether per-application audio capture is available.
    pub app_audio_capture: bool,
    /// Whether the adapter can exclude its own audio from a system capture.
    pub exclude_self: bool,
    /// Whether an OS one-shot notification scheduling layer (Layer B) exists.
    /// `false` on Linux — reported, not faked (HLD §9.3).
    pub reminder_os_layer: bool,
    /// Whether the reminder scheduler is running-only (fires only while the app runs).
    pub reminder_running_only: bool,
}

/// A monotonically-sequenced envelope carrying an [`AppEvent`] to the WebView.
/// The WebView uses `seq` to detect gaps / ordering (HLD §7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SequencedEvent {
    pub seq: u64,
    #[serde(flatten)]
    pub event: AppEvent,
}

impl SequencedEvent {
    #[must_use]
    pub fn new(seq: u64, event: AppEvent) -> Self {
        Self { seq, event }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::id::Id;
    use crate::kind::EntityKind;

    #[test]
    fn event_is_internally_tagged() {
        let ev = AppEvent::NoteSaved {
            note_id: Id::new(),
            version: 3,
            changed_block_ids: vec![BlockId::new("b7")],
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["type"], "NoteSaved");
        assert_eq!(v["version"], 3);
    }

    #[test]
    fn sequenced_event_flattens() {
        let ev = AppEvent::BacklinksChanged {
            target_ref: EntityRef::new(EntityKind::Note, Id::new()),
            count: 2,
        };
        let wrapped = SequencedEvent::new(42, ev);
        let v: serde_json::Value = serde_json::to_value(&wrapped).unwrap();
        assert_eq!(v["seq"], 42);
        assert_eq!(v["type"], "BacklinksChanged");
        assert_eq!(v["count"], 2);
    }

    #[test]
    fn error_event_carries_taxonomy() {
        let ev = AppEvent::Error {
            taxonomy: ErrorClass::Conflict,
            retryable: false,
            context: "stale write".into(),
        };
        let v: serde_json::Value = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["taxonomy"], "conflict");
        assert_eq!(v["retryable"], false);
    }
}

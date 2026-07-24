//! The retrieval unit the Ask pipeline reasons over — an in-memory projection of a
//! `chunk` row (**Data Model §9.2**) plus its owning spine [`EntityRef`].
//!
//! A [`Chunk`] is source-agnostic (a note block, a transcript window, a task, or a
//! reminder all reduce to one). It carries exactly the fields citation-verify needs
//! to (a) match a produced [`Citation`] back to real retrieved evidence and (b) let
//! the UI navigate `chunk → note block / segment+timestamp` (HLD §8.5). The
//! `source_kind` reuses the authoritative [`SourceKind`] enum from `llm-api`, so a
//! [`Chunk`] and the [`Citation`] it backs speak the same vocabulary.

use app_domain::{ChunkId, EntityKind, EntityRef, Id, SessionId};
use llm_api::{Citation, SourceKind};

use crate::text::snippet;

/// Max citation snippet length (chars). A leading excerpt of the chunk text, never
/// the whole block, so a citation stays a pointer-with-context, not a copy.
const SNIPPET_CHARS: usize = 160;

/// One source-agnostic retrieval unit (Data Model §9.2 `chunk`, in-memory subset).
///
/// The `entity` is the spine node to navigate to; `source_id` is the id recorded in
/// an [`AnswerV1`](llm_api::AnswerV1) citation (the note/task/reminder entity, or the
/// session for a transcript window). For non-temporal sources `t_start_ms` is `0`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    /// `chunk.id` — the id a citation must resolve to.
    pub chunk_id: ChunkId,
    /// The spine entity this chunk belongs to (navigation target).
    pub entity: EntityRef,
    /// The citation `source_kind` discriminant (Data Model §14.2).
    pub source_kind: SourceKind,
    /// The originating entity id, or session id for a transcript window.
    pub source_id: Id,
    /// Millisecond offset within a transcript window; `0` for non-temporal sources.
    pub t_start_ms: i64,
    /// The chunk text used for both the lexical channel and embedding.
    pub text: String,
}

impl Chunk {
    /// A note-block chunk owned by note `note_id`.
    #[must_use]
    pub fn note_block(chunk_id: ChunkId, note_id: Id, text: impl Into<String>) -> Self {
        Self {
            chunk_id,
            entity: EntityRef::new(EntityKind::Note, note_id),
            source_kind: SourceKind::NoteBlock,
            source_id: note_id,
            t_start_ms: 0,
            text: text.into(),
        }
    }

    /// A transcript-window chunk anchored at `t_start_ms` within a session.
    #[must_use]
    pub fn transcript(
        chunk_id: ChunkId,
        session_id: SessionId,
        t_start_ms: i64,
        text: impl Into<String>,
    ) -> Self {
        Self {
            chunk_id,
            entity: EntityRef::new(EntityKind::Session, session_id),
            source_kind: SourceKind::TranscriptWindow,
            source_id: session_id,
            t_start_ms,
            text: text.into(),
        }
    }

    /// A task chunk (one chunk per task, Data Model §9.2).
    #[must_use]
    pub fn task(chunk_id: ChunkId, task_id: Id, text: impl Into<String>) -> Self {
        Self {
            chunk_id,
            entity: EntityRef::new(EntityKind::Task, task_id),
            source_kind: SourceKind::Task,
            source_id: task_id,
            t_start_ms: 0,
            text: text.into(),
        }
    }

    /// A reminder chunk (one chunk per reminder).
    #[must_use]
    pub fn reminder(chunk_id: ChunkId, reminder_id: Id, text: impl Into<String>) -> Self {
        Self {
            chunk_id,
            entity: EntityRef::new(EntityKind::Reminder, reminder_id),
            source_kind: SourceKind::Reminder,
            source_id: reminder_id,
            t_start_ms: 0,
            text: text.into(),
        }
    }

    /// The [`Citation`] that would point at this chunk, with a leading snippet.
    /// This is the ground truth citation-verify compares a produced citation to.
    #[must_use]
    pub fn to_citation(&self) -> Citation {
        Citation {
            chunk_id: self.chunk_id,
            source_kind: self.source_kind,
            source_id: self.source_id,
            t_start_ms: self.t_start_ms,
            snippet: snippet(&self.text, SNIPPET_CHARS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_block_maps_source_kind_and_entity() {
        let note = Id::new();
        let c = Chunk::note_block(Id::new(), note, "hello world");
        assert_eq!(c.source_kind, SourceKind::NoteBlock);
        assert_eq!(c.entity.kind, EntityKind::Note);
        assert_eq!(c.source_id, note);
        assert_eq!(c.t_start_ms, 0);
    }

    #[test]
    fn transcript_carries_time_anchor_and_session() {
        let sess = Id::new();
        let c = Chunk::transcript(Id::new(), sess, 12_000, "we agreed to ship");
        assert_eq!(c.source_kind, SourceKind::TranscriptWindow);
        assert_eq!(c.entity.kind, EntityKind::Session);
        assert_eq!(c.t_start_ms, 12_000);
    }

    #[test]
    fn to_citation_round_trips_identity_fields() {
        let c = Chunk::task(Id::new(), Id::new(), "write the release notes for beta");
        let cite = c.to_citation();
        assert_eq!(cite.chunk_id, c.chunk_id);
        assert_eq!(cite.source_kind, c.source_kind);
        assert_eq!(cite.source_id, c.source_id);
        assert!(!cite.snippet.is_empty());
    }
}

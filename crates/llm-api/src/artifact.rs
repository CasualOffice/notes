//! AI artifact JSON contracts — **Data Model §14**, authoritative here.
//!
//! [`MeetingArtifactV1`] (§14.1) and [`AnswerV1`] (§14.2) serialize byte-for-byte
//! to the documented shapes. Field names, order, and nullability match the doc;
//! never add or rename a field without changing the doc first (CLAUDE.md
//! "Document authority").
//!
//! Grounding invariants ("Evidence or nothing"):
//! - every [`MeetingArtifactV1`] fact carries **non-empty** `evidence_segment_ids`;
//! - `owner` / `due_date` are `null` unless stated in the cited evidence;
//! - an [`AnswerV1`] that is not `unanswered` must carry at least one citation;
//!   otherwise it is returned as `unanswered:true` (never hallucinate).
//!
//! Resolving ids to real `transcript_segment` / `chunk` rows is a DB-side step;
//! [`crate::SchemaValidate`] here enforces only the structural invariants.

use app_domain::{ChunkId, Day, Id, SegmentId, SessionId};
use serde::{Deserialize, Serialize};

use crate::{SchemaValidate, SchemaViolation};

// ===========================================================================
// §14.1 MeetingArtifactV1
// ===========================================================================

/// The immutable-per-generation meeting artifact (Data Model §14.1). Stored in
/// `artifact.artifact_json`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MeetingArtifactV1 {
    /// Schema tag — always [`MeetingArtifactV1::SCHEMA`].
    pub schema: String,
    pub session_id: SessionId,
    pub executive_summary: String,
    pub topics: Vec<Topic>,
    pub decisions: Vec<Decision>,
    pub action_items: Vec<ActionItem>,
    pub risks: Vec<Risk>,
    pub open_questions: Vec<OpenQuestion>,
}

impl MeetingArtifactV1 {
    /// The `schema` discriminant string.
    pub const SCHEMA: &'static str = "MeetingArtifactV1";

    /// An empty, well-formed artifact for a session — the shape a deterministic
    /// fallback returns when generation fails (Data Model §14.1 "topics-only";
    /// here, nothing invented at all). Passes [`SchemaValidate`].
    #[must_use]
    pub fn empty(session_id: SessionId) -> Self {
        Self {
            schema: Self::SCHEMA.to_string(),
            session_id,
            executive_summary: String::new(),
            topics: Vec::new(),
            decisions: Vec::new(),
            action_items: Vec::new(),
            risks: Vec::new(),
            open_questions: Vec::new(),
        }
    }
}

/// A discussion topic with its supporting evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Topic {
    pub title: String,
    pub summary: String,
    /// Non-empty; each id resolves to a real `transcript_segment`.
    pub evidence_segment_ids: Vec<SegmentId>,
}

/// A decision reached, with optional rationale.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub statement: String,
    /// `null` unless a rationale was stated.
    pub rationale: Option<String>,
    pub evidence_segment_ids: Vec<SegmentId>,
}

/// An action item — the actionable extraction bridged into Tasks. `owner` /
/// `due_date` are `null` unless explicitly stated in the cited evidence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ActionItem {
    pub task: String,
    /// `null` unless an owner was named in the evidence (never invented).
    pub owner: Option<String>,
    /// `YYYY-MM-DD`, `null` unless a date was stated in the evidence.
    pub due_date: Option<Day>,
    /// REQUIRED, non-empty.
    pub evidence_segment_ids: Vec<SegmentId>,
}

/// A risk raised during the meeting.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Risk {
    pub statement: String,
    pub evidence_segment_ids: Vec<SegmentId>,
}

/// An unresolved question left open.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OpenQuestion {
    pub question: String,
    pub evidence_segment_ids: Vec<SegmentId>,
}

impl SchemaValidate for MeetingArtifactV1 {
    fn validate(&self) -> Result<(), SchemaViolation> {
        if self.schema != Self::SCHEMA {
            return Err(SchemaViolation::new(format!(
                "schema must be {:?}, got {:?}",
                Self::SCHEMA,
                self.schema
            )));
        }
        // Every fact must cite non-empty evidence ("Evidence or nothing").
        for (i, t) in self.topics.iter().enumerate() {
            require_evidence("topics", i, &t.evidence_segment_ids)?;
        }
        for (i, d) in self.decisions.iter().enumerate() {
            require_evidence("decisions", i, &d.evidence_segment_ids)?;
        }
        for (i, a) in self.action_items.iter().enumerate() {
            require_evidence("action_items", i, &a.evidence_segment_ids)?;
            if a.task.trim().is_empty() {
                return Err(SchemaViolation::new(format!(
                    "action_items[{i}].task is empty"
                )));
            }
        }
        for (i, r) in self.risks.iter().enumerate() {
            require_evidence("risks", i, &r.evidence_segment_ids)?;
        }
        for (i, q) in self.open_questions.iter().enumerate() {
            require_evidence("open_questions", i, &q.evidence_segment_ids)?;
        }
        Ok(())
    }
}

fn require_evidence(field: &str, idx: usize, ids: &[SegmentId]) -> Result<(), SchemaViolation> {
    if ids.is_empty() {
        return Err(SchemaViolation::new(format!(
            "{field}[{idx}].evidence_segment_ids must be non-empty"
        )));
    }
    Ok(())
}

// ===========================================================================
// §14.2 AnswerV1
// ===========================================================================

/// The Ask-your-notes RAG output (Data Model §14.2). If no citation resolves,
/// this is returned as [`AnswerV1::unanswered`] rather than displaying an
/// ungrounded answer (Foundation §4; Quality Gate G7).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnswerV1 {
    /// Schema tag — always [`AnswerV1::SCHEMA`].
    pub schema: String,
    pub answer: String,
    pub citations: Vec<Citation>,
    /// Confidence in `0.0..=1.0`.
    pub confidence: f32,
    /// `true` => "I couldn't find this in your notes".
    pub unanswered: bool,
}

impl AnswerV1 {
    /// The `schema` discriminant string.
    pub const SCHEMA: &'static str = "AnswerV1";

    /// The grounded-refusal answer: no citations resolved, so nothing is
    /// displayed (Data Model §14.2 citation-verify contract).
    #[must_use]
    pub fn unanswered() -> Self {
        Self {
            schema: Self::SCHEMA.to_string(),
            answer: String::new(),
            citations: Vec::new(),
            confidence: 0.0,
            unanswered: true,
        }
    }
}

/// A single evidence citation backing an [`AnswerV1`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Citation {
    /// The retrieved `chunk.id`; must resolve to a real `chunk` before display.
    pub chunk_id: ChunkId,
    pub source_kind: SourceKind,
    /// The originating entity (note/task/reminder) or session id.
    pub source_id: Id,
    /// Millisecond offset within a transcript window (0 for non-temporal sources).
    pub t_start_ms: i64,
    pub snippet: String,
}

/// The kind of source a citation points at (Data Model §14.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    NoteBlock,
    TranscriptWindow,
    Task,
    Reminder,
}

impl SchemaValidate for AnswerV1 {
    fn validate(&self) -> Result<(), SchemaViolation> {
        if self.schema != Self::SCHEMA {
            return Err(SchemaViolation::new(format!(
                "schema must be {:?}, got {:?}",
                Self::SCHEMA,
                self.schema
            )));
        }
        if !(0.0..=1.0).contains(&self.confidence) {
            return Err(SchemaViolation::new(format!(
                "confidence {} out of range 0.0..=1.0",
                self.confidence
            )));
        }
        if self.unanswered {
            // A refusal carries no citations.
            if !self.citations.is_empty() {
                return Err(SchemaViolation::new(
                    "unanswered answer must have empty citations",
                ));
            }
        } else {
            // "Evidence or nothing": an answered response must be grounded.
            if self.answer.trim().is_empty() {
                return Err(SchemaViolation::new("answered response has empty answer"));
            }
            if self.citations.is_empty() {
                return Err(SchemaViolation::new(
                    "answered response must carry at least one citation",
                ));
            }
        }
        Ok(())
    }
}

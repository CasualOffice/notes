//! # llm-api — constrained-generation contract + AI artifact schemas
//!
//! Defines the [`ConstrainedLlm`] trait and the **repair -> deterministic
//! fallback** generation contract of **HLD §9.2**, plus the authoritative
//! [`MeetingArtifactV1`] and [`AnswerV1`] serde structs of **Data Model §14**.
//!
//! Two things live here, deliberately together:
//!
//! 1. **The engine contract.** [`ConstrainedLlm::decode`] performs one
//!    GBNF-constrained decode. The generic [`generate_structured`] helper wraps
//!    it with the exact contract of Data Model §14.1: parse+validate the output;
//!    on a schema violation issue **one repair pass**; if that still fails, fall
//!    back to a caller-supplied **deterministic** value. Validation failures never
//!    surface invented data (CLAUDE.md "Evidence or nothing").
//!
//! 2. **The JSON contracts.** [`MeetingArtifactV1`] / [`AnswerV1`] serialize
//!    byte-for-byte to the shapes in Data Model §14. Every artifact fact carries
//!    non-empty `evidence_segment_ids`; an answer with no resolvable citations is
//!    returned as `unanswered:true` rather than displayed.
//!
//! This crate is the **pure contract layer only** — `llm-llamacpp` implements
//! [`ConstrainedLlm`] over FFI (single resident context + bounded queue). No
//! unsafe here.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod artifact;

pub use artifact::{
    ActionItem, AnswerV1, Citation, Decision, MeetingArtifactV1, OpenQuestion, Risk, SourceKind,
    Topic,
};

use app_domain::ModelId;
use serde::de::DeserializeOwned;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Grammar + request/response types
// ---------------------------------------------------------------------------

/// A GBNF grammar that hard-constrains the decoder's output (HLD §9.2). The
/// concrete grammars for [`MeetingArtifactV1`] / [`AnswerV1`] are owned by the
/// backend; this newtype just carries the text across the contract.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grammar(pub String);

impl Grammar {
    #[must_use]
    pub fn new(gbnf: impl Into<String>) -> Self {
        Self(gbnf.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A single constrained-generation request.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationRequest {
    /// Optional system preamble.
    pub system: Option<String>,
    /// The user/context prompt (transcript window, retrieved chunks, ...).
    pub prompt: String,
    /// Upper bound on generated tokens.
    pub max_tokens: u32,
    /// Sampling temperature (0.0 = greedy/deterministic).
    pub temperature: f32,
    /// Optional RNG seed for reproducible decodes.
    pub seed: Option<u64>,
}

impl GenerationRequest {
    /// A greedy, deterministic request for `prompt` (temperature 0, fixed seed).
    #[must_use]
    pub fn deterministic(prompt: impl Into<String>, max_tokens: u32) -> Self {
        Self {
            system: None,
            prompt: prompt.into(),
            max_tokens,
            temperature: 0.0,
            seed: Some(0),
        }
    }

    /// Build the follow-up **repair** request: the original prompt plus a note
    /// describing the schema violation the first attempt produced (Data Model
    /// §14.1 "one repair pass on schema-violation").
    #[must_use]
    pub fn with_repair(&self, violation: &str) -> Self {
        let mut next = self.clone();
        next.prompt = format!(
            "{}\n\n[The previous response was rejected: {violation}. \
             Respond again, strictly conforming to the schema. \
             Do not invent owners, dates, or citations.]",
            self.prompt
        );
        next
    }
}

/// Which path in the repair->fallback contract produced a structured value —
/// provenance for observability and tests (Data Model §14.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GenerationPath {
    /// The first constrained decode parsed and validated.
    Direct,
    /// The first decode failed; the single repair pass succeeded.
    Repaired,
    /// Both decodes failed schema validation; the deterministic fallback was used.
    DeterministicFallback,
}

/// A structured generation result plus the path that produced it.
#[derive(Clone, Debug, PartialEq)]
pub struct GenerationOutcome<T> {
    pub value: T,
    pub path: GenerationPath,
}

// ---------------------------------------------------------------------------
// Schema validation
// ---------------------------------------------------------------------------

/// A structural schema violation (not a transport error) — triggers the repair
/// pass, then deterministic fallback.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
#[error("schema violation: {0}")]
pub struct SchemaViolation(pub String);

impl SchemaViolation {
    #[must_use]
    pub fn new(msg: impl Into<String>) -> Self {
        Self(msg.into())
    }
}

/// Structural post-validation for a constrained artifact (Data Model §14). This
/// enforces the invariants a grammar alone cannot (e.g. "evidence non-empty on
/// every fact"). Resolving ids against real rows is a separate, DB-side step.
pub trait SchemaValidate {
    /// # Errors
    /// Returns a [`SchemaViolation`] describing the first invariant broken.
    fn validate(&self) -> Result<(), SchemaViolation>;
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Failures raised across the LLM contract (`thiserror`, typed per taxonomy).
///
/// Note: a **schema violation is not an `LlmError`** — it is handled internally
/// by the repair->fallback contract and never propagates as an error. These are
/// transport/backend failures only; the LLM never owns recording state, so the
/// caller keeps its captured audio on any of these (CLAUDE.md invariant).
#[derive(Debug, Error)]
pub enum LlmError {
    /// The model weights / context are not resident.
    #[error("llm model not loaded")]
    ModelNotLoaded,

    /// The bounded request queue is saturated (backpressure). Retryable.
    #[error("llm request queue full")]
    QueueFull,

    /// The generation was cancelled by the caller.
    #[error("llm generation cancelled")]
    Cancelled,

    /// The supplied GBNF grammar was invalid.
    #[error("invalid grammar: {0}")]
    GrammarError(String),

    /// The decoder failed to produce output.
    #[error("decode failed: {0}")]
    DecodeFailed(String),

    /// Catch-all for a native/backend failure.
    #[error("llm backend error: {0}")]
    Backend(String),
}

// ---------------------------------------------------------------------------
// The trait
// ---------------------------------------------------------------------------

/// A GBNF-constrained local LLM (HLD §9.2), implemented by `llm-llamacpp`.
///
/// Object-safe on purpose: `app-service` holds a `dyn ConstrainedLlm`. Structured
/// generation with the repair->fallback contract is provided by the generic free
/// function [`generate_structured`], keeping the trait itself vtable-friendly.
pub trait ConstrainedLlm: Send + Sync {
    /// The model backing this engine (provenance for `artifact.llm_model`).
    fn model_id(&self) -> &ModelId;

    /// Perform one GBNF-constrained decode, returning the raw text the grammar
    /// produced (expected to be JSON for the artifact schemas).
    ///
    /// # Errors
    /// Returns an [`LlmError`] on transport/backend failure. It does **not** vet
    /// the JSON against a schema — that is [`generate_structured`]'s job.
    fn decode(&self, req: &GenerationRequest, grammar: &Grammar) -> Result<String, LlmError>;
}

/// Parse then structurally validate raw model output.
fn parse_and_validate<T>(raw: &str) -> Result<T, SchemaViolation>
where
    T: DeserializeOwned + SchemaValidate,
{
    let value: T = serde_json::from_str(raw)
        .map_err(|e| SchemaViolation::new(format!("invalid json: {e}")))?;
    value.validate()?;
    Ok(value)
}

/// Structured constrained generation with the **repair -> deterministic
/// fallback** contract of Data Model §14.1.
///
/// 1. Decode once; parse + [`SchemaValidate`]. On success -> [`GenerationPath::Direct`].
/// 2. On a schema violation, issue exactly **one repair pass**
///    ([`GenerationRequest::with_repair`]) and re-validate. On success ->
///    [`GenerationPath::Repaired`].
/// 3. If the repair still violates the schema, use the caller's `fallback`
///    (e.g. topics-only from transcript) -> [`GenerationPath::DeterministicFallback`].
///    The fallback is deterministic and never invents evidence.
///
/// A backend/transport [`LlmError`] from [`ConstrainedLlm::decode`] propagates —
/// it is *not* absorbed by the fallback (the caller keeps its captured audio and
/// may retry).
///
/// # Errors
/// Propagates an [`LlmError`] from the underlying decode.
pub fn generate_structured<T, L, F>(
    llm: &L,
    req: &GenerationRequest,
    grammar: &Grammar,
    fallback: F,
) -> Result<GenerationOutcome<T>, LlmError>
where
    T: DeserializeOwned + SchemaValidate,
    L: ConstrainedLlm + ?Sized,
    F: FnOnce() -> T,
{
    // Attempt 1 — direct.
    let raw = llm.decode(req, grammar)?;
    let violation = match parse_and_validate::<T>(&raw) {
        Ok(value) => {
            return Ok(GenerationOutcome {
                value,
                path: GenerationPath::Direct,
            })
        }
        Err(v) => v,
    };

    // Attempt 2 — one repair pass.
    let repair_req = req.with_repair(&violation.0);
    let raw2 = llm.decode(&repair_req, grammar)?;
    if let Ok(value) = parse_and_validate::<T>(&raw2) {
        return Ok(GenerationOutcome {
            value,
            path: GenerationPath::Repaired,
        });
    }

    // Deterministic fallback — never invents data.
    Ok(GenerationOutcome {
        value: fallback(),
        path: GenerationPath::DeterministicFallback,
    })
}

// ---------------------------------------------------------------------------
// Test double
// ---------------------------------------------------------------------------

/// A scripted [`ConstrainedLlm`] for tests. Each call to [`ConstrainedLlm::decode`]
/// returns the next queued response; when exhausted it repeats the last one. This
/// lets a test drive the direct / repair / fallback branches deterministically.
#[derive(Debug)]
pub struct MockLlm {
    model_id: ModelId,
    responses: std::sync::Mutex<std::collections::VecDeque<String>>,
}

impl MockLlm {
    /// A mock that always returns `response`.
    #[must_use]
    pub fn always(response: impl Into<String>) -> Self {
        Self::scripted(vec![response.into()])
    }

    /// A mock that returns each response in turn (repeating the last).
    #[must_use]
    pub fn scripted(responses: Vec<String>) -> Self {
        Self {
            model_id: ModelId::new("mock-llm"),
            responses: std::sync::Mutex::new(responses.into_iter().collect()),
        }
    }
}

impl ConstrainedLlm for MockLlm {
    fn model_id(&self) -> &ModelId {
        &self.model_id
    }

    fn decode(&self, _req: &GenerationRequest, _grammar: &Grammar) -> Result<String, LlmError> {
        let mut q = self
            .responses
            .lock()
            .map_err(|_| LlmError::Backend("mock lock poisoned".into()))?;
        match q.len() {
            0 => Err(LlmError::DecodeFailed("no scripted responses".into())),
            1 => Ok(q.front().cloned().unwrap_or_default()),
            _ => Ok(q.pop_front().unwrap_or_default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::{ChunkId, Id, SegmentId, SessionId};

    fn sample_artifact() -> MeetingArtifactV1 {
        let seg: SegmentId = Id::new();
        MeetingArtifactV1 {
            schema: MeetingArtifactV1::SCHEMA.to_string(),
            session_id: SessionId::new(),
            executive_summary: "We shipped the beta and agreed on Q3 scope.".into(),
            topics: vec![Topic {
                title: "Beta launch".into(),
                summary: "Beta is live to internal users.".into(),
                evidence_segment_ids: vec![seg],
            }],
            decisions: vec![Decision {
                statement: "Ship beta Friday".into(),
                rationale: Some("QA sign-off received".into()),
                evidence_segment_ids: vec![seg],
            }],
            action_items: vec![ActionItem {
                task: "Write release notes".into(),
                owner: Some("Alex".into()),
                due_date: None,
                evidence_segment_ids: vec![seg],
            }],
            risks: vec![Risk {
                statement: "Load testing incomplete".into(),
                evidence_segment_ids: vec![seg],
            }],
            open_questions: vec![OpenQuestion {
                question: "Who owns rollback?".into(),
                evidence_segment_ids: vec![seg],
            }],
        }
    }

    fn sample_answer() -> AnswerV1 {
        AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "The beta ships Friday.".into(),
            citations: vec![Citation {
                chunk_id: ChunkId::new(),
                source_kind: SourceKind::TranscriptWindow,
                source_id: Id::new(),
                t_start_ms: 12_000,
                snippet: "we agreed to ship the beta on Friday".into(),
            }],
            confidence: 0.82,
            unanswered: false,
        }
    }

    #[test]
    fn meeting_artifact_roundtrips() {
        let a = sample_artifact();
        let json = serde_json::to_string(&a).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema"], "MeetingArtifactV1");
        assert!(
            v["action_items"][0]["evidence_segment_ids"]
                .as_array()
                .unwrap()
                .len()
                == 1
        );
        // owner/due_date honesty: due_date null is preserved.
        assert!(v["action_items"][0]["due_date"].is_null());

        let back: MeetingArtifactV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(a, back);
        back.validate().unwrap();
    }

    #[test]
    fn answer_roundtrips_both_modes() {
        let ans = sample_answer();
        let json = serde_json::to_string(&ans).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["schema"], "AnswerV1");
        assert_eq!(v["unanswered"], false);
        let back: AnswerV1 = serde_json::from_str(&json).unwrap();
        assert_eq!(ans, back);
        back.validate().unwrap();

        let unanswered = AnswerV1::unanswered();
        let j = serde_json::to_string(&unanswered).unwrap();
        let back2: AnswerV1 = serde_json::from_str(&j).unwrap();
        assert_eq!(back2, unanswered);
        assert!(back2.unanswered);
        back2.validate().unwrap();
    }

    #[test]
    fn validate_rejects_empty_evidence() {
        let mut a = sample_artifact();
        a.action_items[0].evidence_segment_ids.clear();
        assert!(a.validate().is_err());
    }

    #[test]
    fn validate_rejects_answered_without_citations() {
        let mut ans = sample_answer();
        ans.citations.clear();
        // answered but no citations -> must be rejected ("evidence or nothing").
        assert!(ans.validate().is_err());
    }

    #[test]
    fn generate_direct_path() {
        let json = serde_json::to_string(&sample_artifact()).unwrap();
        let llm = MockLlm::always(json);
        let req = GenerationRequest::deterministic("summarise", 512);
        let out: GenerationOutcome<MeetingArtifactV1> =
            generate_structured(&llm, &req, &Grammar::new(""), || {
                panic!("fallback must not run")
            })
            .unwrap();
        assert_eq!(out.path, GenerationPath::Direct);
    }

    #[test]
    fn generate_repair_path() {
        let good = serde_json::to_string(&sample_artifact()).unwrap();
        let llm = MockLlm::scripted(vec!["{ not valid json".into(), good]);
        let req = GenerationRequest::deterministic("summarise", 512);
        let out: GenerationOutcome<MeetingArtifactV1> =
            generate_structured(&llm, &req, &Grammar::new(""), || {
                panic!("fallback must not run")
            })
            .unwrap();
        assert_eq!(out.path, GenerationPath::Repaired);
    }

    #[test]
    fn generate_fallback_path_never_invents() {
        // Both attempts violate the schema -> deterministic fallback runs.
        let llm = MockLlm::scripted(vec!["garbage".into(), "still garbage".into()]);
        let req = GenerationRequest::deterministic("summarise", 512);
        let session = SessionId::new();
        let out: GenerationOutcome<MeetingArtifactV1> =
            generate_structured(&llm, &req, &Grammar::new(""), || {
                MeetingArtifactV1::empty(session)
            })
            .unwrap();
        assert_eq!(out.path, GenerationPath::DeterministicFallback);
        assert_eq!(out.value.session_id, session);
        // The fallback invents nothing: no action items, no evidence.
        assert!(out.value.action_items.is_empty());
        out.value.validate().unwrap();
    }

    #[test]
    fn backend_error_propagates_not_absorbed() {
        let llm = MockLlm::scripted(Vec::new()); // empty -> DecodeFailed
        let req = GenerationRequest::deterministic("x", 16);
        let res: Result<GenerationOutcome<AnswerV1>, _> =
            generate_structured(&llm, &req, &Grammar::new(""), AnswerV1::unanswered);
        assert!(matches!(res, Err(LlmError::DecodeFailed(_))));
    }
}

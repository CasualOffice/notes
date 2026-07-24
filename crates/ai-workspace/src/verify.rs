//! Citation-verify — the load-bearing grounding gate (**N14**, **Data Model §14.2**,
//! **HLD §8.5**). "Evidence or nothing."
//!
//! The contract (Data Model §14.2, verbatim): *before display, every
//! `citations[].chunk_id` MUST resolve to a real `chunk`. If none resolve, return
//! `{"unanswered": true}` with empty citations rather than hallucinate.*
//!
//! This module is the only place that decides whether an [`AnswerV1`] is fit to
//! display. It:
//!   1. honours the model's own refusal (`unanswered:true`) — never "answers" for it;
//!   2. resolves each produced citation against the **retrieved candidate pool** (a
//!      citation is verifiable iff it points at a chunk retrieval actually returned —
//!      HLD §8.5 "an unverifiable citation is dropped");
//!   3. **rebuilds** each surviving citation from the *real* chunk, so the
//!      `source_kind` / `source_id` / `t_start_ms` / `snippet` shown are ground truth,
//!      never model-authored text (we never surface unverified evidence); and
//!   4. if zero citations survive, refuses: the answer is downgraded to `unanswered`.
//!
//! A stronger NLI/entailment "does the evidence *support* the claim" check is a
//! documented future seam; grounding here is citation-resolution against retrieved
//! evidence, which is exactly the §14.2 contract and guarantees the N14 invariant
//! ("100% of displayed citations resolve to real chunks").

use llm_api::{AnswerV1, Citation};

use crate::retrieve::RetrievalResult;

/// Why an answer was refused (downgraded to `unanswered`). Provenance for the UI's
/// "I couldn't find this in your notes" and for tests/observability.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnansweredReason {
    /// Retrieval returned no evidence at all — nothing could ground an answer.
    NoEvidenceRetrieved,
    /// The model itself returned `unanswered:true` (an honest refusal, respected).
    ModelRefused,
    /// The model answered, but **no** produced citation resolved to a retrieved
    /// chunk (all were unverifiable/fabricated) — the N14 gate fired.
    NoVerifiableCitations,
    /// The answer text was empty despite claiming to answer (defensive; the schema
    /// validator normally catches this first).
    EmptyAnswer,
    /// Confidence fell below the caller's floor.
    LowConfidence,
}

/// A verified, display-ready answer: its `answer` carries **only** citations that
/// resolved to real retrieved chunks, each rebuilt from ground truth.
#[derive(Clone, Debug, PartialEq)]
pub struct VerifiedAnswer {
    /// The answer with citations pruned to the verified set.
    pub answer: AnswerV1,
    /// Citations the model produced that did **not** resolve — dropped, never shown
    /// (kept here only for observability/telemetry-free logging).
    pub dropped: Vec<Citation>,
}

/// Run citation-verify against the retrieval pool.
///
/// Returns `Ok(VerifiedAnswer)` when the answer is grounded and fit to display, or
/// `Err(UnansweredReason)` when it must be downgraded to `unanswered`.
///
/// `min_confidence` is the caller's floor (default `0.0` = disabled).
pub fn verify(
    answer: &AnswerV1,
    retrieval: &RetrievalResult,
    min_confidence: f32,
) -> Result<VerifiedAnswer, UnansweredReason> {
    // 1. Respect an honest refusal from the model.
    if answer.unanswered {
        return Err(UnansweredReason::ModelRefused);
    }

    // 2. No evidence retrieved ⇒ nothing can be grounded (defensive: a non-refusing
    //    answer over an empty pool must never display).
    if retrieval.is_empty() {
        return Err(UnansweredReason::NoEvidenceRetrieved);
    }

    if answer.answer.trim().is_empty() {
        return Err(UnansweredReason::EmptyAnswer);
    }

    if answer.confidence < min_confidence {
        return Err(UnansweredReason::LowConfidence);
    }

    // 3. Resolve every citation against the retrieved pool; rebuild survivors from
    //    ground truth; drop the rest. De-duplicate by chunk_id, preserving order.
    let mut verified: Vec<Citation> = Vec::new();
    let mut dropped: Vec<Citation> = Vec::new();
    let mut kept_ids: Vec<_> = Vec::new();
    for cited in &answer.citations {
        match retrieval.resolve(cited.chunk_id) {
            Some(chunk) => {
                if kept_ids.contains(&chunk.chunk_id) {
                    continue; // duplicate citation of the same chunk
                }
                kept_ids.push(chunk.chunk_id);
                // Rebuild from truth: never surface a model-authored snippet/source.
                verified.push(chunk.to_citation());
            }
            None => dropped.push(cited.clone()),
        }
    }

    // 4. Zero survivors ⇒ refuse (the hard N14 gate).
    if verified.is_empty() {
        return Err(UnansweredReason::NoVerifiableCitations);
    }

    let mut grounded = answer.clone();
    grounded.citations = verified;
    Ok(VerifiedAnswer {
        answer: grounded,
        dropped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::retrieve::fuse_channels;
    use app_domain::{ChunkId, Id};
    use llm_api::{AnswerV1, SchemaValidate, SourceKind};
    use search::RrfConfig;

    fn chunk_id(i: u8) -> ChunkId {
        let mut b = [0u8; 16];
        b[15] = i;
        Id::from_bytes(b)
    }

    fn pool_of(chunks: Vec<Chunk>) -> RetrievalResult {
        fuse_channels(&chunks, &[], RrfConfig::default(), 8)
    }

    fn answered(citations: Vec<Citation>) -> AnswerV1 {
        AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "The beta ships Friday.".into(),
            citations,
            confidence: 0.8,
            unanswered: false,
        }
    }

    #[test]
    fn resolving_citation_passes_and_is_rebuilt_from_truth() {
        let c = Chunk::note_block(
            chunk_id(1),
            Id::new(),
            "we agreed to ship the beta on Friday",
        );
        let retrieval = pool_of(vec![c.clone()]);
        // Model cites the real chunk but with a bogus snippet/source_kind.
        let bogus = Citation {
            chunk_id: chunk_id(1),
            source_kind: SourceKind::Task,
            source_id: Id::new(),
            t_start_ms: 999,
            snippet: "model-authored text".into(),
        };
        let v = verify(&answered(vec![bogus]), &retrieval, 0.0).unwrap();
        assert_eq!(v.answer.citations.len(), 1);
        // Rebuilt from ground truth — the note's real source_kind/source_id, not the
        // model's claims.
        assert_eq!(v.answer.citations[0].source_kind, SourceKind::NoteBlock);
        assert_eq!(v.answer.citations[0].source_id, c.source_id);
        assert_ne!(v.answer.citations[0].snippet, "model-authored text");
        v.answer.validate().unwrap();
    }

    #[test]
    fn fabricated_citation_is_dropped_and_downgraded() {
        let c = Chunk::note_block(chunk_id(1), Id::new(), "real evidence body");
        let retrieval = pool_of(vec![c]);
        // The only citation points at a chunk that was never retrieved.
        let fake = Citation {
            chunk_id: chunk_id(99),
            source_kind: SourceKind::NoteBlock,
            source_id: Id::new(),
            t_start_ms: 0,
            snippet: "hallucinated".into(),
        };
        let err = verify(&answered(vec![fake]), &retrieval, 0.0).unwrap_err();
        assert_eq!(err, UnansweredReason::NoVerifiableCitations);
    }

    #[test]
    fn mixed_real_and_fake_keeps_only_verified() {
        let c = Chunk::note_block(chunk_id(1), Id::new(), "real evidence body");
        let retrieval = pool_of(vec![c]);
        let real = Citation {
            chunk_id: chunk_id(1),
            source_kind: SourceKind::NoteBlock,
            source_id: Id::new(),
            t_start_ms: 0,
            snippet: "x".into(),
        };
        let fake = Citation {
            chunk_id: chunk_id(42),
            source_kind: SourceKind::NoteBlock,
            source_id: Id::new(),
            t_start_ms: 0,
            snippet: "y".into(),
        };
        let v = verify(&answered(vec![real, fake]), &retrieval, 0.0).unwrap();
        assert_eq!(v.answer.citations.len(), 1);
        assert_eq!(v.answer.citations[0].chunk_id, chunk_id(1));
        assert_eq!(v.dropped.len(), 1);
        assert_eq!(v.dropped[0].chunk_id, chunk_id(42));
    }

    #[test]
    fn model_refusal_is_respected() {
        let c = Chunk::note_block(chunk_id(1), Id::new(), "body");
        let retrieval = pool_of(vec![c]);
        let err = verify(&AnswerV1::unanswered(), &retrieval, 0.0).unwrap_err();
        assert_eq!(err, UnansweredReason::ModelRefused);
    }

    #[test]
    fn low_confidence_is_refused() {
        let c = Chunk::note_block(chunk_id(1), Id::new(), "body");
        let retrieval = pool_of(vec![c]);
        let real = Citation {
            chunk_id: chunk_id(1),
            source_kind: SourceKind::NoteBlock,
            source_id: Id::new(),
            t_start_ms: 0,
            snippet: "x".into(),
        };
        let err = verify(&answered(vec![real]), &retrieval, 0.9).unwrap_err();
        assert_eq!(err, UnansweredReason::LowConfidence);
    }
}

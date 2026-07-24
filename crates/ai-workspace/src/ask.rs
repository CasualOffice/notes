//! The Ask pipeline orchestration (**HLD §8.5** `ai.ask`): retrieve → RRF fuse →
//! (rerank) → grounded decode → **citation-verify** → verdict.
//!
//! [`ask`] wires the modules into the full flow and is the crate's primary entry
//! point. It never fabricates: a soft embedder failure degrades to lexical-only
//! retrieval; an empty pool, a model refusal, a malformed decode, or an unverifiable
//! citation all resolve to an [`AnswerVerdict::Unanswered`] rather than displaying
//! ungrounded text. Only a real transport/backend failure returns an [`AskError`].

use llm_api::{AnswerV1, ConstrainedLlm, GenerationPath};
use search::RrfConfig;

use app_domain::EntityRef;

use crate::chunk::Chunk;
use crate::decode::decode_answer;
use crate::error::AskResult;
use crate::retrieve::{
    fuse_channels, is_soft_vector_error, rerank_identity, RetrievalResult, Retriever,
};
use crate::suggest::{auto_links_from_answer, Suggestion};
use crate::verify::{verify, UnansweredReason, VerifiedAnswer};

/// Tunables for a single [`ask`] call. [`AskConfig::default`] matches HLD §8.5 /
/// Data Model §10.1 (RRF k=60, top-K evidence).
#[derive(Clone, Copy, Debug)]
pub struct AskConfig {
    /// Per-channel lexical (BM25) candidate cap before fusion.
    pub lexical_limit: usize,
    /// Vector-KNN candidate cap before fusion.
    pub vector_k: usize,
    /// Number of fused chunks rendered as numbered evidence in the prompt.
    pub evidence_top_k: usize,
    /// Decode token budget for the [`AnswerV1`].
    pub max_tokens: u32,
    /// Confidence floor below which a produced answer is refused (`0.0` disables).
    pub min_confidence: f32,
    /// RRF configuration (k=60 by default).
    pub rrf: RrfConfig,
}

impl Default for AskConfig {
    fn default() -> Self {
        Self {
            lexical_limit: 50,
            vector_k: 50,
            evidence_top_k: 8,
            max_tokens: 512,
            min_confidence: 0.0,
            rrf: RrfConfig::default(),
        }
    }
}

/// The context an [`ask`] runs against: the retrieval backend and the LLM. Both are
/// trait objects, so the same pipeline runs over the offline
/// [`InMemoryCorpus`](crate::InMemoryCorpus) + [`MockLlm`](llm_api::MockLlm) in tests
/// and the real storage-backed retriever + `llm-llamacpp` in production.
pub struct AskContext<'a> {
    /// The two-channel retriever (lexical ∪ vector).
    pub retriever: &'a dyn Retriever,
    /// The GBNF-constrained local LLM.
    pub llm: &'a dyn ConstrainedLlm,
    /// Pipeline tunables.
    pub config: AskConfig,
}

impl<'a> AskContext<'a> {
    /// A context with default [`AskConfig`].
    #[must_use]
    pub fn new(retriever: &'a dyn Retriever, llm: &'a dyn ConstrainedLlm) -> Self {
        Self {
            retriever,
            llm,
            config: AskConfig::default(),
        }
    }
}

/// A grounded, display-ready answer plus the evidence and provenance behind it.
#[derive(Clone, Debug)]
pub struct Answered {
    /// The verified answer (citations pruned to the resolvable set, rebuilt from
    /// truth).
    pub verified: VerifiedAnswer,
    /// The full retrieval result (pool, fused ranking, numbered evidence).
    pub retrieval: RetrievalResult,
    /// Which decode path produced the answer (direct / repaired / fallback).
    pub path: GenerationPath,
}

impl Answered {
    /// The numbered evidence chunks shown to the model.
    #[must_use]
    pub fn evidence(&self) -> &[Chunk] {
        &self.retrieval.evidence
    }

    /// Generate cited, reversible auto-link suggestions from this answer, linking
    /// `target` (e.g. the note being composed) to each distinct cited entity.
    #[must_use]
    pub fn auto_link_suggestions(&self, target: EntityRef) -> Vec<Suggestion> {
        auto_links_from_answer(target, &self.verified)
    }
}

/// A refusal: "I couldn't find this in your notes" (Data Model §14.2). Carries the
/// reason and the display answer ([`AnswerV1::unanswered`]).
#[derive(Clone, Debug)]
pub struct Unanswered {
    /// Why the pipeline refused.
    pub reason: UnansweredReason,
    /// The decode path, or `None` when refused before any decode (empty pool).
    pub path: Option<GenerationPath>,
    /// The canonical refusal answer (empty citations, `unanswered:true`).
    pub answer: AnswerV1,
}

/// The outcome of an [`ask`]: a grounded answer or a grounded refusal. Never an
/// ungrounded answer — that is the whole point of the citation-verify gate (N14).
#[derive(Clone, Debug)]
pub enum AnswerVerdict {
    Answered(Answered),
    Unanswered(Unanswered),
}

impl AnswerVerdict {
    fn refuse(reason: UnansweredReason, path: Option<GenerationPath>) -> Self {
        Self::Unanswered(Unanswered {
            reason,
            path,
            answer: AnswerV1::unanswered(),
        })
    }

    /// Whether a grounded answer is available for display.
    #[must_use]
    pub fn is_answered(&self) -> bool {
        matches!(self, Self::Answered(_))
    }

    /// The display answer: the verified answer when grounded, else the canonical
    /// refusal. Every citation it carries is guaranteed to resolve (N14).
    #[must_use]
    pub fn answer(&self) -> &AnswerV1 {
        match self {
            Self::Answered(a) => &a.verified.answer,
            Self::Unanswered(u) => &u.answer,
        }
    }
}

/// Run the full grounded Ask flow for `query`.
///
/// # Errors
/// Returns an [`AskError`](crate::AskError) only on a real transport/backend failure
/// (LLM not loaded / queue full / cancelled / decode error, or a durable storage
/// failure). Every *grounding* outcome — refusal included — is an `Ok(AnswerVerdict)`.
pub fn ask(ctx: &AskContext, query: &str) -> AskResult<AnswerVerdict> {
    let cfg = ctx.config;

    // 1. Retrieve — lexical always; vector degrades to empty on a soft model error.
    let lexical = ctx.retriever.lexical(query, cfg.lexical_limit)?;
    let vector = match ctx.retriever.vector(query, cfg.vector_k) {
        Ok(v) => v,
        Err(e) if is_soft_vector_error(&e) => Vec::new(),
        Err(e) => return Err(e),
    };

    // 2. Fuse (RRF) + optional rerank.
    let mut retrieval = fuse_channels(&lexical, &vector, cfg.rrf, cfg.evidence_top_k);
    rerank_identity(query, &mut retrieval.evidence);

    // Nothing retrieved ⇒ refuse without spending a decode (nothing to ground).
    if retrieval.is_empty() {
        return Ok(AnswerVerdict::refuse(
            UnansweredReason::NoEvidenceRetrieved,
            None,
        ));
    }

    // 3. Grounded decode → AnswerV1 (repair → deterministic-fallback contract).
    let outcome = decode_answer(ctx.llm, query, &retrieval.evidence, cfg.max_tokens)?;

    // 4. Citation-verify — the load-bearing gate.
    match verify(&outcome.value, &retrieval, cfg.min_confidence) {
        Ok(verified) => Ok(AnswerVerdict::Answered(Answered {
            verified,
            retrieval,
            path: outcome.path,
        })),
        Err(reason) => Ok(AnswerVerdict::refuse(reason, Some(outcome.path))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::InMemoryCorpus;
    use app_domain::{ChunkId, EntityKind, Id};
    use embeddings::MockEmbedder;
    use llm_api::{AnswerV1, MockLlm};

    fn chunk_id(i: u8) -> ChunkId {
        let mut b = [0u8; 16];
        b[15] = i;
        Id::from_bytes(b)
    }

    fn corpus() -> (InMemoryCorpus<MockEmbedder>, ChunkId, Id) {
        let note = Id::new();
        let cid = chunk_id(1);
        let chunks = vec![
            Chunk::note_block(
                cid,
                note,
                "we decided pricing stays at twelve dollars per seat",
            ),
            Chunk::note_block(
                chunk_id(2),
                Id::new(),
                "the offsite is scheduled for next month",
            ),
            Chunk::transcript(
                chunk_id(3),
                Id::new(),
                4000,
                "standup blockers and status updates",
            ),
        ];
        let corpus = InMemoryCorpus::index(chunks, MockEmbedder::with_dimension(64)).unwrap();
        (corpus, cid, note)
    }

    fn valid_answer(cid: ChunkId, note: Id) -> String {
        let ans = AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "Pricing stays at $12 per seat.".into(),
            citations: vec![Chunk::note_block(
                cid,
                note,
                "we decided pricing stays at twelve dollars per seat",
            )
            .to_citation()],
            confidence: 0.88,
            unanswered: false,
        };
        serde_json::to_string(&ans).unwrap()
    }

    #[test]
    fn ask_returns_grounded_answer_whose_every_citation_resolves() {
        let (corpus, cid, note) = corpus();
        let llm = MockLlm::always(valid_answer(cid, note));
        let ctx = AskContext::new(&corpus, &llm);
        let verdict = ask(&ctx, "what did we decide about pricing?").unwrap();

        assert!(verdict.is_answered());
        let ans = verdict.answer();
        assert!(!ans.citations.is_empty());
        // The N14 invariant: every displayed citation resolves to a retrieved chunk.
        if let AnswerVerdict::Answered(a) = &verdict {
            for c in &ans.citations {
                assert!(a.retrieval.resolve(c.chunk_id).is_some());
            }
        }
    }

    #[test]
    fn ask_refuses_when_model_returns_unanswered() {
        let (corpus, _, _) = corpus();
        let llm = MockLlm::always(serde_json::to_string(&AnswerV1::unanswered()).unwrap());
        let ctx = AskContext::new(&corpus, &llm);
        let verdict = ask(&ctx, "what did we decide about pricing?").unwrap();
        assert!(!verdict.is_answered());
        if let AnswerVerdict::Unanswered(u) = verdict {
            assert_eq!(u.reason, UnansweredReason::ModelRefused);
            assert!(u.answer.unanswered);
        } else {
            panic!("expected refusal");
        }
    }

    #[test]
    fn ask_catches_fabricated_citation_and_downgrades() {
        let (corpus, _, _) = corpus();
        // The model answers but cites a chunk_id that was never retrieved.
        let fabricated = AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "Pricing is $999.".into(),
            citations: vec![Chunk::note_block(chunk_id(200), Id::new(), "fake").to_citation()],
            confidence: 0.99,
            unanswered: false,
        };
        let llm = MockLlm::always(serde_json::to_string(&fabricated).unwrap());
        let ctx = AskContext::new(&corpus, &llm);
        let verdict = ask(&ctx, "what is the price?").unwrap();
        assert!(!verdict.is_answered());
        assert!(verdict.answer().citations.is_empty());
    }

    #[test]
    fn ask_over_empty_corpus_refuses_without_decoding() {
        let corpus = InMemoryCorpus::index(Vec::new(), MockEmbedder::with_dimension(32)).unwrap();
        // A poisoned LLM would panic if called — it must not be reached.
        let llm = MockLlm::scripted(Vec::new());
        let ctx = AskContext::new(&corpus, &llm);
        let verdict = ask(&ctx, "anything").unwrap();
        if let AnswerVerdict::Unanswered(u) = verdict {
            assert_eq!(u.reason, UnansweredReason::NoEvidenceRetrieved);
            assert!(u.path.is_none()); // decode never ran
        } else {
            panic!("expected refusal");
        }
    }

    #[test]
    fn ask_produces_cited_auto_link_suggestion() {
        let (corpus, cid, note) = corpus();
        let llm = MockLlm::always(valid_answer(cid, note));
        let ctx = AskContext::new(&corpus, &llm);
        let verdict = ask(&ctx, "pricing?").unwrap();
        let AnswerVerdict::Answered(a) = verdict else {
            panic!("expected answer");
        };
        // A different note being composed links to the cited note.
        let target = EntityRef::new(EntityKind::Note, Id::new());
        let suggestions = a.auto_link_suggestions(target);
        assert_eq!(suggestions.len(), 1);
        assert!(!suggestions[0].citations.is_empty());
        assert_eq!(suggestions[0].proposed["dst_id"], note.to_string());
    }
}

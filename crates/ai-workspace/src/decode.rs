//! Grounded decode — the middle of the Ask flow (**HLD §8.5**, **Data Model
//! §14.1/§14.2**). Builds a prompt from *numbered* retrieved evidence and drives the
//! [`ConstrainedLlm`] through the **repair → deterministic fallback** contract of
//! `llm-api` to produce an [`AnswerV1`].
//!
//! The fallback is [`AnswerV1::unanswered`]: if the model twice fails to emit schema-
//! valid JSON, the pipeline refuses rather than displaying anything ("Evidence or
//! nothing"). Because `generate_structured` already parses + [`SchemaValidate`]s the
//! output, a syntactically broken or schema-violating answer can never reach
//! citation-verify — it becomes an honest `unanswered`.
//!
//! ## Grammar seam
//! [`answer_grammar`] returns a representative GBNF for [`AnswerV1`]. The
//! *authoritative* grammar is owned by the `llm-llamacpp` backend (HLD §9.2); this
//! text documents the shape and is what a real GBNF-constrained decoder would be
//! handed. The [`MockLlm`](llm_api::MockLlm) ignores it (it replays scripted JSON),
//! so the whole flow is exercised offline.

use llm_api::{
    generate_structured, AnswerV1, ConstrainedLlm, GenerationOutcome, GenerationRequest, Grammar,
};

use crate::chunk::Chunk;
use crate::error::AskResult;
use crate::text::snippet;

/// System preamble: the grounding contract, stated to the model.
const SYSTEM: &str = "You answer strictly from the NUMBERED evidence below. \
Cite every claim with the chunk_id of the evidence that supports it. \
Never invent facts, owners, dates, or citations. \
If the evidence does not contain the answer, return {\"unanswered\": true}.";

/// Per-evidence snippet budget in the prompt (chars).
const EVIDENCE_SNIPPET_CHARS: usize = 400;

/// A representative GBNF grammar for [`AnswerV1`] (the real one is backend-owned —
/// HLD §9.2). Kept as a documented seam; [`MockLlm`](llm_api::MockLlm) ignores it.
#[must_use]
pub fn answer_grammar() -> Grammar {
    // Minimal illustrative GBNF: a JSON object whose shape matches AnswerV1. The
    // production grammar pins field names/order exactly; this documents the seam.
    Grammar::new(
        r#"root ::= "{" ws "\"schema\"" ws ":" ws "\"AnswerV1\"" ws "," ws object-rest "}"
object-rest ::= (char)*
char ::= [^\x00]
ws ::= [ \t\n]*"#,
    )
}

/// Render the numbered evidence block a grounded prompt is built from. Each entry is
/// `[n] (source_kind chunk_id) text…`, so the model can cite by `chunk_id`.
#[must_use]
pub fn render_evidence(evidence: &[Chunk]) -> String {
    let mut out = String::new();
    for (i, c) in evidence.iter().enumerate() {
        let kind = match c.source_kind {
            llm_api::SourceKind::NoteBlock => "note_block",
            llm_api::SourceKind::TranscriptWindow => "transcript_window",
            llm_api::SourceKind::Task => "task",
            llm_api::SourceKind::Reminder => "reminder",
        };
        out.push_str(&format!(
            "[{n}] (source_kind={kind} chunk_id={id}) {text}\n",
            n = i + 1,
            id = c.chunk_id,
            text = snippet(&c.text, EVIDENCE_SNIPPET_CHARS),
        ));
    }
    out
}

/// Build the full grounded prompt for `query` over `evidence`.
#[must_use]
pub fn build_prompt(query: &str, evidence: &[Chunk]) -> String {
    format!(
        "Question: {query}\n\nEvidence:\n{evidence}\n\
         Answer as AnswerV1 JSON, citing only chunk_ids that appear above.",
        evidence = render_evidence(evidence),
    )
}

/// Decode a grounded [`AnswerV1`] for `query` over `evidence`, honouring the
/// repair → deterministic-fallback contract. The fallback is an honest refusal, so a
/// malformed model can only ever yield `unanswered` — never fabrication.
///
/// # Errors
/// Propagates a transport/backend [`AskError`](crate::AskError) from the LLM (model
/// not loaded, queue full, cancelled, decode failure) — never a schema violation.
pub fn decode_answer(
    llm: &dyn ConstrainedLlm,
    query: &str,
    evidence: &[Chunk],
    max_tokens: u32,
) -> AskResult<GenerationOutcome<AnswerV1>> {
    let prompt = build_prompt(query, evidence);
    let req = GenerationRequest {
        system: Some(SYSTEM.to_string()),
        prompt,
        max_tokens,
        temperature: 0.0,
        seed: Some(0),
    };
    let grammar = answer_grammar();
    let outcome = generate_structured::<AnswerV1, dyn ConstrainedLlm, _>(
        llm,
        &req,
        &grammar,
        AnswerV1::unanswered,
    )?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::{ChunkId, Id};
    use llm_api::{GenerationPath, MockLlm};

    fn chunk_id(i: u8) -> ChunkId {
        let mut b = [0u8; 16];
        b[15] = i;
        Id::from_bytes(b)
    }

    fn evidence() -> Vec<Chunk> {
        vec![Chunk::note_block(
            chunk_id(1),
            Id::new(),
            "we decided pricing stays at $12 per seat",
        )]
    }

    #[test]
    fn prompt_numbers_evidence_and_exposes_chunk_ids() {
        let ev = evidence();
        let p = build_prompt("what is the price?", &ev);
        assert!(p.contains("[1]"));
        assert!(p.contains(&ev[0].chunk_id.to_string()));
        assert!(p.contains("note_block"));
    }

    #[test]
    fn malformed_output_falls_back_to_unanswered() {
        let llm = MockLlm::scripted(vec!["not json".into(), "still not json".into()]);
        let out = decode_answer(&llm, "q", &evidence(), 256).unwrap();
        assert_eq!(out.path, GenerationPath::DeterministicFallback);
        assert!(out.value.unanswered);
    }

    #[test]
    fn valid_output_decodes_directly() {
        let ans = AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "pricing stays at $12/seat".into(),
            citations: vec![Chunk::note_block(chunk_id(1), Id::new(), "x").to_citation()],
            confidence: 0.9,
            unanswered: false,
        };
        let llm = MockLlm::always(serde_json::to_string(&ans).unwrap());
        let out = decode_answer(&llm, "q", &evidence(), 256).unwrap();
        assert_eq!(out.path, GenerationPath::Direct);
        assert!(!out.value.unanswered);
    }
}

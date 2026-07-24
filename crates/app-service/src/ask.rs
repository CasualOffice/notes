//! Grounded "Ask your notes" surface (HLD §8.5 `ai.ask`, Data Model §14.2).
//!
//! Wires the real `ai-workspace` [`ask`](ai_workspace::ask) pipeline —
//! retrieve → RRF-fuse → grounded decode → **citation-verify** — over the
//! **offline mock backends** ([`MockEmbedder`](embeddings::MockEmbedder) +
//! [`MockLlm`](llm_api::MockLlm)); no native model, no socket (CLAUDE.md
//! offline invariant). Corpus is rebuilt per call from the live `block` and
//! `transcript_segment` rows so the answer is grounded in the user's actual notes
//! and meeting transcripts. The real embedder + `llm-llamacpp` plug into the same
//! [`Retriever`](ai_workspace::Retriever)/`ConstrainedLlm` seams in a later pass.
//!
//! Because the mock LLM cannot generate, the surfaced answer is **extractive**: the
//! host crafts a candidate [`AnswerV1`](ai_workspace::AnswerV1) that cites the top
//! retrieved chunk, then runs the full pipeline so the **citation-verify gate still
//! decides** — an answer whose citation cannot be resolved becomes `unanswered`,
//! never displayed (N14 / refusal-over-hallucination). When nothing is retrieved the
//! pipeline refuses honestly.

use ai_workspace::{ask, AnswerV1, AskContext, Chunk, InMemoryCorpus, Retriever};
use app_domain::{AppError, AppResult, Id};
use embeddings::MockEmbedder;
use llm_api::MockLlm;

use crate::Service;

/// Embedding width for the offline mock retriever (matches the ai-workspace tests).
const MOCK_DIMS: usize = 64;
/// Max characters of the cited chunk surfaced as the extractive answer body.
const ANSWER_CHARS: usize = 400;

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}

impl Service {
    /// `ai.ask` — run the grounded Ask pipeline for `query` over the user's notes +
    /// transcripts and return the [`AnswerV1`](ai_workspace::AnswerV1) verdict
    /// (grounded answer with resolvable citations, or `unanswered`). The returned
    /// JSON is the serialized `AnswerV1`; every citation it carries is guaranteed to
    /// resolve to a real chunk (the citation-verify gate, N14).
    pub fn ai_ask(&self, query: &str) -> AppResult<serde_json::Value> {
        let chunks = self.ask_corpus()?;
        let corpus = InMemoryCorpus::index(chunks, MockEmbedder::with_dimension(MOCK_DIMS))
            .map_err(|e| AppError::Internal(format!("ask index failed: {e}")))?;

        // Craft the candidate answer the mock LLM will "return": cite the top
        // lexical chunk with an extractive body. If nothing matches, feed the
        // canonical refusal so the pipeline resolves to `unanswered` honestly.
        let response = match top_lexical(&corpus, query) {
            Some(chunk) => {
                let ans = AnswerV1 {
                    schema: AnswerV1::SCHEMA.to_string(),
                    answer: extractive(&chunk.text),
                    citations: vec![chunk.to_citation()],
                    confidence: 0.6,
                    unanswered: false,
                };
                serde_json::to_string(&ans)?
            }
            None => serde_json::to_string(&AnswerV1::unanswered())?,
        };

        let llm = MockLlm::always(response);
        let ctx = AskContext::new(&corpus, &llm);
        let verdict =
            ask(&ctx, query).map_err(|e| AppError::Internal(format!("ask failed: {e}")))?;

        // Whether grounded or refused, `answer()` is the display AnswerV1 — a refusal
        // carries empty citations + `unanswered:true` (never ungrounded text, N14).
        Ok(serde_json::to_value(verdict.answer())?)
    }

    /// Build the in-memory retrieval corpus from live note blocks and final-pass
    /// transcript segments (Data Model §9.2 `chunk`, in-memory subset). `chunk_id`s
    /// are ephemeral per call — citation-verify only needs them consistent within
    /// this one ask; navigation uses the stable `entity`/`source_id` on each chunk.
    fn ask_corpus(&self) -> AppResult<Vec<Chunk>> {
        self.read(|c| {
            let mut chunks: Vec<Chunk> = Vec::new();

            let mut bstmt = c.prepare(
                "SELECT b.note_id, b.text_content FROM block b \
                 JOIN entity e ON e.id = b.note_id \
                 WHERE e.deleted_at IS NULL AND b.text_content IS NOT NULL \
                   AND trim(b.text_content) <> ''",
            )?;
            let blocks = bstmt
                .query_map([], |r| {
                    Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (note_id, text) in blocks {
                chunks.push(Chunk::note_block(
                    Id::new(),
                    Id::from_bytes(to16(&note_id)),
                    text,
                ));
            }

            let mut tstmt = c.prepare(
                "SELECT session_id, t_start_ms, text FROM transcript_segment \
                 WHERE pass = 'final' AND trim(text) <> ''",
            )?;
            let segs = tstmt
                .query_map([], |r| {
                    Ok((
                        r.get::<_, Vec<u8>>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (session_id, t_start_ms, text) in segs {
                chunks.push(Chunk::transcript(
                    Id::new(),
                    Id::from_bytes(to16(&session_id)),
                    t_start_ms,
                    text,
                ));
            }

            Ok(chunks)
        })
    }
}

/// The single best lexical (BM25) chunk for `query`, or `None` when the corpus
/// shares no query term (the refuse-without-decoding path).
fn top_lexical(corpus: &InMemoryCorpus<MockEmbedder>, query: &str) -> Option<Chunk> {
    corpus
        .lexical(query, 1)
        .ok()
        .and_then(|v| v.into_iter().next())
}

/// A leading, whitespace-trimmed excerpt of the cited chunk — the extractive answer
/// body. Never the whole block; a pointer-with-context, mirroring citation snippets.
fn extractive(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.chars().count() <= ANSWER_CHARS {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(ANSWER_CHARS).collect();
    format!("{}…", cut.trim_end())
}

#[cfg(test)]
mod tests {
    use crate::{EventSink, Service};
    use app_domain::Id;
    use storage::{Paths, Store};

    fn svc() -> Service {
        let dir = std::env::temp_dir().join(format!("cn-ask-{}", Id::new()));
        let store = Store::open_memory(Paths::new(dir)).expect("open_memory");
        let sink: EventSink = Box::new(|_| {});
        Service::new(store, "test", sink)
    }

    #[test]
    fn ask_grounds_answer_in_note_and_citation_resolves() {
        let s = svc();
        s.notes_import_markdown(
            "Pricing stays at twelve dollars per seat for the beta.",
            None,
        )
        .unwrap();
        let v = s.ai_ask("what is the pricing per seat?").unwrap();
        assert_eq!(v["unanswered"], serde_json::json!(false));
        let cites = v["citations"].as_array().expect("citations array");
        assert!(!cites.is_empty(), "a grounded answer carries a citation");
        // The citation points at a real note (navigation target).
        assert_eq!(cites[0]["source_kind"], serde_json::json!("note_block"));
    }

    #[test]
    fn ask_refuses_over_empty_corpus() {
        let s = svc();
        let v = s.ai_ask("anything at all?").unwrap();
        assert_eq!(v["unanswered"], serde_json::json!(true));
        assert!(v["citations"].as_array().unwrap().is_empty());
    }
}

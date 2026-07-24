//! Retrieval + fusion: the first half of the Ask flow (**HLD §8.5**, **Data Model
//! §10.1**). Two ranked channels — lexical BM25 and vector KNN — fused by
//! **Reciprocal Rank Fusion** into one evidence set.
//!
//! ## What is real vs. a documented seam
//! In production the lexical channel is `search.query` FTS5/BM25 executed by
//! `storage`, and the vector channel is `embeddings` + `sqlite-vec` KNN. Neither the
//! FTS5 tables nor the native `vec0` extension exist in this pure-Rust layer, so this
//! module ships a self-contained, **offline-testable** [`InMemoryCorpus`] that:
//!   - scores the lexical channel with an in-Rust **BM25** (`k1=1.2`, `b=0.75`) — the
//!     same ranking family `bm25(fts_*)` produces (Data Model §10.1), and
//!   - scores the vector channel with the real [`embeddings::VectorStore`] cosine KNN
//!     over an in-memory SQLite connection (no native extension).
//!
//! Fusion itself is **not** re-implemented: it delegates to
//! [`search::rrf_fuse`] verbatim, so the exact production RRF (k=60, no score
//! normalization, id tie-break) ranks the evidence. A doc that hits *both* channels
//! sums two reciprocal-rank contributions and so outranks a single-channel hit — the
//! cross-source-agreement property RRF exists for.
//!
//! ## Rerank seam
//! [`RetrievalResult::evidence`] is the top-K feeding the grounded prompt. The
//! optional bge-reranker of HLD §8.5 is a documented hook ([`rerank_identity`]); it
//! reorders `evidence` in place and is a no-op here.

use std::collections::HashMap;

use rusqlite::Connection;

use app_domain::{ChunkId, EntityRef};
use embeddings::{ContentHash, Embedder, EmbeddingError, VectorStore};
use search::{rrf_fuse, FusedHit, RrfConfig};

use crate::chunk::Chunk;
use crate::error::{AskError, AskResult};
use crate::text::tokenize;

/// A two-channel retriever over a corpus. The Ask pipeline calls both channels and
/// fuses them; an implementation degrades gracefully (an unavailable embedding model
/// surfaces as [`EmbeddingError::ModelNotLoaded`], which the pipeline turns into
/// FTS-only retrieval rather than a hard failure — Architecture §10).
pub trait Retriever {
    /// Lexical BM25 candidates for `query`, best-first, capped at `limit`.
    ///
    /// # Errors
    /// Returns an [`AskError`] only on a durable backend failure.
    fn lexical(&self, query: &str, limit: usize) -> AskResult<Vec<Chunk>>;

    /// Vector-KNN candidates for `query`, best-first, capped at `k`.
    ///
    /// # Errors
    /// Returns [`AskError::Retrieval`] wrapping [`EmbeddingError::ModelNotLoaded`]
    /// when the embedding model is not resident — the pipeline treats that as a
    /// soft, retryable degrade to lexical-only, not a failure.
    fn vector(&self, query: &str, k: usize) -> AskResult<Vec<Chunk>>;
}

/// BM25 free-parameters (Robertson/Sparck-Jones). The canonical defaults, matching
/// the ranking family SQLite FTS5 `bm25()` produces (Data Model §10.1).
const BM25_K1: f64 = 1.2;
const BM25_B: f64 = 0.75;

/// A compact in-memory BM25 index over a fixed chunk set.
#[derive(Clone, Debug)]
struct Bm25Index {
    /// Per-document term frequencies (parallel to the corpus chunk order).
    tf: Vec<HashMap<String, u32>>,
    /// Per-document token length.
    len: Vec<usize>,
    /// Document frequency per term.
    df: HashMap<String, usize>,
    /// Mean document length.
    avgdl: f64,
    /// Document count.
    n: usize,
}

impl Bm25Index {
    fn build(chunks: &[Chunk]) -> Self {
        let mut tf = Vec::with_capacity(chunks.len());
        let mut len = Vec::with_capacity(chunks.len());
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut total_len: usize = 0;

        for c in chunks {
            let tokens = tokenize(&c.text);
            total_len += tokens.len();
            let mut counts: HashMap<String, u32> = HashMap::new();
            for t in &tokens {
                *counts.entry(t.clone()).or_insert(0) += 1;
            }
            for term in counts.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
            len.push(tokens.len());
            tf.push(counts);
        }

        let n = chunks.len();
        let avgdl = if n == 0 {
            0.0
        } else {
            total_len as f64 / n as f64
        };
        Self {
            tf,
            len,
            df,
            avgdl,
            n,
        }
    }

    /// The BM25 idf of a term (the `+1` "probabilistic idf" form, always positive).
    fn idf(&self, term: &str) -> f64 {
        let df = *self.df.get(term).unwrap_or(&0) as f64;
        let n = self.n as f64;
        (((n - df + 0.5) / (df + 0.5)) + 1.0).ln()
    }

    /// BM25 score of document `doc` against the query terms.
    fn score(&self, doc: usize, query_terms: &[String]) -> f64 {
        let dl = self.len[doc] as f64;
        let denom_norm = BM25_K1 * (1.0 - BM25_B + BM25_B * (dl / self.avgdl.max(1.0)));
        let mut score = 0.0;
        for term in query_terms {
            let Some(&freq) = self.tf[doc].get(term) else {
                continue;
            };
            let f = f64::from(freq);
            score += self.idf(term) * (f * (BM25_K1 + 1.0)) / (f + denom_norm);
        }
        score
    }
}

/// An offline, fully-testable corpus implementing both retrieval channels with **no
/// model and no native extension**. Lexical ranking is in-Rust BM25; vector ranking
/// is the real [`embeddings::VectorStore`] cosine KNN over an in-memory SQLite
/// connection, embedded with the supplied [`Embedder`].
#[derive(Debug)]
pub struct InMemoryCorpus<E: Embedder> {
    chunks: Vec<Chunk>,
    by_id: HashMap<ChunkId, usize>,
    bm25: Bm25Index,
    conn: Connection,
    store: VectorStore,
    embedder: E,
}

impl<E: Embedder> InMemoryCorpus<E> {
    /// Index `chunks`, embedding each with `embedder`. Every chunk is stored in a
    /// private in-memory vector store; the BM25 index is built over the same set.
    ///
    /// # Errors
    /// Propagates embedder / storage failures raised while indexing.
    pub fn index(chunks: Vec<Chunk>, embedder: E) -> AskResult<Self> {
        let conn = Connection::open_in_memory().map_err(AskError::from)?;
        let store = VectorStore::for_embedder(&embedder);
        store.ensure_schema(&conn)?;

        let mut by_id = HashMap::with_capacity(chunks.len());
        for (i, c) in chunks.iter().enumerate() {
            by_id.insert(c.chunk_id, i);
            let vector = embedder.embed_one(&c.text)?;
            store.upsert(&conn, c.chunk_id, &ContentHash::of(&c.text), &vector)?;
        }

        let bm25 = Bm25Index::build(&chunks);
        Ok(Self {
            chunks,
            by_id,
            bm25,
            conn,
            store,
            embedder,
        })
    }

    /// The indexed chunks (in insertion order).
    #[must_use]
    pub fn chunks(&self) -> &[Chunk] {
        &self.chunks
    }
}

impl<E: Embedder> Retriever for InMemoryCorpus<E> {
    fn lexical(&self, query: &str, limit: usize) -> AskResult<Vec<Chunk>> {
        let terms = dedup_tokens(query);
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // Score every doc; keep positives (a doc sharing no query term scores 0).
        let mut scored: Vec<(f64, usize)> = (0..self.chunks.len())
            .map(|i| (self.bm25.score(i, &terms), i))
            .filter(|(s, _)| *s > 0.0)
            .collect();
        // Descending score; ascending chunk_id tie-break (deterministic, mirrors
        // search::rrf_fuse and embeddings::knn).
        scored.sort_by(|a, b| {
            b.0.partial_cmp(&a.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| self.chunks[a.1].chunk_id.cmp(&self.chunks[b.1].chunk_id))
        });
        scored.truncate(limit);
        Ok(scored
            .into_iter()
            .map(|(_, i)| self.chunks[i].clone())
            .collect())
    }

    fn vector(&self, query: &str, k: usize) -> AskResult<Vec<Chunk>> {
        let qvec = self.embedder.embed_one(query)?; // ModelNotLoaded degrades upstream
        let neighbors = self.store.knn(&self.conn, &qvec, k)?;
        let mut out = Vec::with_capacity(neighbors.len());
        for nb in neighbors {
            if let Some(&i) = self.by_id.get(&nb.chunk_id) {
                out.push(self.chunks[i].clone());
            }
        }
        Ok(out)
    }
}

/// The fused output of the two retrieval channels.
#[derive(Clone, Debug)]
pub struct RetrievalResult {
    /// Every retrieved chunk (union of both channels, de-duplicated by `chunk_id`).
    /// This is the **candidate pool** citation-verify resolves against — a citation
    /// is valid iff it points at a chunk that was actually retrieved.
    pub pool: Vec<Chunk>,
    /// The RRF entity ranking (`search::rrf_fuse` output), best-first.
    pub fused: Vec<FusedHit>,
    /// The top-K chunks fed as numbered evidence to the grounded prompt.
    pub evidence: Vec<Chunk>,
}

impl RetrievalResult {
    /// Look a chunk up in the candidate pool by id (the citation-verify primitive).
    #[must_use]
    pub fn resolve(&self, chunk_id: ChunkId) -> Option<&Chunk> {
        self.pool.iter().find(|c| c.chunk_id == chunk_id)
    }

    /// Whether any evidence was retrieved at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pool.is_empty()
    }
}

/// Fuse the two channels' ranked chunk lists into a [`RetrievalResult`].
///
/// The entity ranking is produced by [`search::rrf_fuse`] over the per-channel
/// entity lists; the numbered `evidence` walks that ranking and gathers each
/// entity's retrieved chunks (best channel-rank first) up to `evidence_top_k`.
#[must_use]
pub fn fuse_channels(
    lexical: &[Chunk],
    vector: &[Chunk],
    cfg: RrfConfig,
    evidence_top_k: usize,
) -> RetrievalResult {
    // Per-channel entity lists (dedup, best-rank-first) → RRF.
    let lex_entities = dedup_entities(lexical);
    let vec_entities = dedup_entities(vector);
    let fused = rrf_fuse(&[lex_entities, vec_entities], cfg);

    // Candidate pool: union by chunk_id, and each chunk's best rank across channels
    // (min position) for a deterministic within-entity ordering.
    let mut pool: Vec<Chunk> = Vec::new();
    let mut best_rank: HashMap<ChunkId, usize> = HashMap::new();
    let mut seen: HashMap<ChunkId, usize> = HashMap::new();
    for channel in [lexical, vector] {
        for (rank, c) in channel.iter().enumerate() {
            best_rank
                .entry(c.chunk_id)
                .and_modify(|r| *r = (*r).min(rank))
                .or_insert(rank);
            if let std::collections::hash_map::Entry::Vacant(e) = seen.entry(c.chunk_id) {
                e.insert(pool.len());
                pool.push(c.clone());
            }
        }
    }

    // Evidence: walk fused entities; within each, its pool chunks by (best_rank, id).
    let mut by_entity: HashMap<EntityRef, Vec<usize>> = HashMap::new();
    for (i, c) in pool.iter().enumerate() {
        by_entity.entry(c.entity).or_default().push(i);
    }
    let mut evidence: Vec<Chunk> = Vec::new();
    for hit in &fused {
        let Some(idxs) = by_entity.get(&hit.entity) else {
            continue;
        };
        let mut ordered = idxs.clone();
        ordered.sort_by(|&a, &b| {
            let ra = best_rank
                .get(&pool[a].chunk_id)
                .copied()
                .unwrap_or(usize::MAX);
            let rb = best_rank
                .get(&pool[b].chunk_id)
                .copied()
                .unwrap_or(usize::MAX);
            ra.cmp(&rb)
                .then_with(|| pool[a].chunk_id.cmp(&pool[b].chunk_id))
        });
        for i in ordered {
            if evidence.len() >= evidence_top_k {
                break;
            }
            evidence.push(pool[i].clone());
        }
        if evidence.len() >= evidence_top_k {
            break;
        }
    }

    RetrievalResult {
        pool,
        fused,
        evidence,
    }
}

/// The optional rerank hook of HLD §8.5 (bge-reranker over top-K). This identity
/// stub leaves order unchanged; a real reranker replaces it without touching the
/// pipeline's shape.
pub fn rerank_identity(_query: &str, _evidence: &mut [Chunk]) {}

/// First-occurrence-preserving list of the distinct entities in a chunk list.
fn dedup_entities(chunks: &[Chunk]) -> Vec<EntityRef> {
    let mut seen: Vec<EntityRef> = Vec::new();
    for c in chunks {
        if !seen.contains(&c.entity) {
            seen.push(c.entity);
        }
    }
    seen
}

/// Distinct lowercased tokens of `s`, order-stable.
fn dedup_tokens(s: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in tokenize(s) {
        if !out.contains(&t) {
            out.push(t);
        }
    }
    out
}

/// Soft-degrade classifier: a vector-channel error caused by an unavailable model
/// is recoverable (fall back to lexical-only); anything else is a hard failure.
#[must_use]
pub(crate) fn is_soft_vector_error(err: &AskError) -> bool {
    matches!(err, AskError::Retrieval(EmbeddingError::ModelNotLoaded(_)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::{EntityKind, Id};
    use embeddings::MockEmbedder;

    fn chunk_id(i: u8) -> ChunkId {
        let mut b = [0u8; 16];
        b[15] = i;
        Id::from_bytes(b)
    }

    fn corpus() -> InMemoryCorpus<MockEmbedder> {
        let note_a = Id::new();
        let note_b = Id::new();
        let sess = Id::new();
        let chunks = vec![
            Chunk::note_block(chunk_id(1), note_a, "quarterly revenue planning and budget"),
            Chunk::note_block(chunk_id(2), note_b, "weekend hiking trip in the mountains"),
            Chunk::transcript(chunk_id(3), sess, 5000, "team standup notes and blockers"),
        ];
        InMemoryCorpus::index(chunks, MockEmbedder::with_dimension(64)).unwrap()
    }

    #[test]
    fn bm25_ranks_lexical_overlap_first() {
        let c = corpus();
        let hits = c.lexical("revenue budget planning", 10).unwrap();
        assert!(!hits.is_empty());
        assert_eq!(hits[0].chunk_id, chunk_id(1));
    }

    #[test]
    fn lexical_ignores_non_matching_docs() {
        let c = corpus();
        // Only chunk 2 shares tokens; the others score 0 and are filtered out.
        let hits = c.lexical("hiking mountains", 10).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].chunk_id, chunk_id(2));
    }

    #[test]
    fn vector_channel_returns_semantic_nearest() {
        let c = corpus();
        // MockEmbedder is deterministic: the identical text is the nearest vector.
        let hits = c.vector("weekend hiking trip in the mountains", 3).unwrap();
        assert_eq!(hits[0].chunk_id, chunk_id(2));
    }

    #[test]
    fn empty_query_yields_no_lexical_hits() {
        let c = corpus();
        assert!(c.lexical("   ", 10).unwrap().is_empty());
    }

    #[test]
    fn rrf_ranks_dual_channel_hit_above_single_channel() {
        // Doc X (entity ex) is rank-1 in BOTH channels; doc Y only in lexical, doc Z
        // only in vector. RRF must place X first (it sums two contributions).
        let ex = EntityRef::new(EntityKind::Note, Id::new());
        let ey = EntityRef::new(EntityKind::Note, Id::new());
        let ez = EntityRef::new(EntityKind::Note, Id::new());
        let cx = Chunk {
            chunk_id: chunk_id(10),
            entity: ex,
            ..Chunk::note_block(chunk_id(10), ex.id, "x")
        };
        let cy = Chunk::note_block(chunk_id(11), ey.id, "y");
        let cz = Chunk::note_block(chunk_id(12), ez.id, "z");

        let lexical = vec![cx.clone(), cy.clone()];
        let vector = vec![cz.clone(), cx.clone()];
        let res = fuse_channels(&lexical, &vector, RrfConfig::default(), 8);

        assert_eq!(res.fused[0].entity, ex);
        // The dual-channel entity strictly outscores either single-channel entity.
        assert!(res.fused[0].score > res.fused[1].score);
        // The pool is the union, de-duplicated (X appeared in both channels once).
        assert_eq!(res.pool.len(), 3);
    }

    #[test]
    fn evidence_is_capped_at_top_k() {
        let e = EntityRef::new(EntityKind::Note, Id::new());
        let lexical: Vec<Chunk> = (0..5)
            .map(|i| Chunk::note_block(chunk_id(20 + i), e.id, format!("body {i}")))
            .collect();
        let res = fuse_channels(&lexical, &[], RrfConfig::default(), 2);
        assert_eq!(res.evidence.len(), 2);
        assert_eq!(res.pool.len(), 5);
    }

    #[test]
    fn resolve_finds_pooled_chunk_only() {
        let c = Chunk::note_block(chunk_id(30), Id::new(), "body");
        let res = fuse_channels(std::slice::from_ref(&c), &[], RrfConfig::default(), 8);
        assert!(res.resolve(chunk_id(30)).is_some());
        assert!(res.resolve(chunk_id(99)).is_none());
    }
}

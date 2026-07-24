//! The [`VectorStore`]: per-chunk embedding persistence + brute-force cosine KNN.
//!
//! Implements the retrieval-side of **Data Model §9.3** (`embedding` / `vec_chunk`)
//! and the vector channel of the hybrid search in **§10.1 / HLD §8.5**, at the
//! layer that needs **no native extension**. Instead of the `sqlite-vec` `vec0`
//! virtual table it keeps each embedding as a little-endian `f32` blob and scores
//! candidates with a Rust cosine loop. The persisted provenance columns
//! (`embed_model`, `dims`, `content_hash`) are exactly those of §9.3, so swapping
//! in `sqlite-vec` later is a storage change, not an API change.
//!
//! ## Isolation note
//! `storage`'s migration does not yet create the `chunk`/`embedding`/`vec_chunk`
//! tables (only `fts_chunk`). To stay inside its own crate boundary this store owns
//! a self-managed [`EMBEDDING_CHUNK_DDL`] table over any [`rusqlite::Connection`]
//! (in-memory in tests, the SQLCipher writer in production). It holds **no** foreign
//! key to `chunk`, so it composes with `storage`'s `with_read`/transaction closures
//! without coupling to a schema this crate cannot edit.
//!
//! ## `sqlite-vec` seam (do NOT load a runtime extension now)
//! When the extension is available, the migration adds the vec0 virtual table
//! ([`VEC_CHUNK_DDL`], quoted verbatim from §9.3). [`VectorStore::upsert`] then also
//! writes the (int8-quantized) vector into `vec_chunk`, and [`VectorStore::knn`]
//! delegates to a `WHERE embedding MATCH ?1 ORDER BY distance LIMIT k` query instead
//! of the Rust loop. The public API here — [`Neighbor`], `knn`, `upsert` — is
//! unchanged by that switch; only the private body changes.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

use app_domain::{ChunkId, Id, Timestamp};

use crate::embedder::Embedder;
use crate::error::{EmbeddingError, EmbeddingResult};
use crate::math;

/// DDL for the self-managed brute-force embedding table (see module docs). Applied
/// idempotently by [`VectorStore::ensure_schema`].
pub const EMBEDDING_CHUNK_DDL: &str = "\
CREATE TABLE IF NOT EXISTS embedding_chunk (
  chunk_id     BLOB PRIMARY KEY,   -- chunk.id (UUIDv7, 16-byte BLOB)
  embed_model  TEXT NOT NULL,      -- provenance, e.g. 'embeddinggemma-300m@256d-int8'
  dims         INTEGER NOT NULL,   -- vector length
  content_hash TEXT NOT NULL,      -- BLAKE3 gate for incremental re-embed
  vector       BLOB NOT NULL,      -- little-endian f32[dims]
  created_at   INTEGER NOT NULL    -- epoch-ms UTC
);
CREATE INDEX IF NOT EXISTS idx_embedding_chunk_model ON embedding_chunk(embed_model);";

/// The `sqlite-vec` virtual-table DDL quoted verbatim from Data Model §9.3, kept
/// here as documentation of the seam. **Not executed** by this crate — loading the
/// `vec0` extension is a later phase.
pub const VEC_CHUNK_DDL: &str = "\
CREATE VIRTUAL TABLE vec_chunk USING vec0(
  chunk_id  TEXT PRIMARY KEY,
  embedding FLOAT[256]
);";

/// Outcome of an [`VectorStore::upsert`] — records whether the content-hash gate
/// (Data Model §9.2: *"if unchanged, skip … embedding work"*) admitted the write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UpsertOutcome {
    /// No prior row for this chunk — a new embedding was stored.
    Inserted,
    /// A prior row existed with a different `content_hash` (or `embed_model`) — it
    /// was re-embedded.
    Updated,
    /// The stored `content_hash` and `embed_model` already match — the write was
    /// skipped (the incremental gate fired).
    Unchanged,
}

impl UpsertOutcome {
    /// Whether a vector was actually written (`Inserted` or `Updated`).
    #[must_use]
    pub fn wrote(self) -> bool {
        matches!(self, Self::Inserted | Self::Updated)
    }
}

/// A BLAKE3 content hash gating incremental re-embedding (Data Model §9.2/§3). The
/// stored `chunk.content_hash` is a BLAKE3 digest; [`ContentHash::of`] reproduces
/// it so callers/tests can compute a gate value from chunk text.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentHash(String);

impl ContentHash {
    /// BLAKE3 hex digest of `content`.
    #[must_use]
    pub fn of(content: &str) -> Self {
        Self(blake3::hash(content.as_bytes()).to_hex().to_string())
    }

    /// Wrap an existing hex digest (e.g. read from `chunk.content_hash`).
    #[must_use]
    pub fn from_hex(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }

    /// The hex string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A stored embedding row with its provenance (Data Model §9.3).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredEmbedding {
    pub chunk_id: ChunkId,
    pub embed_model: String,
    pub dims: usize,
    pub content_hash: ContentHash,
    pub vector: Vec<f32>,
    pub created_at: Timestamp,
}

/// One brute-force KNN neighbour. `score` is cosine similarity in `[-1, 1]`
/// (higher is nearer). The `ai-workspace` agent ranks these, maps `chunk_id` to its
/// spine entity, and re-fuses with the FTS channel via `search::rrf_fuse`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Neighbor {
    pub chunk_id: ChunkId,
    pub score: f32,
    pub embed_model: String,
}

/// Persists per-chunk embeddings and answers cosine-KNN queries for one embedding
/// model. Stateless w.r.t. the DB: every method takes a [`Connection`], so it
/// composes with `storage`'s single-writer / read-pool discipline (HLD §5).
#[derive(Clone, Debug)]
pub struct VectorStore {
    embed_model: String,
    dims: usize,
}

impl VectorStore {
    /// A store stamping rows with `embed_model` / `dims` provenance.
    #[must_use]
    pub fn new(embed_model: impl Into<String>, dims: usize) -> Self {
        Self {
            embed_model: embed_model.into(),
            dims,
        }
    }

    /// A store whose provenance is taken from an [`Embedder`].
    #[must_use]
    pub fn for_embedder(embedder: &dyn Embedder) -> Self {
        Self::new(embedder.model_id(), embedder.dimension())
    }

    /// The provenance string rows are stamped with and KNN filters on.
    #[must_use]
    pub fn embed_model(&self) -> &str {
        &self.embed_model
    }

    /// The expected vector dimensionality.
    #[must_use]
    pub fn dimension(&self) -> usize {
        self.dims
    }

    /// Create the brute-force table if absent (idempotent).
    ///
    /// # Errors
    /// [`EmbeddingError::Storage`] on any SQLite failure.
    pub fn ensure_schema(&self, conn: &Connection) -> EmbeddingResult<()> {
        conn.execute_batch(EMBEDDING_CHUNK_DDL)?;
        Ok(())
    }

    /// Persist (or skip) a chunk's embedding, gated on `content_hash`.
    ///
    /// If a row for `chunk_id` already exists with the same `content_hash` **and**
    /// the same `embed_model`, the vector is left untouched and
    /// [`UpsertOutcome::Unchanged`] is returned — the incremental gate of Data Model
    /// §9.2. Otherwise the row is inserted/replaced.
    ///
    /// # Errors
    /// - [`EmbeddingError::DimensionMismatch`] if `vector.len() != dimension()`.
    /// - [`EmbeddingError::Storage`] on any SQLite failure.
    pub fn upsert(
        &self,
        conn: &Connection,
        chunk_id: ChunkId,
        content_hash: &ContentHash,
        vector: &[f32],
    ) -> EmbeddingResult<UpsertOutcome> {
        if vector.len() != self.dims {
            return Err(EmbeddingError::DimensionMismatch {
                expected: self.dims,
                actual: vector.len(),
            });
        }

        let existing: Option<(String, String)> = conn
            .query_row(
                "SELECT content_hash, embed_model FROM embedding_chunk WHERE chunk_id = ?1",
                params![&chunk_id.as_bytes()[..]],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;

        if let Some((stored_hash, stored_model)) = &existing {
            if stored_hash == content_hash.as_str() && stored_model == &self.embed_model {
                return Ok(UpsertOutcome::Unchanged);
            }
        }

        conn.execute(
            "INSERT OR REPLACE INTO embedding_chunk \
             (chunk_id, embed_model, dims, content_hash, vector, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                &chunk_id.as_bytes()[..],
                &self.embed_model,
                self.dims as i64,
                content_hash.as_str(),
                encode_vector(vector),
                Timestamp::now().as_millis(),
            ],
        )?;

        Ok(if existing.is_some() {
            UpsertOutcome::Updated
        } else {
            UpsertOutcome::Inserted
        })
    }

    /// Embed `text` with `embedder` and [`upsert`](Self::upsert) it, gated on the
    /// BLAKE3 hash of `text`. A convenience wrapper for the indexer path; the gate
    /// means an unchanged chunk never invokes the (costly) embedder.
    ///
    /// Note: this always embeds then checks length; to skip embedding entirely on
    /// an unchanged chunk, compute [`ContentHash::of`] first and call
    /// [`is_current`](Self::is_current). See [`upsert_text_gated`](Self::upsert_text_gated).
    ///
    /// # Errors
    /// Propagates embedder and storage errors.
    pub fn upsert_text(
        &self,
        conn: &Connection,
        chunk_id: ChunkId,
        text: &str,
        embedder: &dyn Embedder,
    ) -> EmbeddingResult<UpsertOutcome> {
        let hash = ContentHash::of(text);
        let vector = embedder.embed_one(text)?;
        self.upsert(conn, chunk_id, &hash, &vector)
    }

    /// Content-hash-gated embed: if the chunk's stored hash already matches
    /// `BLAKE3(text)` for this `embed_model`, returns [`UpsertOutcome::Unchanged`]
    /// **without invoking `embedder`** — the intended incremental path that avoids
    /// re-running the model on unchanged content (Data Model §9.2, Architecture §10
    /// "content-hash gate; job idempotent").
    ///
    /// # Errors
    /// Propagates embedder and storage errors.
    pub fn upsert_text_gated(
        &self,
        conn: &Connection,
        chunk_id: ChunkId,
        text: &str,
        embedder: &dyn Embedder,
    ) -> EmbeddingResult<UpsertOutcome> {
        let hash = ContentHash::of(text);
        if self.is_current(conn, chunk_id, &hash)? {
            return Ok(UpsertOutcome::Unchanged);
        }
        let vector = embedder.embed_one(text)?;
        self.upsert(conn, chunk_id, &hash, &vector)
    }

    /// Whether `chunk_id` already has an embedding for this `embed_model` at
    /// `content_hash` (i.e. re-embedding would be redundant).
    ///
    /// # Errors
    /// [`EmbeddingError::Storage`] on any SQLite failure.
    pub fn is_current(
        &self,
        conn: &Connection,
        chunk_id: ChunkId,
        content_hash: &ContentHash,
    ) -> EmbeddingResult<bool> {
        let row: Option<(String, String)> = conn
            .query_row(
                "SELECT content_hash, embed_model FROM embedding_chunk WHERE chunk_id = ?1",
                params![&chunk_id.as_bytes()[..]],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((h, m)) => h == content_hash.as_str() && m == self.embed_model,
            None => false,
        })
    }

    /// Fetch a stored embedding (any `embed_model`), or `None`.
    ///
    /// # Errors
    /// [`EmbeddingError::Storage`]/[`EmbeddingError::CorruptVectorBlob`] on failure.
    pub fn get(
        &self,
        conn: &Connection,
        chunk_id: ChunkId,
    ) -> EmbeddingResult<Option<StoredEmbedding>> {
        let row = conn
            .query_row(
                "SELECT embed_model, dims, content_hash, vector, created_at \
                 FROM embedding_chunk WHERE chunk_id = ?1",
                params![&chunk_id.as_bytes()[..]],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, i64>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                        r.get::<_, i64>(4)?,
                    ))
                },
            )
            .optional()?;

        match row {
            None => Ok(None),
            Some((embed_model, dims, hash, blob, created_at)) => Ok(Some(StoredEmbedding {
                chunk_id,
                embed_model,
                dims: dims.max(0) as usize,
                content_hash: ContentHash::from_hex(hash),
                vector: decode_vector(&blob)?,
                created_at: Timestamp::from_millis(created_at),
            })),
        }
    }

    /// Delete a chunk's embedding. Returns whether a row was removed.
    ///
    /// # Errors
    /// [`EmbeddingError::Storage`] on any SQLite failure.
    pub fn delete(&self, conn: &Connection, chunk_id: ChunkId) -> EmbeddingResult<bool> {
        let n = conn.execute(
            "DELETE FROM embedding_chunk WHERE chunk_id = ?1",
            params![&chunk_id.as_bytes()[..]],
        )?;
        Ok(n > 0)
    }

    /// Count embeddings stamped with this store's `embed_model`.
    ///
    /// # Errors
    /// [`EmbeddingError::Storage`] on any SQLite failure.
    pub fn count(&self, conn: &Connection) -> EmbeddingResult<usize> {
        let n: i64 = conn.query_row(
            "SELECT COUNT(*) FROM embedding_chunk WHERE embed_model = ?1",
            params![&self.embed_model],
            |r| r.get(0),
        )?;
        Ok(n.max(0) as usize)
    }

    /// Brute-force cosine KNN: the `k` nearest chunks to `query` among rows whose
    /// `embed_model` matches this store (Data Model §9.3: KNN is restricted to a
    /// single `embed_model` per batch, so mixed-provenance rows during a re-embed
    /// never contaminate a ranking).
    ///
    /// Results are sorted by descending cosine similarity, ties broken by ascending
    /// `chunk_id` for deterministic, reproducible output (the op-log correctness
    /// oracle expects stable ordering — mirrors `search::rrf_fuse`).
    ///
    /// # Errors
    /// [`EmbeddingError::Storage`]/[`EmbeddingError::CorruptVectorBlob`] on failure.
    pub fn knn(
        &self,
        conn: &Connection,
        query: &[f32],
        k: usize,
    ) -> EmbeddingResult<Vec<Neighbor>> {
        // SEAM: with the `vec0` extension loaded this becomes
        //   SELECT chunk_id, distance FROM vec_chunk
        //   WHERE embedding MATCH ?1 AND k = ?2 ORDER BY distance;
        // The Rust scan below is the extension-free fallback.
        let mut stmt =
            conn.prepare("SELECT chunk_id, vector FROM embedding_chunk WHERE embed_model = ?1")?;
        let rows = stmt.query_map(params![&self.embed_model], |r| {
            Ok((r.get::<_, Vec<u8>>(0)?, r.get::<_, Vec<u8>>(1)?))
        })?;

        let mut scored: Vec<Neighbor> = Vec::new();
        for row in rows {
            let (id_blob, vec_blob) = row?;
            let chunk_id = decode_chunk_id(&id_blob)?;
            let vector = decode_vector(&vec_blob)?;
            scored.push(Neighbor {
                chunk_id,
                score: math::cosine_similarity(query, &vector),
                embed_model: self.embed_model.clone(),
            });
        }

        // Descending score; ascending id tie-break (deterministic).
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.chunk_id.cmp(&b.chunk_id))
        });
        scored.truncate(k);
        Ok(scored)
    }
}

/// Encode `f32` components as a little-endian byte blob.
fn encode_vector(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

/// Decode a little-endian `f32` blob written by [`encode_vector`].
fn decode_vector(blob: &[u8]) -> EmbeddingResult<Vec<f32>> {
    if !blob.len().is_multiple_of(4) {
        return Err(EmbeddingError::CorruptVectorBlob(blob.len()));
    }
    Ok(blob
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect())
}

/// Decode a 16-byte chunk-id BLOB into a [`ChunkId`].
fn decode_chunk_id(blob: &[u8]) -> EmbeddingResult<ChunkId> {
    let bytes: [u8; 16] = blob.try_into().map_err(|_| {
        EmbeddingError::Storage(format!("chunk_id blob is {} bytes, not 16", blob.len()))
    })?;
    Ok(Id::from_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedder::MockEmbedder;

    fn mem() -> Connection {
        Connection::open_in_memory().expect("in-memory sqlite")
    }

    fn chunk(i: u8) -> ChunkId {
        let mut b = [0u8; 16];
        b[15] = i;
        Id::from_bytes(b)
    }

    #[test]
    fn ensure_schema_is_idempotent() {
        let c = mem();
        let s = VectorStore::new("m", 4);
        s.ensure_schema(&c).unwrap();
        s.ensure_schema(&c).unwrap(); // second call must not error
    }

    #[test]
    fn upsert_roundtrips_and_reports_outcomes() {
        let c = mem();
        let s = VectorStore::new("m", 4);
        s.ensure_schema(&c).unwrap();

        let id = chunk(1);
        let h1 = ContentHash::of("hello");
        assert_eq!(
            s.upsert(&c, id, &h1, &[0.1, 0.2, 0.3, 0.4]).unwrap(),
            UpsertOutcome::Inserted
        );

        let got = s.get(&c, id).unwrap().expect("row");
        assert_eq!(got.embed_model, "m");
        assert_eq!(got.dims, 4);
        assert_eq!(got.content_hash, h1);
        assert_eq!(got.vector, vec![0.1, 0.2, 0.3, 0.4]);

        // Same hash → gate fires, no rewrite.
        assert_eq!(
            s.upsert(&c, id, &h1, &[9.0, 9.0, 9.0, 9.0]).unwrap(),
            UpsertOutcome::Unchanged
        );
        assert_eq!(
            s.get(&c, id).unwrap().unwrap().vector,
            vec![0.1, 0.2, 0.3, 0.4]
        );

        // Changed hash → re-embed.
        let h2 = ContentHash::of("hello world");
        assert_eq!(
            s.upsert(&c, id, &h2, &[1.0, 0.0, 0.0, 0.0]).unwrap(),
            UpsertOutcome::Updated
        );
        assert_eq!(
            s.get(&c, id).unwrap().unwrap().vector,
            vec![1.0, 0.0, 0.0, 0.0]
        );
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let c = mem();
        let s = VectorStore::new("m", 4);
        s.ensure_schema(&c).unwrap();
        let err = s
            .upsert(&c, chunk(1), &ContentHash::of("x"), &[1.0, 2.0])
            .unwrap_err();
        assert!(matches!(
            err,
            EmbeddingError::DimensionMismatch {
                expected: 4,
                actual: 2
            }
        ));
    }

    #[test]
    fn content_hash_gate_skips_embedder_call() {
        let c = mem();
        let e = MockEmbedder::with_dimension(32);
        let s = VectorStore::for_embedder(&e);
        s.ensure_schema(&c).unwrap();
        let id = chunk(7);

        assert_eq!(
            s.upsert_text_gated(&c, id, "unchanged body", &e).unwrap(),
            UpsertOutcome::Inserted
        );
        // Re-embedding identical text is gated out.
        assert_eq!(
            s.upsert_text_gated(&c, id, "unchanged body", &e).unwrap(),
            UpsertOutcome::Unchanged
        );
        // Different text passes the gate.
        assert_eq!(
            s.upsert_text_gated(&c, id, "edited body", &e).unwrap(),
            UpsertOutcome::Updated
        );
    }

    #[test]
    fn knn_returns_nearest_first() {
        let c = mem();
        let e = MockEmbedder::with_dimension(64);
        let s = VectorStore::for_embedder(&e);
        s.ensure_schema(&c).unwrap();

        // Fixture: three distinct chunks with deterministic embeddings.
        let texts = [
            (chunk(1), "quarterly revenue and budget planning"),
            (chunk(2), "weekend hiking trip in the mountains"),
            (chunk(3), "team standup notes and blockers"),
        ];
        for (id, t) in texts {
            s.upsert(&c, id, &ContentHash::of(t), &e.embed_text(t))
                .unwrap();
        }

        // Query identical to chunk 2's text: it must be the top neighbour with
        // near-perfect similarity, ahead of the unrelated chunks.
        let q = e.embed_text("weekend hiking trip in the mountains");
        let hits = s.knn(&c, &q, 3).unwrap();
        assert_eq!(hits.len(), 3);
        assert_eq!(hits[0].chunk_id, chunk(2));
        assert!((hits[0].score - 1.0).abs() < 1e-5);
        assert!(hits[0].score > hits[1].score);
        assert!(hits[1].score >= hits[2].score);

        // top-k truncation.
        assert_eq!(s.knn(&c, &q, 1).unwrap().len(), 1);
    }

    #[test]
    fn knn_filters_by_embed_model() {
        let c = mem();
        let ea = MockEmbedder::with_dimension(16).with_model_id("model-a");
        let eb = MockEmbedder::with_dimension(16).with_model_id("model-b");
        let sa = VectorStore::for_embedder(&ea);
        let sb = VectorStore::for_embedder(&eb);
        sa.ensure_schema(&c).unwrap();

        sa.upsert(&c, chunk(1), &ContentHash::of("a"), &ea.embed_text("a"))
            .unwrap();
        sb.upsert(&c, chunk(2), &ContentHash::of("b"), &eb.embed_text("b"))
            .unwrap();

        // Each store only sees its own provenance.
        let hits_a = sa.knn(&c, &ea.embed_text("a"), 10).unwrap();
        assert_eq!(hits_a.len(), 1);
        assert_eq!(hits_a[0].chunk_id, chunk(1));
        assert_eq!(hits_a[0].embed_model, "model-a");

        assert_eq!(sa.count(&c).unwrap(), 1);
        assert_eq!(sb.count(&c).unwrap(), 1);
    }

    #[test]
    fn delete_removes_row() {
        let c = mem();
        let s = VectorStore::new("m", 4);
        s.ensure_schema(&c).unwrap();
        let id = chunk(5);
        s.upsert(&c, id, &ContentHash::of("x"), &[1.0, 0.0, 0.0, 0.0])
            .unwrap();
        assert!(s.delete(&c, id).unwrap());
        assert!(!s.delete(&c, id).unwrap());
        assert!(s.get(&c, id).unwrap().is_none());
    }

    #[test]
    fn corrupt_blob_is_reported_not_panicked() {
        assert!(matches!(
            decode_vector(&[0u8; 5]),
            Err(EmbeddingError::CorruptVectorBlob(5))
        ));
    }

    #[test]
    fn ties_break_on_chunk_id_deterministically() {
        let c = mem();
        let s = VectorStore::new("m", 4);
        s.ensure_schema(&c).unwrap();
        // Two chunks with the SAME vector → identical score → id tie-break.
        let v = [0.5, 0.5, 0.5, 0.5];
        s.upsert(&c, chunk(9), &ContentHash::of("nine"), &v)
            .unwrap();
        s.upsert(&c, chunk(2), &ContentHash::of("two"), &v).unwrap();
        let hits = s.knn(&c, &v, 2).unwrap();
        assert_eq!(hits[0].chunk_id, chunk(2)); // lower id first
        assert_eq!(hits[1].chunk_id, chunk(9));
    }
}

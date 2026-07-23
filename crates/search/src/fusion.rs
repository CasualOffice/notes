//! Reciprocal Rank Fusion. Implements **Data Model §10.1 / HLD §8.5**:
//! `score(d) = Σ_i 1/(k + rank_i(d))`, `k = 60`, **no score normalization** — the
//! Garcia recipe. Fusion is over *ordinal rank*, never raw BM25/cosine scores,
//! which is why [`SearchHit::bm25`](crate::query::SearchHit::bm25) is kept
//! unnormalized.
//!
//! ## What's live vs. the Phase-3 seam
//! Phase 1 has one retrieval channel (FTS5 BM25) but several source *lists*
//! (note/task/transcript), so RRF already earns its keep fusing those into one
//! ranked palette result. The **vector KNN channel** (`embeddings` +
//! `sqlite-vec`) is Phase 3: it will stream in a second family of ranked lists
//! that re-fuse through this exact function (HLD N6, <300 ms re-fuse). No re-model
//! is needed — [`rrf_fuse`] already accepts N lists; the vector channel is just
//! more lists. The `SearchSource` tag on each hit records which channel it came
//! from so a re-fuse can dedupe by entity.

use app_domain::EntityRef;

/// The RRF rank-offset constant `k` (Data Model §10.1). Larger `k` flattens the
/// contribution of top ranks; 60 is the canonical value.
pub const RRF_K: u32 = 60;

/// RRF configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RrfConfig {
    pub k: u32,
}

impl Default for RrfConfig {
    fn default() -> Self {
        Self { k: RRF_K }
    }
}

/// One entity's fused score after RRF.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FusedHit {
    pub entity: EntityRef,
    /// Σ 1/(k + rank). Higher is better.
    pub score: f64,
}

/// Fuse several ranked lists of entity refs into one ranking by RRF.
///
/// Each input list is assumed already ordered best-first; an entity's rank is its
/// 1-based position within a list. An entity appearing in multiple lists sums its
/// reciprocal-rank contributions (this is what makes cross-source agreement win).
/// Ties break on the entity id for determinism (reproducible ordering — the op-log
/// correctness oracle expects stable output).
#[must_use]
pub fn rrf_fuse(lists: &[Vec<EntityRef>], cfg: RrfConfig) -> Vec<FusedHit> {
    let k = f64::from(cfg.k);
    let mut acc: Vec<(EntityRef, f64)> = Vec::new();

    for list in lists {
        for (idx, entity) in list.iter().enumerate() {
            let rank = idx as u32 + 1;
            let contribution = 1.0 / (k + f64::from(rank));
            if let Some(slot) = acc.iter_mut().find(|(e, _)| e == entity) {
                slot.1 += contribution;
            } else {
                acc.push((*entity, contribution));
            }
        }
    }

    // Sort by score desc, then by id asc for a deterministic tie-break.
    acc.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.id.cmp(&b.0.id))
    });

    acc.into_iter()
        .map(|(entity, score)| FusedHit { entity, score })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::{EntityKind, Id};

    fn refs(n: usize) -> Vec<EntityRef> {
        // Deterministic, monotonically-increasing ids so id tie-breaks are testable.
        (0..n)
            .map(|i| {
                let mut b = [0u8; 16];
                b[15] = i as u8;
                EntityRef::new(EntityKind::Note, Id::from_bytes(b))
            })
            .collect()
    }

    #[test]
    fn single_list_preserves_order() {
        let r = refs(3);
        let fused = rrf_fuse(std::slice::from_ref(&r), RrfConfig::default());
        assert_eq!(fused.iter().map(|f| f.entity).collect::<Vec<_>>(), r);
        // rank-1 score = 1/(60+1)
        assert!((fused[0].score - 1.0 / 61.0).abs() < 1e-12);
    }

    #[test]
    fn agreement_across_lists_beats_a_single_top_hit() {
        let r = refs(3);
        // List A: [0,1,2]; List B: [1,0,2]. Entity 1 is rank1 in B, rank2 in A;
        // entity 0 is rank1 in A, rank2 in B — they tie on summed score, so the
        // id tie-break puts entity 0 first. Entity 2 (rank3 twice) trails.
        let a = vec![r[0], r[1], r[2]];
        let b = vec![r[1], r[0], r[2]];
        let fused = rrf_fuse(&[a, b], RrfConfig::default());
        assert_eq!(fused[0].entity, r[0]);
        assert_eq!(fused[1].entity, r[1]);
        assert_eq!(fused[2].entity, r[2]);
        // The two leaders each scored 1/61 + 1/62; the trailer 2/63.
        let leader = 1.0 / 61.0 + 1.0 / 62.0;
        assert!((fused[0].score - leader).abs() < 1e-12);
    }

    #[test]
    fn empty_input_is_empty() {
        assert!(rrf_fuse(&[], RrfConfig::default()).is_empty());
    }
}

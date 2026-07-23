//! The search query + result models. Implements the `search.query {q, filters?,
//! mode}` command shape (HLD §6, Feature Specs §7) and the ranked hit list
//! (`SearchHits` — entity ref + snippet + BM25 rank) it returns.
//!
//! A [`SearchQuery`] is the compiled, typed form of a palette input: free text +
//! [`Filters`] + the sources to hit. [`SearchQuery::compile`] lowers it to one
//! [`CompiledSql`] per active FTS source; `storage` runs them and maps rows back
//! via [`SearchHit::from_row_parts`]. Vector KNN + RRF re-fusion is Phase 3 (see
//! [`crate::fusion`]); the FTS lists produced here are already RRF-ready.

use app_domain::{Day, EntityKind, EntityRef, Id, QueryId, SearchSource};
use serde::{Deserialize, Serialize};

use crate::filter::{parse_query, Filters};
use crate::fts::{build_fts_query, build_match_expr, FtsSource, MatchMode};
use crate::sql::CompiledSql;

/// Default number of hits requested per source before fusion.
pub const DEFAULT_LIMIT: u32 = 50;

/// The default FTS sources a typeless query fans out over. Reminders have no FTS5
/// table in Phase 1 (Data Model §10), so the reachable pillars are notes, tasks,
/// and meetings (via transcript). This is the documented Phase-1 shortfall vs.
/// AC-7.4 ("all four pillars from one palette").
pub const DEFAULT_SOURCES: [FtsSource; 3] =
    [FtsSource::Note, FtsSource::Task, FtsSource::Transcript];

/// Which retrieval mode the `search.query` command is running in (HLD §6). `Do`
/// is *not* here — it is a command runner, not retrieval (see [`crate::palette`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SearchMode {
    /// Quick-switcher: prefix + BM25, recency-boosted (Feature Specs §7.1).
    Go,
    /// Hybrid RAG retrieval feeding a cited answer (Feature Specs §6/§7.1).
    Ask,
}

impl SearchMode {
    /// Go matches prefixes (results while typing); Ask matches whole tokens.
    #[must_use]
    pub const fn match_mode(self) -> MatchMode {
        match self {
            SearchMode::Go => MatchMode::Prefix,
            SearchMode::Ask => MatchMode::Exact,
        }
    }
}

/// A fully-typed search request ready to lower to SQL.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchQuery {
    /// Free-text (filters already stripped out).
    pub text: String,
    pub filters: Filters,
    pub mode: SearchMode,
    /// Sources to hit, already resolved from `filters.types` (or defaults).
    pub sources: Vec<FtsSource>,
    /// Per-source row cap before fusion.
    pub limit: u32,
    /// The caller's local wall-date, resolving relative `date:` filters.
    pub today: Day,
}

impl SearchQuery {
    /// Parse a raw palette query string (`type:task is:open foo`) into a typed
    /// request. `today` resolves relative date filters to concrete `YYYY-MM-DD`.
    #[must_use]
    pub fn parse(input: &str, mode: SearchMode, today: Day) -> Self {
        let parsed = parse_query(input);
        let sources = parsed.filters.active_sources(&DEFAULT_SOURCES);
        Self {
            text: parsed.text,
            filters: parsed.filters,
            mode,
            sources,
            limit: DEFAULT_LIMIT,
            today,
        }
    }

    /// True when there is no searchable text — the palette should show recents
    /// rather than run a MATCH (Feature Specs §7.2 empty-query rule).
    #[must_use]
    pub fn is_recents(&self) -> bool {
        build_match_expr(&self.text, self.mode.match_mode()).is_none()
    }

    /// Lower to one [`CompiledSql`] per active source. Empty when the query is a
    /// recents request (no MATCH text). Each entry is paired with its source so
    /// the caller can tag hits and fuse per-source rankings.
    #[must_use]
    pub fn compile(&self) -> Vec<(FtsSource, CompiledSql)> {
        let Some(match_expr) = build_match_expr(&self.text, self.mode.match_mode()) else {
            return Vec::new();
        };
        self.sources
            .iter()
            .map(|&source| {
                let filter = self.filters.compile_for(source, self.today);
                (
                    source,
                    build_fts_query(source, &match_expr, &filter, self.limit),
                )
            })
            .collect()
    }
}

/// One ranked search hit (`SearchHits` element, HLD §6).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    /// The resolved spine entity (`{kind, id}`) to navigate to.
    pub entity: EntityRef,
    /// Denormalized display title from `entity.title` (nullable in the schema).
    pub title: Option<String>,
    /// FTS5 `snippet()` excerpt with `[`…`]` match markers.
    pub snippet: String,
    /// Raw `bm25()` rank — **lower is better** (more negative). Kept unnormalized
    /// so RRF fuses on ordinal rank, not score (Data Model §10.1).
    pub bm25: f64,
    /// Which retrieval channel produced this hit (`Fts` now; `Vector` in Phase 3).
    pub source: SearchSource,
    /// Which FTS table it came from (note/task/transcript/chunk).
    pub fts_source: FtsSource,
}

impl SearchHit {
    /// Assemble a hit from a raw FTS SELECT row (columns [`crate::fts::HIT_COLUMNS`]).
    /// `id_hex` is the lower-hex of the 16-byte entity id; `kind_str` is
    /// `entity.kind`. Returns `None` if either fails to parse (a corrupt map row).
    #[must_use]
    pub fn from_row_parts(
        id_hex: &str,
        kind_str: &str,
        title: Option<String>,
        bm25: f64,
        snippet: String,
        fts_source: FtsSource,
    ) -> Option<Self> {
        let entity = entity_ref_from_hex(kind_str, id_hex)?;
        Some(Self {
            entity,
            title,
            snippet,
            bm25,
            source: SearchSource::Fts,
            fts_source,
        })
    }
}

/// The streamed result envelope for a `search.query`. `complete=false` while the
/// vector channel is still streaming in (Phase 3); the FTS-only first paint sets
/// it per whether any more channels are pending.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SearchResults {
    /// Correlates with the `AppEvent::SearchPartial { query_id }` stream (HLD §7).
    pub query_id: QueryId,
    pub hits: Vec<SearchHit>,
    /// `true` once every retrieval channel has reported (FTS + vector).
    pub complete: bool,
}

impl SearchResults {
    #[must_use]
    pub fn new(query_id: QueryId, hits: Vec<SearchHit>, complete: bool) -> Self {
        Self {
            query_id,
            hits,
            complete,
        }
    }
}

/// Rebuild an [`EntityRef`] from an FTS row's `lower(hex(id))` + `kind` strings.
#[must_use]
pub fn entity_ref_from_hex(kind_str: &str, id_hex: &str) -> Option<EntityRef> {
    let kind = EntityKind::from_db_str(kind_str)?;
    let bytes = hex16(id_hex)?;
    Some(EntityRef::new(kind, Id::from_bytes(bytes)))
}

/// Parse exactly 32 lower/upper hex chars into a 16-byte array.
fn hex16(s: &str) -> Option<[u8; 16]> {
    if s.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    let bytes = s.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = (bytes[i * 2] as char).to_digit(16)?;
        let lo = (bytes[i * 2 + 1] as char).to_digit(16)?;
        *slot = ((hi << 4) | lo) as u8;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn today() -> Day {
        Day::from_str("2026-07-23").unwrap()
    }

    #[test]
    fn parse_resolves_sources_from_type_filter() {
        let q = SearchQuery::parse("type:task foo", SearchMode::Go, today());
        assert_eq!(q.sources, vec![FtsSource::Task]);
        assert_eq!(q.text, "foo");
    }

    #[test]
    fn typeless_query_uses_default_sources() {
        let q = SearchQuery::parse("foo", SearchMode::Go, today());
        assert_eq!(q.sources, DEFAULT_SOURCES.to_vec());
    }

    #[test]
    fn empty_text_is_recents_and_compiles_to_nothing() {
        let q = SearchQuery::parse("type:task", SearchMode::Go, today());
        assert!(q.is_recents());
        assert!(q.compile().is_empty());
    }

    #[test]
    fn compile_emits_one_query_per_source() {
        let q = SearchQuery::parse("hello", SearchMode::Go, today());
        let compiled = q.compile();
        assert_eq!(compiled.len(), DEFAULT_SOURCES.len());
        assert_eq!(compiled[0].0, FtsSource::Note);
        assert!(compiled[0].1.sql.contains("bm25(fts_note)"));
    }

    #[test]
    fn go_uses_prefix_ask_uses_exact() {
        assert_eq!(SearchMode::Go.match_mode(), MatchMode::Prefix);
        assert_eq!(SearchMode::Ask.match_mode(), MatchMode::Exact);
    }

    #[test]
    fn entity_ref_round_trips_from_hex() {
        let id = Id::new();
        let hex: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
        let r = entity_ref_from_hex("note", &hex).unwrap();
        assert_eq!(r.kind, EntityKind::Note);
        assert_eq!(r.id, id);
    }

    #[test]
    fn entity_ref_rejects_bad_kind_or_hex() {
        assert!(entity_ref_from_hex("block", &"00".repeat(16)).is_none());
        assert!(entity_ref_from_hex("note", "xyz").is_none());
    }

    #[test]
    fn hit_from_row_parts_builds_fts_source() {
        let hex: String = Id::new()
            .as_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        let hit = SearchHit::from_row_parts(
            &hex,
            "task",
            Some("Ship it".into()),
            -3.2,
            "…[ship] it…".into(),
            FtsSource::Task,
        )
        .unwrap();
        assert_eq!(hit.source, SearchSource::Fts);
        assert_eq!(hit.fts_source, FtsSource::Task);
        assert_eq!(hit.entity.kind, EntityKind::Task);
    }
}

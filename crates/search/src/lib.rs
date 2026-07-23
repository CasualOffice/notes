//! # search
//!
//! Hybrid retrieval. Implements **Data Model §10 / §10.1** and **HLD §8.5**:
//! FTS5 BM25 (synchronous, <10 ms first paint) unioned with sqlite-vec KNN, fused
//! by **Reciprocal Rank Fusion** (`k=60`, no score normalization). First-class
//! filters (`type:`/`tag:`/`date:`/`person:`/`is:`) compile to SQL predicates
//! applied *before* fusion. FTS returns immediately; vectors stream in and re-fuse.
//!
//! ## Phase-1 slice (this crate as it stands)
//! The FTS half is implemented as **pure query construction**: this crate builds
//! parameterized SQL + bind values and hands them to `storage` for execution — it
//! never opens a DB connection (CLAUDE.md: the WebView never sees SQL; all DB
//! access is Rust-side via `storage`). What ships here:
//! - [`fts`] — the four FTS5 sources (Data Model §10), safe MATCH-expression
//!   construction, and the BM25 SELECT builder.
//! - [`filter`] — the `type:/tag:/date:/person:/is:` grammar and its per-source
//!   compilation to `WHERE` predicates (Feature Specs §7.2).
//! - [`query`] — the [`SearchQuery`] request model and the ranked
//!   [`SearchHit`]/[`SearchResults`] returned by `search.query` (HLD §6).
//! - [`snippet`] — Rust-side excerpt construction (contentless FTS5 can't
//!   `snippet()`), building `[`-marked windows from resolved source text.
//! - [`palette`] — the `Cmd-K` palette: [`classify`](palette::classify) by sigil,
//!   the **Go** quick-switcher ([`GoQuery`](palette::GoQuery)) and **Do** command
//!   runner ([`match_commands`](palette::match_commands)) models (Feature Specs §7.1).
//! - [`fusion`] — RRF ([`rrf_fuse`](fusion::rrf_fuse)), already fusing the
//!   per-source FTS lists; the **vector KNN channel is the documented Phase-3 seam**
//!   (`embeddings` + sqlite-vec re-fuse through the same function, HLD N6).
//!
//! ## Assumptions the integration phase must reconcile
//! - **Per-source map tables.** Data Model §10 spells out `fts_note_map(rowid,
//!   entity_id)` only; this crate assumes sibling `fts_task_map` /
//!   `fts_transcript_map` / `fts_chunk_map` with the same shape, each `entity_id`
//!   pointing at the *owning spine entity*.
//! - **Reminders have no FTS5 table** in Phase 1 (§10 defines note/task/transcript/
//!   chunk). `type:reminder` therefore resolves to zero sources; AC-7.4's "all four
//!   pillars" is met for note/task/meeting only until a `fts_reminder` (or a
//!   title-scan fallback) is added.
//! - **Chunk hits are polymorphic** (a chunk belongs to a note/session/task), so
//!   [`FtsSource::Chunk`](fts::FtsSource::Chunk) reads `entity.kind` at run time
//!   rather than a fixed kind.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod error;
pub mod filter;
pub mod fts;
pub mod fusion;
pub mod palette;
pub mod query;
pub mod snippet;
pub mod sql;

// Flat re-exports of the most-used surface.
pub use error::{SearchError, SearchResult};
pub use filter::{parse_query, DateSpec, Filters, IsFilter, ParsedInput, TypeFilter};
pub use fts::{build_fts_query, build_match_expr, FtsSource, MatchMode, HIT_COLUMNS};
pub use fusion::{rrf_fuse, FusedHit, RrfConfig, RRF_K};
pub use palette::{
    builtin_commands, classify, match_commands, DoCommandSpec, GoQuery, PaletteInput, PaletteMode,
    ScopeKind,
};
pub use query::{
    entity_ref_from_hex, SearchHit, SearchMode, SearchQuery, SearchResults, DEFAULT_LIMIT,
    DEFAULT_SOURCES,
};
pub use snippet::make_snippet;
pub use sql::{CompiledSql, SqlParam, WhereClause};

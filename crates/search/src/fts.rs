//! FTS5 source model, MATCH-expression construction, and BM25 query building.
//! Implements **Data Model §10** (the four external-content FTS5 tables and their
//! `rowid ↔ entity_id` side maps) and the synchronous BM25 first-paint path of
//! **HLD §8.5 / N5**.
//!
//! ## The four sources (Data Model §10)
//! `fts_note(title, body)`, `fts_task(title, notes_md)`, `fts_transcript(text)`,
//! `fts_chunk(breadcrumb, text)` — all contentless (`content=''`), rows fed
//! explicitly on save inside the projection transaction. Each has a side table
//! `<name>_map(rowid INTEGER PRIMARY KEY, entity_id BLOB)` resolving an FTS rowid
//! to its **owning spine entity**: note→note, task→task, transcript→session,
//! chunk→its source entity. (Only `fts_note_map` is spelled out in §10; the
//! sibling maps follow the same convention — see the crate-level assumptions.)

use app_domain::EntityKind;
use serde::{Deserialize, Serialize};

use crate::sql::{CompiledSql, SqlParam, WhereClause};

/// One of the four per-source FTS5 tables (Data Model §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FtsSource {
    Note,
    Task,
    Transcript,
    Chunk,
}

impl FtsSource {
    /// Every source, in the canonical order.
    #[must_use]
    pub const fn all() -> [FtsSource; 4] {
        [
            FtsSource::Note,
            FtsSource::Task,
            FtsSource::Transcript,
            FtsSource::Chunk,
        ]
    }

    /// The virtual-table name.
    #[must_use]
    pub const fn table(self) -> &'static str {
        match self {
            FtsSource::Note => "fts_note",
            FtsSource::Task => "fts_task",
            FtsSource::Transcript => "fts_transcript",
            FtsSource::Chunk => "fts_chunk",
        }
    }

    /// The `rowid ↔ entity_id` side-map table (Data Model §10).
    #[must_use]
    pub const fn map_table(self) -> &'static str {
        match self {
            FtsSource::Note => "fts_note_map",
            FtsSource::Task => "fts_task_map",
            FtsSource::Transcript => "fts_transcript_map",
            FtsSource::Chunk => "fts_chunk_map",
        }
    }

    /// Indexed columns, in declaration order (the BM25 column indices).
    #[must_use]
    pub const fn columns(self) -> &'static [&'static str] {
        match self {
            FtsSource::Note => &["title", "body"],
            FtsSource::Task => &["title", "notes_md"],
            FtsSource::Transcript => &["text"],
            FtsSource::Chunk => &["breadcrumb", "text"],
        }
    }

    /// The 0-based column index the `snippet()` helper draws its excerpt from
    /// (the "body" column, not the title).
    #[must_use]
    pub const fn snippet_col(self) -> i32 {
        match self {
            FtsSource::Note => 1,       // body
            FtsSource::Task => 1,       // notes_md
            FtsSource::Transcript => 0, // text
            FtsSource::Chunk => 1,      // text
        }
    }

    /// The spine [`EntityKind`] an FTS hit from this source resolves to. `Chunk`
    /// resolves to whatever entity owns the chunk (note/session/task), so it is
    /// polymorphic and returns `None` here — the join reads `entity.kind` at run
    /// time instead.
    #[must_use]
    pub const fn entity_kind(self) -> Option<EntityKind> {
        match self {
            FtsSource::Note => Some(EntityKind::Note),
            FtsSource::Task => Some(EntityKind::Task),
            FtsSource::Transcript => Some(EntityKind::Session),
            FtsSource::Chunk => None,
        }
    }

    /// The `(detail_table, alias)` joined so filter predicates can reference
    /// kind-specific columns (e.g. `t.status`, `n.daily_date`). `None` for
    /// sources with no kind-specific predicate surface in this phase.
    #[must_use]
    pub const fn detail_join(self) -> Option<(&'static str, &'static str)> {
        match self {
            FtsSource::Note => Some(("note", "n")),
            FtsSource::Task => Some(("task", "t")),
            FtsSource::Transcript => Some(("session", "s")),
            FtsSource::Chunk => None,
        }
    }
}

/// How free-text tokens become an FTS5 MATCH expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Every token matched whole. Used by the Ask/RAG path.
    Exact,
    /// The final token becomes a prefix (`tok*`). Used by the Go quick-switcher so
    /// results appear while the user is still typing (AC-7.1).
    Prefix,
}

/// Build a safe FTS5 MATCH expression from raw user text.
///
/// Each whitespace-delimited token is double-quoted (with embedded `"` doubled),
/// which neutralizes FTS5 operators/columns so arbitrary user input can never
/// form a malformed or injected MATCH. Tokens are space-joined = implicit AND.
/// In [`MatchMode::Prefix`] the **last** token gets a `*` prefix wildcard.
///
/// Returns `None` when the input has no usable tokens (caller shows recents).
#[must_use]
pub fn build_match_expr(input: &str, mode: MatchMode) -> Option<String> {
    let tokens: Vec<&str> = input.split_whitespace().filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return None;
    }
    let last = tokens.len() - 1;
    let mut out = String::new();
    for (i, tok) in tokens.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push('"');
        // Escape FTS5 string literal: double any embedded quote.
        for ch in tok.chars() {
            if ch == '"' {
                out.push('"');
            }
            out.push(ch);
        }
        out.push('"');
        if matches!(mode, MatchMode::Prefix) && i == last {
            out.push('*');
        }
    }
    Some(out)
}

/// The projected columns of every FTS SELECT this builder emits, in order.
/// Kept as a constant so `storage`'s row mapper and these queries can't drift.
///
/// **No `snippet` column:** the Data Model §10 FTS5 tables are *contentless*
/// (`content=''`), and SQLite's `snippet()`/`highlight()` cannot read column text
/// from a contentless table. The excerpt is therefore built Rust-side from the
/// resolved entity's source text — see [`crate::snippet::make_snippet`].
pub const HIT_COLUMNS: &[&str] = &["entity_id", "kind", "title", "rank"];

/// Build the synchronous BM25 query for one source.
///
/// Shape (Data Model §10, `bm25()` ranking; lower = better, so `ORDER BY rank`):
/// ```sql
/// SELECT lower(hex(e.id)) AS entity_id, e.kind AS kind, e.title AS title,
///        bm25(<tbl>) AS rank
/// FROM <tbl>
/// JOIN <tbl>_map m ON m.rowid = <tbl>.rowid
/// JOIN entity e   ON e.id = m.entity_id
/// [JOIN <detail> a ON a.entity_id = e.id]
/// WHERE <tbl> MATCH ?          -- param 1
///   AND e.deleted_at IS NULL
///   [AND (<filter>)]           -- filter params
/// ORDER BY rank
/// LIMIT ?                      -- last param
/// ```
/// Bind order is fixed: `[MATCH, <filter params…>, LIMIT]`. `snippet()` is
/// deliberately absent (contentless tables — see [`HIT_COLUMNS`]).
#[must_use]
pub fn build_fts_query(
    source: FtsSource,
    match_expr: &str,
    filter: &WhereClause,
    limit: u32,
) -> CompiledSql {
    let tbl = source.table();
    let map = source.map_table();

    let mut sql = String::new();
    sql.push_str("SELECT lower(hex(e.id)) AS entity_id, e.kind AS kind, e.title AS title, ");
    sql.push_str(&format!("bm25({tbl}) AS rank\n"));
    sql.push_str(&format!("FROM {tbl}\n"));
    sql.push_str(&format!("JOIN {map} m ON m.rowid = {tbl}.rowid\n"));
    sql.push_str("JOIN entity e ON e.id = m.entity_id\n");
    if let Some((detail, alias)) = source.detail_join() {
        sql.push_str(&format!(
            "JOIN {detail} {alias} ON {alias}.entity_id = e.id\n"
        ));
    }
    sql.push_str(&format!("WHERE {tbl} MATCH ?\n"));
    sql.push_str("  AND e.deleted_at IS NULL\n");

    let mut params: Vec<SqlParam> = Vec::with_capacity(2 + filter.params.len());
    params.push(SqlParam::text(match_expr));

    if !filter.is_empty() {
        sql.push_str("  AND (");
        sql.push_str(&filter.sql);
        sql.push_str(")\n");
        params.extend(filter.params.iter().cloned());
    }

    sql.push_str("ORDER BY rank\nLIMIT ?");
    params.push(SqlParam::Int(i64::from(limit)));

    CompiledSql::new(sql, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_expr_quotes_and_ands_tokens() {
        let e = build_match_expr("quarterly planning", MatchMode::Exact).unwrap();
        assert_eq!(e, "\"quarterly\" \"planning\"");
    }

    #[test]
    fn match_expr_prefix_wildcards_last_token_only() {
        let e = build_match_expr("quarterly plan", MatchMode::Prefix).unwrap();
        assert_eq!(e, "\"quarterly\" \"plan\"*");
    }

    #[test]
    fn match_expr_escapes_embedded_quote_and_neutralizes_operators() {
        // A raw OR / column-filter / quote can't escape the quoting.
        let e = build_match_expr("a\"b OR title:x", MatchMode::Exact).unwrap();
        assert_eq!(e, "\"a\"\"b\" \"OR\" \"title:x\"");
    }

    #[test]
    fn match_expr_empty_is_none() {
        assert!(build_match_expr("   ", MatchMode::Prefix).is_none());
    }

    #[test]
    fn fts_query_binds_match_then_limit() {
        let q = build_fts_query(FtsSource::Note, "\"foo\"", &WhereClause::empty(), 20);
        assert!(q.sql.contains("bm25(fts_note)"));
        assert!(q.sql.contains("JOIN fts_note_map"));
        // contentless tables can't snippet() — the excerpt is built Rust-side.
        assert!(!q.sql.contains("snippet("));
        assert!(q.sql.trim_end().ends_with("LIMIT ?"));
        assert_eq!(q.params.len(), 2);
        assert_eq!(q.params[0], SqlParam::text("\"foo\""));
        assert_eq!(q.params[1], SqlParam::Int(20));
    }

    #[test]
    fn fts_query_interleaves_filter_params_between_match_and_limit() {
        let filter = WhereClause {
            sql: "t.status = ?".to_string(),
            params: vec![SqlParam::text("open")],
        };
        let q = build_fts_query(FtsSource::Task, "\"x\"", &filter, 5);
        assert!(q.sql.contains("JOIN task t ON t.entity_id = e.id"));
        assert!(q.sql.contains("AND (t.status = ?)"));
        assert_eq!(
            q.params,
            vec![
                SqlParam::text("\"x\""),
                SqlParam::text("open"),
                SqlParam::Int(5)
            ]
        );
    }
}

//! Parameterized-SQL carriers. Implements the query-construction half of
//! **Data Model §10** without ever touching a DB connection.
//!
//! The `search` crate is pure: it *builds* SQL text plus ordered bind values and
//! hands them to `storage`, which owns every `rusqlite`/SQLCipher call (CLAUDE.md:
//! "the WebView never sees SQL … all DB access is Rust-side via storage"). Keeping
//! the connection out of this crate is what lets it stay dependency-light and
//! unit-testable in isolation.

use serde::{Deserialize, Serialize};

/// A single positional (`?`) bind value.
///
/// Only the two SQLite storage classes the search predicates actually use are
/// modeled: `TEXT` (titles, tags, `YYYY-MM-DD` days, FTS MATCH expressions) and
/// `INTEGER` (row limits). Entity ids are never bound as blobs here — the FTS
/// SELECTs resolve `entity.id` via `lower(hex(id))` on the SQL side.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "t", content = "v", rename_all = "snake_case")]
pub enum SqlParam {
    Text(String),
    Int(i64),
}

impl SqlParam {
    /// Convenience `TEXT` constructor.
    #[must_use]
    pub fn text(s: impl Into<String>) -> Self {
        Self::Text(s.into())
    }
}

/// A ready-to-execute statement: `sql` uses ordered `?` placeholders bound
/// left-to-right from `params`.
#[derive(Clone, Debug, PartialEq)]
pub struct CompiledSql {
    pub sql: String,
    pub params: Vec<SqlParam>,
}

impl CompiledSql {
    #[must_use]
    pub fn new(sql: impl Into<String>, params: Vec<SqlParam>) -> Self {
        Self {
            sql: sql.into(),
            params,
        }
    }
}

/// A composable `WHERE`-predicate fragment (with **no** leading `AND`) plus the
/// bind params it references, in placeholder order. Empty means "no constraint".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WhereClause {
    pub sql: String,
    pub params: Vec<SqlParam>,
}

impl WhereClause {
    /// An empty (always-true) clause.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.sql.is_empty()
    }

    /// AND another parenthesized predicate onto this one, preserving param order.
    /// A no-op when `frag` is empty.
    pub fn and_raw(&mut self, frag: &str, mut params: Vec<SqlParam>) {
        if frag.is_empty() {
            return;
        }
        if self.sql.is_empty() {
            self.sql.push_str(frag);
        } else {
            self.sql.push_str(" AND ");
            self.sql.push_str(frag);
        }
        self.params.append(&mut params);
    }
}

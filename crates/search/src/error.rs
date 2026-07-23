//! Search error taxonomy. Maps into the shared [`AppError`] (HLD §10) at the
//! command boundary — a malformed filter is a `Validation` failure.

use app_domain::AppError;

/// Errors raised while parsing/compiling a search query.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SearchError {
    /// A `key:value` filter used an unknown key or malformed value.
    #[error("invalid filter token: {0}")]
    InvalidFilter(String),

    /// A `date:` value was neither a keyword (`today`/`overdue`/`upcoming`) nor a
    /// parseable `YYYY-MM-DD` (or range/`<`/`>` form).
    #[error("invalid date filter: {0}")]
    InvalidDate(String),

    /// The query resolved to no searchable text (caller should show recents).
    #[error("empty query")]
    EmptyQuery,
}

impl From<SearchError> for AppError {
    fn from(e: SearchError) -> Self {
        AppError::Validation(e.to_string())
    }
}

/// Convenience alias for fallible search paths.
pub type SearchResult<T> = Result<T, SearchError>;

//! The `Cmd/Ctrl-K` command-palette model. Implements **Feature Specs §7.1**
//! (one palette, mode chosen by leading sigil) and the `palette.run {mode, input}`
//! command (HLD §6).
//!
//! - **Go** (no sigil): quick-switcher — [`SearchQuery`] in [`SearchMode::Go`]
//!   (prefix + BM25, recency seam). Navigates to an entity.
//! - **Do** (`>`): command runner over a static command registry. Acts, never
//!   navigates.
//! - **Ask** (`?`): hybrid RAG (retrieval reused, answer in `ai-workspace`).
//! - **Scoped** (`#` / `@` / `[[`): constrained search over tags / people / notes.
//!
//! Go and Do are the two models this Phase-1 slice fully specifies; Ask/Scoped
//! carry their sigil + payload so the router can dispatch, with retrieval reusing
//! the same [`SearchQuery`] spine.

use app_domain::Day;
use serde::{Deserialize, Serialize};

use crate::query::{SearchMode, SearchQuery};

/// The entity scope a `#`/`@`/`[[` sigil constrains search to (Feature Specs §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// `#` — tags.
    Tag,
    /// `@` — people.
    Person,
    /// `[[` — note titles (wikilink autocomplete).
    Note,
}

/// Which palette mode an input string selects, by its leading sigil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteMode {
    Go,
    Do,
    Ask,
    Scoped(ScopeKind),
}

/// A palette input classified into `{mode, body}` — the sigil stripped off.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaletteInput {
    pub mode: PaletteMode,
    /// The query text with the mode sigil removed and outer whitespace trimmed.
    pub body: String,
}

/// Classify a raw palette string by its leading sigil (Feature Specs §7.1).
///
/// With no sigil, Go is the default. `[[` is checked before single-char sigils so
/// it wins over any accidental first-char overlap.
#[must_use]
pub fn classify(input: &str) -> PaletteInput {
    let trimmed = input.trim_start();
    if let Some(rest) = trimmed.strip_prefix("[[") {
        return PaletteInput {
            mode: PaletteMode::Scoped(ScopeKind::Note),
            body: rest.trim().to_string(),
        };
    }
    let mut chars = trimmed.chars();
    let (mode, skip) = match chars.next() {
        Some('>') => (PaletteMode::Do, 1),
        Some('?') => (PaletteMode::Ask, 1),
        Some('#') => (PaletteMode::Scoped(ScopeKind::Tag), 1),
        Some('@') => (PaletteMode::Scoped(ScopeKind::Person), 1),
        _ => (PaletteMode::Go, 0),
    };
    PaletteInput {
        mode,
        body: trimmed[skip..].trim().to_string(),
    }
}

/// The **Go** quick-switcher model: a Go-mode [`SearchQuery`] plus the recency
/// boost seam. Ordering is BM25 rank with a recency tie-break (Feature Specs
/// §7.1 "recency-boosted"); the boost is applied at fusion/ordering time — the
/// SQL keeps `ORDER BY rank`, and [`GoQuery::recency_boost`] documents the seam
/// the ranker will consume.
#[derive(Clone, Debug, PartialEq)]
pub struct GoQuery {
    pub search: SearchQuery,
    /// Weight applied to `entity.updated_at` recency when breaking BM25 ties.
    /// `0.0` = pure BM25. Consumed by the ranker/fusion step, not the SQL.
    pub recency_boost: f64,
}

impl GoQuery {
    /// Default recency weight for the quick-switcher.
    pub const DEFAULT_RECENCY_BOOST: f64 = 0.15;

    /// Build a Go quick-switcher from an already-sigil-stripped body.
    #[must_use]
    pub fn new(body: &str, today: Day) -> Self {
        Self {
            search: SearchQuery::parse(body, SearchMode::Go, today),
            recency_boost: Self::DEFAULT_RECENCY_BOOST,
        }
    }
}

/// A runnable palette command in **Do** mode (Feature Specs §7.1: "create note,
/// start meeting, toggle theme, open settings…"). The registry is static; the
/// actual effect is dispatched by `app-service`/`tauri-app`, not here.
///
/// Only `Serialize` is derived: the registry is code-defined with `&'static`
/// data and is emitted to the UI, never deserialized back.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct DoCommandSpec {
    /// Stable dispatch id (e.g. `"note.create"`).
    pub id: &'static str,
    /// Human-facing label shown in the palette.
    pub title: &'static str,
    /// Extra match keywords beyond the title words.
    pub keywords: &'static [&'static str],
}

/// The built-in Do-mode command registry. Kept static and side-effect-free; a
/// later phase can extend it with context-sensitive commands.
#[must_use]
pub fn builtin_commands() -> &'static [DoCommandSpec] {
    const CMDS: &[DoCommandSpec] = &[
        DoCommandSpec {
            id: "note.create",
            title: "Create note",
            keywords: &["new", "page"],
        },
        DoCommandSpec {
            id: "note.today",
            title: "Open today",
            keywords: &["daily", "journal"],
        },
        DoCommandSpec {
            id: "task.create",
            title: "New task",
            keywords: &["todo", "add"],
        },
        DoCommandSpec {
            id: "meeting.start",
            title: "Start meeting",
            keywords: &["record", "capture"],
        },
        DoCommandSpec {
            id: "reminder.create",
            title: "New reminder",
            keywords: &["remind", "alert"],
        },
        DoCommandSpec {
            id: "theme.toggle",
            title: "Toggle theme",
            keywords: &["dark", "light", "appearance"],
        },
        DoCommandSpec {
            id: "settings.open",
            title: "Open settings",
            keywords: &["preferences", "config"],
        },
        DoCommandSpec {
            id: "export.note",
            title: "Export note",
            keywords: &["markdown", "save"],
        },
    ];
    CMDS
}

/// Filter the Do-mode command registry by a subsequence match against each
/// command's title + keywords (a lightweight fuzzy filter). An empty query
/// returns the full registry (the palette shows everything).
#[must_use]
pub fn match_commands(query: &str) -> Vec<&'static DoCommandSpec> {
    let q = query.trim().to_ascii_lowercase();
    let all = builtin_commands();
    if q.is_empty() {
        return all.iter().collect();
    }
    all.iter()
        .filter(|c| {
            let title = c.title.to_ascii_lowercase();
            is_subsequence(&q, &title)
                || c.keywords
                    .iter()
                    .any(|k| is_subsequence(&q, &k.to_ascii_lowercase()))
        })
        .collect()
}

/// True if every char of `needle` appears in `haystack` in order (fuzzy match).
fn is_subsequence(needle: &str, haystack: &str) -> bool {
    let mut hay = haystack.chars();
    for nc in needle.chars() {
        if nc.is_whitespace() {
            continue;
        }
        if !hay.any(|hc| hc == nc) {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn today() -> Day {
        Day::from_str("2026-07-23").unwrap()
    }

    #[test]
    fn classify_detects_each_sigil() {
        assert_eq!(classify("foo").mode, PaletteMode::Go);
        assert_eq!(classify("> create").mode, PaletteMode::Do);
        assert_eq!(classify("? what is").mode, PaletteMode::Ask);
        assert_eq!(classify("#work").mode, PaletteMode::Scoped(ScopeKind::Tag));
        assert_eq!(
            classify("@alice").mode,
            PaletteMode::Scoped(ScopeKind::Person)
        );
        assert_eq!(classify("[[Q3").mode, PaletteMode::Scoped(ScopeKind::Note));
    }

    #[test]
    fn classify_strips_sigil_and_trims_body() {
        assert_eq!(classify(">  create note").body, "create note");
        assert_eq!(classify("[[ Q3 Plan").body, "Q3 Plan");
        assert_eq!(classify("plain text").body, "plain text");
    }

    #[test]
    fn go_query_is_prefix_mode() {
        let g = GoQuery::new("quart", today());
        assert_eq!(g.search.mode, SearchMode::Go);
        assert_eq!(g.recency_boost, GoQuery::DEFAULT_RECENCY_BOOST);
        // prefix match on the single token
        assert!(!g.search.is_recents());
    }

    #[test]
    fn match_commands_empty_returns_all() {
        assert_eq!(match_commands("").len(), builtin_commands().len());
    }

    #[test]
    fn match_commands_fuzzy_matches_title_and_keywords() {
        let hits = match_commands("meet");
        assert!(hits.iter().any(|c| c.id == "meeting.start"));
        // keyword "daily" should surface "Open today"
        let hits = match_commands("daily");
        assert!(hits.iter().any(|c| c.id == "note.today"));
    }

    #[test]
    fn match_commands_subsequence_not_substring() {
        // "crn" is a subsequence of "create note" (c-r...n), so it matches.
        let hits = match_commands("crn");
        assert!(hits.iter().any(|c| c.id == "note.create"));
    }
}

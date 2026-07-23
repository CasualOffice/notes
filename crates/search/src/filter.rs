//! The typed-filter grammar (`type:` / `tag:` / `date:` / `person:` / `is:`) and
//! its compilation to SQL `WHERE` predicates. Implements **Feature Specs §7.2**
//! and the "filters compile to SQL predicates applied *before* fusion" rule of
//! **Data Model §10.1 / HLD §8.5**.
//!
//! Parsing is intentionally infallible-at-token-level: a `key:value` token whose
//! value doesn't parse falls back to free-text rather than rejecting the whole
//! query (keeps the palette responsive). Callers that need strictness can inspect
//! [`ParsedInput::unparsed_filters`].
//!
//! Compilation is **per source**: the same [`Filters`] set yields a different
//! [`WhereClause`] for `fts_task` (references `t.status`, `t.deadline_on`) than for
//! `fts_note` (references `n.daily_date`). Predicates that don't apply to a source
//! are dropped, so `is:open` is a no-op on a note query rather than an error.

use app_domain::Day;
use serde::{Deserialize, Serialize};

use crate::fts::FtsSource;
use crate::sql::{SqlParam, WhereClause};

/// `type:` — the pillar the hit must belong to (Feature Specs §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TypeFilter {
    Note,
    Task,
    Reminder,
    Meeting,
}

impl TypeFilter {
    fn parse(v: &str) -> Option<Self> {
        Some(match v.to_ascii_lowercase().as_str() {
            "note" => Self::Note,
            "task" => Self::Task,
            "reminder" => Self::Reminder,
            "meeting" | "session" => Self::Meeting,
            _ => return None,
        })
    }

    /// The FTS source this type restricts to. `Reminder` has **no** FTS5 table in
    /// Phase 1 (Data Model §10 defines note/task/transcript/chunk only), so it
    /// yields `None` — a documented gap the integration phase must reconcile.
    #[must_use]
    pub const fn source(self) -> Option<FtsSource> {
        match self {
            Self::Note => Some(FtsSource::Note),
            Self::Task => Some(FtsSource::Task),
            Self::Meeting => Some(FtsSource::Transcript),
            Self::Reminder => None,
        }
    }
}

/// `date:` — a range or keyword resolved against the caller-supplied "today".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DateSpec {
    /// `date:today`
    Today,
    /// `date:overdue` (deadline strictly before today, still open)
    Overdue,
    /// `date:upcoming` (strictly after today)
    Upcoming,
    /// `date:YYYY-MM-DD`
    On(Day),
    /// `date:YYYY-MM-DD..YYYY-MM-DD` (inclusive)
    Range(Day, Day),
    /// `date:<YYYY-MM-DD`
    Before(Day),
    /// `date:>YYYY-MM-DD`
    After(Day),
}

impl DateSpec {
    fn parse(v: &str) -> Option<Self> {
        match v.to_ascii_lowercase().as_str() {
            "today" => return Some(Self::Today),
            "overdue" => return Some(Self::Overdue),
            "upcoming" | "future" => return Some(Self::Upcoming),
            _ => {}
        }
        if let Some(rest) = v.strip_prefix('<') {
            return rest.parse::<Day>().ok().map(Self::Before);
        }
        if let Some(rest) = v.strip_prefix('>') {
            return rest.parse::<Day>().ok().map(Self::After);
        }
        if let Some((a, b)) = v.split_once("..") {
            let (a, b) = (a.parse::<Day>().ok()?, b.parse::<Day>().ok()?);
            return Some(Self::Range(a, b));
        }
        v.parse::<Day>().ok().map(Self::On)
    }
}

/// `is:` — a state predicate (Feature Specs §7.2: `is:open`/`is:done`/`is:missed`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IsFilter {
    Open,
    Done,
    Missed,
    Pinned,
}

impl IsFilter {
    fn parse(v: &str) -> Option<Self> {
        Some(match v.to_ascii_lowercase().as_str() {
            "open" => Self::Open,
            "done" | "completed" => Self::Done,
            "missed" => Self::Missed,
            "pinned" => Self::Pinned,
            _ => return None,
        })
    }
}

/// A parsed, AND-combined filter set (Feature Specs §7.2: "filters combine (AND)").
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Filters {
    pub types: Vec<TypeFilter>,
    pub tags: Vec<String>,
    pub date: Option<DateSpec>,
    pub persons: Vec<String>,
    pub is: Vec<IsFilter>,
}

impl Filters {
    /// True when no filter is set (Go over the whole store).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
            && self.tags.is_empty()
            && self.date.is_none()
            && self.persons.is_empty()
            && self.is.is_empty()
    }

    /// The FTS sources this filter set permits, given a default set to fall back
    /// to when no `type:` is present. `Reminder` types drop out (no FTS table).
    #[must_use]
    pub fn active_sources(&self, default: &[FtsSource]) -> Vec<FtsSource> {
        if self.types.is_empty() {
            return default.to_vec();
        }
        let mut out = Vec::new();
        for t in &self.types {
            if let Some(s) = t.source() {
                if !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        out
    }

    /// Compile every applicable predicate into one AND-ed [`WhereClause`] for
    /// `source`. `today` resolves relative date keywords. Predicates that don't
    /// apply to the source's detail table are silently skipped.
    #[must_use]
    pub fn compile_for(&self, source: FtsSource, today: Day) -> WhereClause {
        let mut w = WhereClause::empty();

        // is:  — task status lives on `task`; pinned on `note`.
        for f in &self.is {
            match (f, source) {
                (IsFilter::Open, FtsSource::Task) => w.and_raw("t.status = 'open'", vec![]),
                (IsFilter::Done, FtsSource::Task) => w.and_raw("t.status = 'completed'", vec![]),
                (IsFilter::Pinned, FtsSource::Note) => w.and_raw("n.is_pinned = 1", vec![]),
                // is:missed targets `reminder` (no FTS source in Phase 1) — skip.
                _ => {}
            }
        }

        // date:
        if let Some(d) = self.date {
            let (frag, params) = compile_date(d, source, today);
            w.and_raw(&frag, params);
        }

        // tag:  — a `link(rel='tagged')` edge to a Tag whose case-folded name matches.
        for tag in &self.tags {
            w.and_raw(
                "EXISTS (SELECT 1 FROM link l JOIN tag tg ON tg.entity_id = l.dst_entity \
                 WHERE l.src_entity = e.id AND l.rel = 'tagged' AND l.deleted_at IS NULL \
                 AND tg.name = ?)",
                vec![SqlParam::text(tag.to_ascii_lowercase())],
            );
        }

        // person:  — a `mention` edge to a Person entity by case-folded title.
        for p in &self.persons {
            w.and_raw(
                "EXISTS (SELECT 1 FROM link l JOIN entity pe ON pe.id = l.dst_entity \
                 WHERE l.src_entity = e.id AND l.rel = 'mention' AND l.deleted_at IS NULL \
                 AND pe.kind = 'person' AND lower(pe.title) = ?)",
                vec![SqlParam::text(p.to_ascii_lowercase())],
            );
        }

        w
    }
}

/// Compile a [`DateSpec`] for one source. `task` distinguishes `start_on` (When,
/// hides) and `deadline_on` (Due, shows) per Data Model §6.3; `note` matches its
/// `daily_date`. Sources without a date column return an empty fragment.
fn compile_date(spec: DateSpec, source: FtsSource, today: Day) -> (String, Vec<SqlParam>) {
    let today = SqlParam::text(today.to_string());
    match source {
        FtsSource::Task => match spec {
            DateSpec::Today => (
                "(t.start_on <= ? OR t.deadline_on <= ?)".into(),
                vec![today.clone(), today],
            ),
            DateSpec::Overdue => (
                "(t.deadline_on < ? AND t.status = 'open')".into(),
                vec![today],
            ),
            DateSpec::Upcoming => (
                "(t.start_on > ? OR t.deadline_on > ?)".into(),
                vec![today.clone(), today],
            ),
            DateSpec::On(d) => {
                let d = SqlParam::text(d.to_string());
                (
                    "(t.start_on = ? OR t.deadline_on = ?)".into(),
                    vec![d.clone(), d],
                )
            }
            DateSpec::Range(a, b) => {
                let (a, b) = (SqlParam::text(a.to_string()), SqlParam::text(b.to_string()));
                (
                    "((t.start_on BETWEEN ? AND ?) OR (t.deadline_on BETWEEN ? AND ?))".into(),
                    vec![a.clone(), b.clone(), a, b],
                )
            }
            DateSpec::Before(d) => {
                let d = SqlParam::text(d.to_string());
                (
                    "(t.deadline_on < ? OR t.start_on < ?)".into(),
                    vec![d.clone(), d],
                )
            }
            DateSpec::After(d) => {
                let d = SqlParam::text(d.to_string());
                (
                    "(t.deadline_on > ? OR t.start_on > ?)".into(),
                    vec![d.clone(), d],
                )
            }
        },
        FtsSource::Note => match spec {
            DateSpec::Today | DateSpec::On(_) => {
                let d = match spec {
                    DateSpec::On(d) => SqlParam::text(d.to_string()),
                    _ => today,
                };
                ("n.daily_date = ?".into(), vec![d])
            }
            DateSpec::Upcoming => ("n.daily_date > ?".into(), vec![today]),
            DateSpec::Before(d) => (
                "n.daily_date < ?".into(),
                vec![SqlParam::text(d.to_string())],
            ),
            DateSpec::After(d) => (
                "n.daily_date > ?".into(),
                vec![SqlParam::text(d.to_string())],
            ),
            DateSpec::Range(a, b) => (
                "n.daily_date BETWEEN ? AND ?".into(),
                vec![SqlParam::text(a.to_string()), SqlParam::text(b.to_string())],
            ),
            // `overdue` is meaningless for a note's own daily_date — skip.
            DateSpec::Overdue => (String::new(), vec![]),
        },
        // transcript/chunk have no first-class date column in Phase 1.
        FtsSource::Transcript | FtsSource::Chunk => (String::new(), vec![]),
    }
}

/// The result of splitting a palette input into free-text plus typed filters.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ParsedInput {
    /// Everything that wasn't a recognized filter token, re-joined with spaces.
    pub text: String,
    /// The recognized, typed filters.
    pub filters: Filters,
    /// `key:value` tokens whose key was known but value failed to parse. Retained
    /// so a strict caller can surface an error instead of silently searching text.
    pub unparsed_filters: Vec<String>,
}

/// Split a raw query into free-text and typed filters (Feature Specs §7.2).
///
/// Supports simple double-quoted values (`tag:"Q3 Planning"`). A `key:value`
/// whose value fails to parse is recorded in [`ParsedInput::unparsed_filters`] and
/// also kept as free text so the user still gets results.
#[must_use]
pub fn parse_query(input: &str) -> ParsedInput {
    let mut out = ParsedInput::default();
    let mut text_tokens: Vec<String> = Vec::new();

    for raw in tokenize(input) {
        let Some((key, value)) = split_filter(&raw) else {
            text_tokens.push(raw);
            continue;
        };
        let recognized = apply_filter(&mut out.filters, &key, &value);
        match recognized {
            FilterOutcome::Applied => {}
            FilterOutcome::BadValue => {
                out.unparsed_filters.push(raw.clone());
                text_tokens.push(raw);
            }
            FilterOutcome::UnknownKey => text_tokens.push(raw),
        }
    }

    out.text = text_tokens.join(" ");
    out
}

enum FilterOutcome {
    Applied,
    BadValue,
    UnknownKey,
}

fn apply_filter(f: &mut Filters, key: &str, value: &str) -> FilterOutcome {
    if value.is_empty() {
        return FilterOutcome::UnknownKey;
    }
    match key {
        "type" => match TypeFilter::parse(value) {
            Some(t) => {
                if !f.types.contains(&t) {
                    f.types.push(t);
                }
                FilterOutcome::Applied
            }
            None => FilterOutcome::BadValue,
        },
        "tag" => {
            f.tags.push(value.to_string());
            FilterOutcome::Applied
        }
        "person" => {
            f.persons.push(value.to_string());
            FilterOutcome::Applied
        }
        "is" => match IsFilter::parse(value) {
            Some(i) => {
                if !f.is.contains(&i) {
                    f.is.push(i);
                }
                FilterOutcome::Applied
            }
            None => FilterOutcome::BadValue,
        },
        "date" => match DateSpec::parse(value) {
            Some(d) => {
                f.date = Some(d);
                FilterOutcome::Applied
            }
            None => FilterOutcome::BadValue,
        },
        _ => FilterOutcome::UnknownKey,
    }
}

/// Split `key:value`, unquoting a `"…"` value. Returns `None` when there's no `:`.
/// Both halves are owned so the caller can freely move the source token.
fn split_filter(tok: &str) -> Option<(String, String)> {
    let (key, value) = tok.split_once(':')?;
    if key.is_empty() {
        return None;
    }
    Some((key.to_string(), value.trim_matches('"').to_string()))
}

/// Whitespace tokenizer that keeps `"…"`-quoted spans (including a `key:"…"`
/// prefix) as a single token.
fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in input.chars() {
        match ch {
            '"' => {
                in_quote = !in_quote;
                cur.push(ch);
            }
            c if c.is_whitespace() && !in_quote => {
                if !cur.is_empty() {
                    tokens.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn today() -> Day {
        Day::from_str("2026-07-23").unwrap()
    }

    #[test]
    fn parse_splits_filters_from_text() {
        let p = parse_query("quarterly type:task is:open tag:Work");
        assert_eq!(p.text, "quarterly");
        assert_eq!(p.filters.types, vec![TypeFilter::Task]);
        assert_eq!(p.filters.is, vec![IsFilter::Open]);
        assert_eq!(p.filters.tags, vec!["Work".to_string()]);
    }

    #[test]
    fn parse_keeps_quoted_tag_value() {
        let p = parse_query(r#"tag:"Q3 Planning" notes"#);
        assert_eq!(p.filters.tags, vec!["Q3 Planning".to_string()]);
        assert_eq!(p.text, "notes");
    }

    #[test]
    fn parse_unknown_key_is_free_text() {
        let p = parse_query("foo:bar hello");
        assert!(p.filters.is_empty());
        assert_eq!(p.text, "foo:bar hello");
    }

    #[test]
    fn parse_bad_filter_value_is_recorded_and_kept_as_text() {
        let p = parse_query("is:banana");
        assert!(p.filters.is.is_empty());
        assert_eq!(p.unparsed_filters, vec!["is:banana".to_string()]);
        assert_eq!(p.text, "is:banana");
    }

    #[test]
    fn date_keywords_and_ranges_parse() {
        assert_eq!(DateSpec::parse("today"), Some(DateSpec::Today));
        assert_eq!(DateSpec::parse("overdue"), Some(DateSpec::Overdue));
        assert_eq!(DateSpec::parse("2026-07-23"), Some(DateSpec::On(today())));
        assert!(matches!(
            DateSpec::parse("2026-01-01..2026-12-31"),
            Some(DateSpec::Range(_, _))
        ));
        assert!(matches!(
            DateSpec::parse("<2026-07-23"),
            Some(DateSpec::Before(_))
        ));
        assert!(matches!(
            DateSpec::parse(">2026-07-23"),
            Some(DateSpec::After(_))
        ));
        assert_eq!(DateSpec::parse("nope"), None);
    }

    #[test]
    fn active_sources_defaults_when_no_type() {
        let f = Filters::default();
        assert_eq!(
            f.active_sources(&[FtsSource::Note, FtsSource::Task]),
            vec![FtsSource::Note, FtsSource::Task]
        );
    }

    #[test]
    fn active_sources_drops_reminder_type() {
        let f = Filters {
            types: vec![TypeFilter::Reminder, TypeFilter::Task],
            ..Default::default()
        };
        assert_eq!(f.active_sources(&[FtsSource::Note]), vec![FtsSource::Task]);
    }

    #[test]
    fn compile_task_is_open_and_tag() {
        let f = Filters {
            is: vec![IsFilter::Open],
            tags: vec!["Work".to_string()],
            ..Default::default()
        };
        let w = f.compile_for(FtsSource::Task, today());
        assert!(w.sql.contains("t.status = 'open'"));
        assert!(w.sql.contains("l.rel = 'tagged'"));
        // tag value is lower-cased for the case-folded tag.name match.
        assert_eq!(w.params, vec![SqlParam::text("work")]);
    }

    #[test]
    fn compile_task_date_today_binds_today_twice() {
        let f = Filters {
            date: Some(DateSpec::Today),
            ..Default::default()
        };
        let w = f.compile_for(FtsSource::Task, today());
        assert!(w.sql.contains("t.start_on <= ?"));
        assert!(w.sql.contains("t.deadline_on <= ?"));
        assert_eq!(
            w.params,
            vec![SqlParam::text("2026-07-23"), SqlParam::text("2026-07-23")]
        );
    }

    #[test]
    fn compile_note_drops_task_only_is_filter() {
        // is:open is a task predicate; on a note source it must vanish, not error.
        let f = Filters {
            is: vec![IsFilter::Open],
            ..Default::default()
        };
        let w = f.compile_for(FtsSource::Note, today());
        assert!(w.is_empty());
    }
}

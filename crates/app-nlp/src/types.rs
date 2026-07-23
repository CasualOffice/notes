//! The `ParsedEntry` contract and its sub-structures. Implements **Data Model
//! §14.3** (`ParsedEntry` natural-language quick entry) verbatim, plus the
//! UI-side highlight spans of the **Feature Specs §2.1** live-highlight preview.
//!
//! `ParsedEntry` is the *stored/routed* contract and matches §14.3 field-for-field.
//! [`HighlightSpan`]s are a UI concern (byte ranges into the raw input for inline
//! colouring) and are therefore returned alongside — never *inside* — the entry via
//! [`ParseResult`], so the serialized `ParsedEntry` stays byte-identical to §14.3.

use app_domain::time::{Day, Timestamp};
use serde::{Deserialize, Serialize};

/// The route a quick-capture string resolves to (Feature Specs §2.1). Serialized
/// as the §14.3 `kind` field: `"note" | "task" | "reminder"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Route {
    /// Appended as a bullet to today's daily note (§2.1 "otherwise → NOTE").
    Note,
    /// A first-class task; a stated date lands on `deadline_on`/`start_on`.
    Task,
    /// A standalone reminder with an absolute `fire_at`.
    Reminder,
}

/// Recurrence advance mode (Data Model §7.2 / Feature Specs §4.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceMode {
    /// Todoist `every` — next instance = next RRULE occurrence after the
    /// *scheduled* time, regardless of when completed.
    Fixed,
    /// Todoist `every!` — next instance materialized as *completion time + interval*.
    AfterCompletion,
}

/// A task-side recurrence template (§14.3 `recurrence`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Recurrence {
    /// RFC-5545 RRULE string (e.g. `FREQ=MONTHLY;BYMONTHDAY=1`).
    pub rrule: String,
    /// `fixed` vs `after_completion`.
    pub mode: RecurrenceMode,
}

/// A reminder specification (§14.3 `reminder`). `fire_at` is an absolute instant;
/// `tz` is the origin IANA zone so DST shifts render correctly (Feature Specs §4.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReminderSpec {
    /// Absolute first-fire instant (epoch-milliseconds UTC — matches
    /// [`app_domain::time::Timestamp`]).
    pub fire_at: Timestamp,
    /// The origin IANA timezone name (e.g. `America/New_York`), carried from the
    /// [`ParseContext`](crate::ParseContext); this crate never resolves IANA zones
    /// itself (chrono-only, no tz database).
    pub tz: String,
    /// Optional RRULE for a recurring reminder (§2.2 "Review PRs every weekday 9am").
    pub rrule: Option<String>,
}

/// The natural-language quick-entry result. **Data Model §14.3** — every field maps
/// 1:1 to the JSON contract. The parser **never invents a date the user didn't
/// state**; ambiguous input yields low `confidence` and null dates (§2.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParsedEntry {
    /// Schema discriminator, always `"ParsedEntry"`.
    #[serde(default = "parsed_entry_schema")]
    pub schema: String,
    /// The chosen route (§14.3 `kind`).
    pub kind: Route,
    /// The item title with all recognized tokens stripped and whitespace collapsed.
    pub title: String,
    /// Start date (`YYYY-MM-DD`) or null — only when the user stated one.
    pub start_on: Option<Day>,
    /// Deadline date (`YYYY-MM-DD`) or null — only when the user stated one.
    pub deadline_on: Option<Day>,
    /// Reminder spec, or null when the route is not a reminder.
    pub reminder: Option<ReminderSpec>,
    /// Task-side recurrence template, or null.
    pub recurrence: Option<Recurrence>,
    /// The first inline `#project`, or null.
    pub project: Option<String>,
    /// Inline `#tag`s (every `#token` after the first project token).
    pub tags: Vec<String>,
    /// The first inline `@person`, or null.
    pub assignee: Option<String>,
    /// Inline `!priority` (0 = none; 1 = highest).
    pub priority: i32,
    /// Parser confidence in `0.0..=1.0`. Below [`LLM_FALLBACK_THRESHOLD`] the
    /// caller should defer to the Phase-2 LLM fallback (§2.1).
    pub confidence: f32,
    /// Always `false` from this fast-path crate; the LLM fallback sets it true.
    pub used_llm_fallback: bool,
}

fn parsed_entry_schema() -> String {
    "ParsedEntry".to_string()
}

impl ParsedEntry {
    /// The schema tag written into `ParsedEntry.schema`.
    pub const SCHEMA: &'static str = "ParsedEntry";

    /// Below this confidence the grammar fast-path is not trusted and the caller
    /// should route to the resident-LLM fallback (Feature Specs §2.1, Data Model
    /// §14.3 "LLM fallback only on low confidence").
    #[must_use]
    pub fn needs_llm_fallback(&self) -> bool {
        self.confidence < LLM_FALLBACK_THRESHOLD
    }
}

/// Confidence at/above which the grammar fast-path is trusted without an LLM.
pub const LLM_FALLBACK_THRESHOLD: f32 = 0.5;

/// The kind of a recognized inline token, for live UI highlighting (§2.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenKind {
    /// A resolved calendar date phrase (`tomorrow`, `next friday`, `jul 24`).
    Date,
    /// A clock time (`3pm`, `15:00`, `noon`).
    Time,
    /// An `every`/`every!` recurrence phrase.
    Recurrence,
    /// The leading `#project` token.
    Project,
    /// A `#tag` token.
    Tag,
    /// An `@person` mention.
    Mention,
    /// A `!priority` token.
    Priority,
    /// An intent keyword (`remind me`, `todo`).
    Keyword,
}

/// A recognized inline span, as **byte offsets** into the original input string.
/// Half-open `[start, end)`; always on UTF-8 char boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HighlightSpan {
    /// Inclusive start byte offset into the raw input.
    pub start: usize,
    /// Exclusive end byte offset into the raw input.
    pub end: usize,
    /// What the span is.
    pub token: TokenKind,
}

impl HighlightSpan {
    #[must_use]
    pub(crate) fn new(start: usize, end: usize, token: TokenKind) -> Self {
        Self { start, end, token }
    }
}

/// The full output of [`parse`](crate::parse): the §14.3 entry plus UI highlight
/// spans (sorted by `start`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ParseResult {
    /// The routed `ParsedEntry` (Data Model §14.3 contract).
    pub entry: ParsedEntry,
    /// Inline highlight spans over the raw input, ascending by `start`.
    pub highlights: Vec<HighlightSpan>,
}

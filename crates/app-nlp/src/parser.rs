//! The grammar fast-path orchestrator. Implements the quick-capture parse +
//! routing of **Feature Specs §2.1** and the **HLD §8.2** `nlp.parse` flow, emitting
//! a [`ParsedEntry`] (Data Model §14.3) plus inline [`HighlightSpan`]s.
//!
//! Pipeline (each pass masks the spans it consumes so later passes can't re-match):
//! `!priority` → `#project`/`#tag` → `@mention` → intent keywords → recurrence →
//! date → time → vague-temporal → title. Then route (§2.1 first-match-wins) and
//! confidence. **No date is ever invented** — vague input yields low confidence and
//! null dates so the Phase-2 LLM fallback decides (§2.2).

use app_domain::time::{Day, Timestamp};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::context::ParseContext;
use crate::datetime::{scan_date, scan_time, scan_vague, DateHit, TimeHit};
use crate::recurrence::scan_recurrence;
use crate::types::{
    HighlightSpan, ParseResult, ParsedEntry, Recurrence, RecurrenceMode, ReminderSpec, Route,
    TokenKind,
};

static RE_PRIORITY: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)!(p?[1-4]|high|urgent|medium|med|low)\b").unwrap());
static RE_PROJECT: Lazy<Regex> = Lazy::new(|| Regex::new(r"#([A-Za-z][\w/-]*)").unwrap());
static RE_MENTION: Lazy<Regex> = Lazy::new(|| Regex::new(r"@([A-Za-z][\w.\-]*)").unwrap());
static RE_TODO_PREFIX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)^\s*(todo|task|td)\b[:\-]?\s*").unwrap());
static RE_REMIND: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(remind me|reminder|remind)\b").unwrap());
static RE_START_CUE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?i)\b(start|starting|starts|from)\b").unwrap());

/// Common action verbs for the "starts with a verb" routing test (§2.1). Not
/// exhaustive — a miss simply routes to Note (the safe default, never loses text).
static VERBS: &[&str] = &[
    "buy", "call", "email", "send", "review", "draft", "write", "read", "pay", "water", "finish",
    "fix", "update", "check", "meet", "prepare", "plan", "order", "renew", "cancel", "clean",
    "book", "schedule", "submit", "ship", "deploy", "test", "refactor", "reply", "respond", "ask",
    "tell", "pick", "get", "make", "create", "build", "design", "research", "follow", "wrap",
    "sign", "file", "print", "scan", "upload", "download", "install", "backup", "publish", "merge",
    "confirm", "remind", "renew", "return", "collect", "organize",
];

/// Parse a quick-capture string into a [`ParseResult`] (Data Model §14.3 entry +
/// UI highlight spans). Pure and deterministic given `ctx` (HLD §8.2).
#[must_use]
pub fn parse(input: &str, ctx: &ParseContext) -> ParseResult {
    let mut masked: Vec<u8> = input.as_bytes().to_vec();
    let mut hl: Vec<HighlightSpan> = Vec::new();

    // --- priority -------------------------------------------------------
    let mut priority = 0i32;
    if let Some((range, p)) = find_priority(current(&masked)) {
        priority = p;
        push(&mut masked, &mut hl, range, TokenKind::Priority);
    }

    // --- #project / #tags ----------------------------------------------
    let mut project: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    for (i, (range, name)) in find_all(&RE_PROJECT, current(&masked))
        .into_iter()
        .enumerate()
    {
        if i == 0 {
            project = Some(name);
            push(&mut masked, &mut hl, range, TokenKind::Project);
        } else {
            tags.push(name);
            push(&mut masked, &mut hl, range, TokenKind::Tag);
        }
    }

    // --- @mention -------------------------------------------------------
    let mut assignee: Option<String> = None;
    for (i, (range, name)) in find_all(&RE_MENTION, current(&masked))
        .into_iter()
        .enumerate()
    {
        if i == 0 {
            assignee = Some(name);
        }
        push(&mut masked, &mut hl, range, TokenKind::Mention);
    }

    // --- intent keywords ------------------------------------------------
    let mut has_task_keyword = false;
    if let Some(range) = RE_TODO_PREFIX
        .find(current(&masked))
        .map(|m| (m.start(), m.end()))
    {
        has_task_keyword = true;
        push(&mut masked, &mut hl, range, TokenKind::Keyword);
    }
    let mut has_remind_cue = false;
    if let Some(range) = RE_REMIND
        .find(current(&masked))
        .map(|m| (m.start(), m.end()))
    {
        has_remind_cue = true;
        push(&mut masked, &mut hl, range, TokenKind::Keyword);
    }

    // --- recurrence -----------------------------------------------------
    let recurrence = scan_recurrence(current(&masked));
    if let Some(rp) = &recurrence {
        push(&mut masked, &mut hl, rp.range, TokenKind::Recurrence);
    }

    // --- date -----------------------------------------------------------
    let date_hit: Option<DateHit> = scan_date(current(&masked), ctx);
    if let Some(d) = &date_hit {
        push(&mut masked, &mut hl, d.range, TokenKind::Date);
    }

    // --- time -----------------------------------------------------------
    let time_hit: Option<TimeHit> = scan_time(current(&masked));
    if let Some(t) = &time_hit {
        push(&mut masked, &mut hl, t.range, TokenKind::Time);
    }

    // --- vague temporal (only matters if no concrete date) --------------
    let vague = date_hit.is_none() && scan_vague(current(&masked)).is_some();

    // start-vs-deadline cue (searched on the raw input, order-independent)
    let start_cue = RE_START_CUE.is_match(input);

    // --- title ----------------------------------------------------------
    let had_keyword = has_task_keyword || has_remind_cue;
    let title = build_title(current(&masked), had_keyword);

    // --- route + field mapping + confidence -----------------------------
    let mut entry = ParsedEntry {
        schema: ParsedEntry::SCHEMA.to_string(),
        kind: Route::Note,
        title,
        start_on: None,
        deadline_on: None,
        reminder: None,
        recurrence: None,
        project,
        tags,
        assignee,
        priority,
        confidence: 0.9,
        used_llm_fallback: false,
    };

    let has_verb = starts_with_verb(&entry.title);

    if let Some(rp) = &recurrence {
        // Recurrence drives the route (§2.2).
        match rp.mode {
            RecurrenceMode::AfterCompletion => {
                entry.kind = Route::Task;
                entry.recurrence = Some(Recurrence {
                    rrule: rp.rrule.clone(),
                    mode: rp.mode,
                });
                if let Some(t) = &time_hit {
                    let fire = ctx.local_to_utc_ms(rp.first_occurrence(ctx, t.time));
                    entry.reminder = Some(ReminderSpec {
                        fire_at: Timestamp::from_millis(fire),
                        tz: ctx.tz.clone(),
                        rrule: None, // advance is completion-driven, not fixed
                    });
                }
            }
            RecurrenceMode::Fixed => {
                if let Some(t) = &time_hit {
                    entry.kind = Route::Reminder;
                    let fire = ctx.local_to_utc_ms(rp.first_occurrence(ctx, t.time));
                    entry.reminder = Some(ReminderSpec {
                        fire_at: Timestamp::from_millis(fire),
                        tz: ctx.tz.clone(),
                        rrule: Some(rp.rrule.clone()),
                    });
                } else {
                    entry.kind = Route::Task;
                    entry.recurrence = Some(Recurrence {
                        rrule: rp.rrule.clone(),
                        mode: rp.mode,
                    });
                }
            }
        }
    } else if has_remind_cue || time_hit.is_some() {
        entry.kind = Route::Reminder;
        if let Some(t) = &time_hit {
            let fire = reminder_fire_at(ctx, date_hit.as_ref(), t.time);
            entry.reminder = Some(ReminderSpec {
                fire_at: Timestamp::from_millis(fire),
                tz: ctx.tz.clone(),
                rrule: None,
            });
        } else {
            // "remind me <date>" with no clock time — do NOT invent a time.
            // Defer: reminder unresolved, low confidence, LLM/user picks a time.
            entry.confidence = 0.45;
        }
    } else if (has_verb && date_hit.is_some())
        || has_task_keyword
        || entry.project.is_some()
        || entry.assignee.is_some()
    {
        entry.kind = Route::Task;
        apply_task_date(&mut entry, date_hit.as_ref(), start_cue);
    } else {
        // Otherwise → Note (§2.1). A bare date threads onto the daily note; it is
        // highlighted but not applied as a task date.
        entry.kind = Route::Note;
    }

    // --- confidence adjustments ----------------------------------------
    if vague {
        // Vague temporal word and nothing concrete: never invent a date.
        entry.confidence = entry.confidence.min(0.25);
    }
    if date_hit.map(|d| d.next_weekday_ambiguous).unwrap_or(false) {
        entry.confidence -= 0.1;
    }
    if time_hit.map(|t| t.ambiguous_meridiem).unwrap_or(false) {
        entry.confidence -= 0.2;
    }
    entry.confidence = entry.confidence.clamp(0.0, 1.0);

    hl.sort_by_key(|s| s.start);
    ParseResult {
        entry,
        highlights: hl,
    }
}

// ---------------------------------------------------------------------------
// Field mapping helpers
// ---------------------------------------------------------------------------

fn apply_task_date(entry: &mut ParsedEntry, date_hit: Option<&DateHit>, start_cue: bool) {
    if let Some(d) = date_hit {
        let day = Day::from_naive(d.date);
        if start_cue {
            entry.start_on = Some(day);
        } else {
            entry.deadline_on = Some(day);
        }
    }
}

/// Compute a non-recurring reminder's fire instant (epoch-ms UTC). With an explicit
/// date, that date + time is used verbatim. With only a time, it fires today, or
/// tomorrow if today's slot has already passed — never inventing a *date* the user
/// didn't state beyond the natural "today/next" rollover for a stated time.
fn reminder_fire_at(
    ctx: &ParseContext,
    date_hit: Option<&DateHit>,
    time: chrono::NaiveTime,
) -> i64 {
    let local = match date_hit {
        Some(d) => d.date.and_time(time),
        None => {
            let today = ctx.today().and_time(time);
            if today > ctx.now_naive() {
                today
            } else {
                (ctx.today() + chrono::Duration::days(1)).and_time(time)
            }
        }
    };
    ctx.local_to_utc_ms(local)
}

// ---------------------------------------------------------------------------
// Lexical helpers
// ---------------------------------------------------------------------------

fn current(masked: &[u8]) -> &str {
    // Masking only ever replaces whole char ranges with ASCII spaces, so the buffer
    // is always valid UTF-8.
    std::str::from_utf8(masked).unwrap_or("")
}

fn push(masked: &mut [u8], hl: &mut Vec<HighlightSpan>, range: (usize, usize), token: TokenKind) {
    for b in &mut masked[range.0..range.1] {
        *b = b' ';
    }
    hl.push(HighlightSpan::new(range.0, range.1, token));
}

fn find_all(re: &Regex, text: &str) -> Vec<((usize, usize), String)> {
    re.captures_iter(text)
        .filter_map(|c| {
            let whole = c.get(0)?;
            let name = c.get(1)?.as_str().to_string();
            Some(((whole.start(), whole.end()), name))
        })
        .collect()
}

fn find_priority(text: &str) -> Option<((usize, usize), i32)> {
    let c = RE_PRIORITY.captures(text)?;
    let whole = c.get(0)?;
    let raw = c.get(1)?.as_str().to_ascii_lowercase();
    let p = match raw.as_str() {
        "high" | "urgent" => 1,
        "medium" | "med" => 2,
        "low" => 3,
        other => other.trim_start_matches('p').parse::<i32>().unwrap_or(0),
    };
    Some(((whole.start(), whole.end()), p))
}

fn build_title(masked_text: &str, strip_connectors: bool) -> String {
    let mut title: String = masked_text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Trim leading/trailing connective punctuation.
    title = title
        .trim_matches(|c: char| c == ':' || c == '-' || c == ',')
        .trim()
        .to_string();

    if strip_connectors {
        loop {
            let lower = title.to_ascii_lowercase();
            let first = lower.split_whitespace().next().unwrap_or("");
            let stripped = first.trim_matches(|c: char| !c.is_alphanumeric());
            if matches!(stripped, "to" | "that" | "about" | "re" | "for" | "of") {
                // Drop the first word.
                let rest: String = title
                    .split_once(char::is_whitespace)
                    .map(|x| x.1)
                    .unwrap_or("")
                    .to_string();
                if rest.is_empty() {
                    break;
                }
                title = rest.trim().to_string();
            } else {
                break;
            }
        }
    }
    title
}

fn starts_with_verb(title: &str) -> bool {
    let first = title
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_ascii_lowercase();
    VERBS.contains(&first.as_str())
}

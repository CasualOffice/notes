//! # app-nlp
//!
//! Natural-language quick-entry parsing. Implements the **`ParsedEntry`** contract of
//! **Data Model §14.3** and the quick-capture parse of **HLD §8.2 / Feature Specs
//! §2**: a grammar/regex fast-path with a resident-LLM fallback only on low
//! confidence (the fallback itself is a later phase).
//!
//! Parsing is **pure** (no side effects, all clock/zone state injected via
//! [`ParseContext`]) so the live-highlight preview can be cancelled for free
//! (HLD §8.2). It **never invents a date the user didn't state** (Feature Specs
//! §2.2): vague or ambiguous input yields low [`confidence`](ParsedEntry::confidence)
//! and null dates, and the caller defers to the Phase-2 LLM fallback when
//! [`ParsedEntry::needs_llm_fallback`] is true.
//!
//! ## Entry point
//! ```
//! use app_nlp::{parse, ParseContext, Route};
//! use chrono::{FixedOffset, TimeZone};
//!
//! let now = FixedOffset::west_opt(4 * 3600)
//!     .unwrap()
//!     .with_ymd_and_hms(2026, 7, 23, 10, 0, 0) // Thu
//!     .unwrap();
//! let ctx = ParseContext::new(now, "America/New_York");
//!
//! let out = parse("remind me tomorrow 3pm to call Sam", &ctx);
//! assert_eq!(out.entry.kind, Route::Reminder);
//! assert_eq!(out.entry.title, "call Sam");
//! assert!(out.entry.reminder.is_some());
//! ```
//!
//! ## Modules
//! - [`types`]      — the [`ParsedEntry`] contract (§14.3) + [`HighlightSpan`]s.
//! - [`context`]    — [`ParseContext`], the injected "now"/zone resolution state.
//! - [`parser`]     — the [`parse`] fast-path orchestrator + routing (§2.1).
//! - `datetime`/`recurrence` — the date/time and `every`/`every!` grammars.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod context;
mod datetime;
pub mod parser;
mod recurrence;
pub mod types;

pub use context::ParseContext;
pub use parser::parse;
pub use types::{
    HighlightSpan, ParseResult, ParsedEntry, Recurrence, RecurrenceMode, ReminderSpec, Route,
    TokenKind, LLM_FALLBACK_THRESHOLD,
};

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Timelike};

    /// A fixed reference clock: **Thursday 2026-07-23, 10:00 America/New_York (UTC-4)**.
    /// (The HLD ghost-hint example uses the same day: "next friday" → Fri Jul 24.)
    fn ctx() -> ParseContext {
        let now: DateTime<FixedOffset> = FixedOffset::west_opt(4 * 3600)
            .unwrap()
            .with_ymd_and_hms(2026, 7, 23, 10, 0, 0)
            .unwrap();
        ParseContext::new(now, "America/New_York")
    }

    fn day(y: i32, m: u32, d: u32) -> app_domain::time::Day {
        app_domain::time::Day::from_naive(NaiveDate::from_ymd_opt(y, m, d).unwrap())
    }

    // ---- Feature Specs §2.2 golden corpus (AC-2.2) ---------------------

    #[test]
    fn buy_milk_is_note() {
        let e = parse("Buy milk", &ctx()).entry;
        assert_eq!(e.kind, Route::Note);
        assert_eq!(e.title, "Buy milk");
        assert!(e.start_on.is_none() && e.deadline_on.is_none());
        assert!(e.confidence >= LLM_FALLBACK_THRESHOLD);
    }

    #[test]
    fn todo_task_with_project_priority_and_date() {
        let e = parse("todo Draft Q3 deck #Work !2 friday", &ctx()).entry;
        assert_eq!(e.kind, Route::Task);
        assert_eq!(e.title, "Draft Q3 deck");
        assert_eq!(e.project.as_deref(), Some("Work"));
        assert_eq!(e.priority, 2);
        assert_eq!(e.deadline_on, Some(day(2026, 7, 24))); // coming Friday
        assert!(e.start_on.is_none());
    }

    #[test]
    fn remind_me_tomorrow_3pm() {
        let e = parse("remind me tomorrow 3pm to call Sam", &ctx()).entry;
        assert_eq!(e.kind, Route::Reminder);
        assert_eq!(e.title, "call Sam");
        let r = e.reminder.expect("reminder");
        assert_eq!(r.tz, "America/New_York");
        assert!(r.rrule.is_none());
        // tomorrow 15:00 -04:00 == 19:00 UTC.
        let expected = ctx()
            .now
            .offset()
            .with_ymd_and_hms(2026, 7, 24, 15, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(r.fire_at.as_millis(), expected);
    }

    #[test]
    fn review_prs_every_weekday_9am() {
        let e = parse("Review PRs every weekday 9am", &ctx()).entry;
        assert_eq!(e.kind, Route::Reminder);
        assert_eq!(e.title, "Review PRs");
        let r = e.reminder.expect("reminder");
        assert_eq!(r.rrule.as_deref(), Some("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR"));
        // Today (Thu) 09:00 already passed at 10:00 → first fire Fri Jul 24 09:00.
        let fire = ctx()
            .now
            .offset()
            .with_ymd_and_hms(2026, 7, 24, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(r.fire_at.as_millis(), fire);
    }

    #[test]
    fn water_plants_every_bang_3_days() {
        let e = parse("Water plants every! 3 days", &ctx()).entry;
        assert_eq!(e.kind, Route::Task);
        assert_eq!(e.title, "Water plants");
        let rec = e.recurrence.expect("recurrence");
        assert_eq!(rec.rrule, "FREQ=DAILY;INTERVAL=3");
        assert_eq!(rec.mode, RecurrenceMode::AfterCompletion);
    }

    #[test]
    fn pay_rent_every_month_on_the_1st() {
        let e = parse("Pay rent every month on the 1st !1", &ctx()).entry;
        assert_eq!(e.kind, Route::Task);
        assert_eq!(e.title, "Pay rent");
        assert_eq!(e.priority, 1);
        let rec = e.recurrence.expect("recurrence");
        assert_eq!(rec.rrule, "FREQ=MONTHLY;BYMONTHDAY=1");
        assert_eq!(rec.mode, RecurrenceMode::Fixed);
    }

    // ---- Date grammar --------------------------------------------------

    #[test]
    fn bare_weekday_is_soonest_future() {
        // Thu -> "monday" is next Mon Jul 27.
        let e = parse("Pay invoice monday", &ctx()).entry;
        assert_eq!(e.kind, Route::Task);
        assert_eq!(e.deadline_on, Some(day(2026, 7, 27)));
    }

    #[test]
    fn next_weekday_jumps_a_week_and_lowers_confidence() {
        // Coming Friday is Jul 24; "next friday" = +7 = Jul 31.
        let e = parse("Send report next friday", &ctx()).entry;
        assert_eq!(e.deadline_on, Some(day(2026, 7, 31)));
        assert!(e.confidence < 0.9); // flagged ambiguous
    }

    #[test]
    fn today_is_same_day() {
        assert_eq!(
            parse("Ship build today", &ctx()).entry.deadline_on,
            Some(day(2026, 7, 23))
        );
    }

    #[test]
    fn in_n_days_and_weeks() {
        assert_eq!(
            parse("Review draft in 3 days", &ctx()).entry.deadline_on,
            Some(day(2026, 7, 26))
        );
        assert_eq!(
            parse("Renew lease in 2 weeks", &ctx()).entry.deadline_on,
            Some(day(2026, 8, 6))
        );
    }

    #[test]
    fn month_name_absolute_date_rolls_year_when_past() {
        // "jul 4" is before Jul 23 → next year.
        assert_eq!(
            parse("Buy fireworks jul 4", &ctx()).entry.deadline_on,
            Some(day(2027, 7, 4))
        );
        // "dec 25" this year.
        assert_eq!(
            parse("Wrap gifts dec 25", &ctx()).entry.deadline_on,
            Some(day(2026, 12, 25))
        );
    }

    #[test]
    fn iso_date_is_exact() {
        assert_eq!(
            parse("Submit taxes 2027-04-15", &ctx()).entry.deadline_on,
            Some(day(2027, 4, 15))
        );
    }

    #[test]
    fn start_cue_sets_start_on_not_deadline() {
        let e = parse("Draft plan starting monday", &ctx()).entry;
        assert_eq!(e.start_on, Some(day(2026, 7, 27)));
        assert!(e.deadline_on.is_none());
    }

    // ---- Vague / ambiguous: never invent a date ------------------------

    #[test]
    fn vague_word_yields_low_confidence_and_no_date() {
        let e = parse("Call the dentist sometime soon", &ctx()).entry;
        assert!(e.start_on.is_none() && e.deadline_on.is_none());
        assert!(e.reminder.is_none());
        assert!(e.confidence < LLM_FALLBACK_THRESHOLD);
        assert!(e.needs_llm_fallback());
    }

    #[test]
    fn remind_without_time_defers() {
        let e = parse("remind me friday", &ctx()).entry;
        assert_eq!(e.kind, Route::Reminder);
        assert!(e.reminder.is_none()); // no time invented
        assert!(e.needs_llm_fallback());
    }

    // ---- Recurrence variants -------------------------------------------

    #[test]
    fn every_monday_weekly_byday() {
        let e = parse("Standup every monday", &ctx()).entry;
        let rec = e.recurrence.expect("recurrence");
        assert_eq!(rec.rrule, "FREQ=WEEKLY;BYDAY=MO");
        assert_eq!(rec.mode, RecurrenceMode::Fixed);
    }

    #[test]
    fn every_mon_wed_fri_list() {
        let e = parse("Gym every mon, wed and fri", &ctx()).entry;
        let rec = e.recurrence.expect("recurrence");
        assert_eq!(rec.rrule, "FREQ=WEEKLY;BYDAY=MO,WE,FR");
    }

    #[test]
    fn every_other_week() {
        let e = parse("Payroll every other week", &ctx()).entry;
        assert_eq!(e.recurrence.unwrap().rrule, "FREQ=WEEKLY;INTERVAL=2");
    }

    #[test]
    fn every_weekend() {
        let e = parse("Relax every weekend", &ctx()).entry;
        assert_eq!(e.recurrence.unwrap().rrule, "FREQ=WEEKLY;BYDAY=SA,SU");
    }

    #[test]
    fn everyone_is_not_recurrence() {
        // "everyone" must not trigger the recurrence grammar.
        let e = parse("Email everyone about launch", &ctx()).entry;
        assert!(e.recurrence.is_none());
        assert!(e.reminder.is_none());
    }

    // ---- Time grammar --------------------------------------------------

    #[test]
    fn time_only_today_or_tomorrow_rollover() {
        // 9am already passed at 10:00 -> tomorrow.
        let e = parse("standup at 9am", &ctx()).entry;
        assert_eq!(e.kind, Route::Reminder);
        let fire = ctx()
            .now
            .offset()
            .with_ymd_and_hms(2026, 7, 24, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(e.reminder.unwrap().fire_at.as_millis(), fire);
    }

    #[test]
    fn time_later_today_stays_today() {
        let e = parse("sync 3:30pm", &ctx()).entry;
        let fire = ctx()
            .now
            .offset()
            .with_ymd_and_hms(2026, 7, 23, 15, 30, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(e.reminder.unwrap().fire_at.as_millis(), fire);
    }

    #[test]
    fn noon_resolves_to_1200() {
        let e = parse("lunch noon", &ctx()).entry;
        let hour = DateTime::from_timestamp_millis(e.reminder.unwrap().fire_at.as_millis())
            .unwrap()
            .with_timezone(ctx().now.offset())
            .hour();
        assert_eq!(hour, 12);
    }

    // ---- Tags / mentions / priority ------------------------------------

    #[test]
    fn first_hash_is_project_rest_are_tags() {
        let e = parse("todo Plan trip #Travel #summer #europe", &ctx()).entry;
        assert_eq!(e.project.as_deref(), Some("Travel"));
        assert_eq!(e.tags, vec!["summer".to_string(), "europe".to_string()]);
    }

    #[test]
    fn mention_becomes_assignee() {
        let e = parse("todo Ship release @alice", &ctx()).entry;
        assert_eq!(e.assignee.as_deref(), Some("alice"));
        assert_eq!(e.title, "Ship release");
    }

    #[test]
    fn priority_word_forms() {
        assert_eq!(parse("todo x !high", &ctx()).entry.priority, 1);
        assert_eq!(parse("todo x !p3", &ctx()).entry.priority, 3);
    }

    // ---- Highlight spans ------------------------------------------------

    #[test]
    fn highlights_cover_recognized_tokens_sorted() {
        let input = "todo Draft Q3 deck #Work !2 friday";
        let out = parse(input, &ctx());
        // Sorted ascending, and every span is a valid slice of the input.
        let mut last = 0;
        for s in &out.highlights {
            assert!(s.start >= last);
            assert!(s.end <= input.len());
            last = s.start;
        }
        let kinds: Vec<TokenKind> = out.highlights.iter().map(|s| s.token).collect();
        assert!(kinds.contains(&TokenKind::Keyword));
        assert!(kinds.contains(&TokenKind::Project));
        assert!(kinds.contains(&TokenKind::Priority));
        assert!(kinds.contains(&TokenKind::Date));
    }

    #[test]
    fn highlight_slice_matches_source_text() {
        let input = "Review PRs every weekday 9am";
        let out = parse(input, &ctx());
        let recur = out
            .highlights
            .iter()
            .find(|s| s.token == TokenKind::Recurrence)
            .unwrap();
        assert_eq!(&input[recur.start..recur.end], "every weekday");
    }

    // ---- Contract serialization ---------------------------------------

    #[test]
    fn parsed_entry_serializes_to_data_model_14_3_shape() {
        let e = parse("Pay rent every month on the 1st !1", &ctx()).entry;
        let v: serde_json::Value = serde_json::to_value(&e).unwrap();
        assert_eq!(v["schema"], "ParsedEntry");
        assert_eq!(v["kind"], "task");
        assert_eq!(v["recurrence"]["mode"], "fixed");
        assert_eq!(v["used_llm_fallback"], false);
        assert!(v["priority"].is_number());
    }

    #[test]
    fn multibyte_title_survives_masking() {
        let e = parse("todo Café résumé #Work", &ctx()).entry;
        assert_eq!(e.title, "Café résumé");
        assert_eq!(e.project.as_deref(), Some("Work"));
    }

    #[test]
    fn reference_day_is_thursday() {
        assert_eq!(ctx().today().weekday(), chrono::Weekday::Thu);
        let coming_fri = ctx().today() + Duration::days(1);
        assert_eq!(coming_fri, NaiveDate::from_ymd_opt(2026, 7, 24).unwrap());
    }
}

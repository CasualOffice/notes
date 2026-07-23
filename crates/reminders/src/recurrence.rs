//! `recurrence_rule` (Data Model §7.2) and **materialize-on-completion** advance.
//!
//! The store holds a *template* plus exactly one next materialized instance — never
//! a pre-expanded series. On completion the `rrule` crate computes the next date:
//!
//! - `mode = fixed` (`every`) advances from the **scheduled** date of the instance
//!   just completed;
//! - `mode = after_completion` (`every!`) advances from the **completion** date.
//!
//! The stored `rrule` string is the recurrence *pattern* (`FREQ`/`INTERVAL`/`BY…`);
//! series *bounds* live in the dedicated columns `until_on` and `count_remaining`
//! (Data Model §7.2), which this crate treats as authoritative rather than
//! re-deriving `COUNT`/`UNTIL` from the pattern each cycle.

use app_domain::{Day, RecurrenceRuleId};
use chrono::NaiveDate;
use rrule::RRuleSet;
use serde::{Deserialize, Serialize};

use crate::error::ReminderError;

/// How a recurrence advances on completion (`recurrence_rule.mode` CHECK).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecurrenceMode {
    /// Todoist `every`: advance from the instance's scheduled date.
    Fixed,
    /// Todoist `every!`: advance from the actual completion date.
    AfterCompletion,
}

impl RecurrenceMode {
    /// The exact string stored in `recurrence_rule.mode`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Fixed => "fixed",
            Self::AfterCompletion => "after_completion",
        }
    }

    /// Parse from the stored `recurrence_rule.mode` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "fixed" => Self::Fixed,
            "after_completion" => Self::AfterCompletion,
            _ => return None,
        })
    }
}

/// A `recurrence_rule` row (Data Model §7.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecurrenceRule {
    /// `entity_id` (UUIDv7, `kind='recurrence_rule'`).
    pub entity_id: RecurrenceRuleId,
    /// RFC-5545 RRULE pattern string (with or without a leading `RRULE:`).
    pub rrule: String,
    /// Advance mode.
    pub mode: RecurrenceMode,
    /// The single materialized next instance (`None` once the series terminates).
    pub next_scheduled_on: Option<Day>,
    /// Optional inclusive upper bound on the series.
    pub until_on: Option<Day>,
    /// Optional remaining-instance budget; decremented per completion.
    pub count_remaining: Option<u32>,
    /// Completed instance dates, in completion order.
    pub complete_instances: Vec<Day>,
}

/// The outcome of [`RecurrenceRule::advance`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Advance {
    /// The series produced a new materialized instance on this DAY.
    Next(Day),
    /// The series terminated (`count_remaining` exhausted, `until_on` passed, or
    /// the pattern yielded no further occurrence). `next_scheduled_on` is now `None`.
    Terminated,
}

impl RecurrenceRule {
    /// Advance the series after completing the instance scheduled on
    /// `completed_instance_on`, where `completion_date` is the actual wall-date the
    /// user marked it done.
    ///
    /// Mutates `self` in place: appends to `complete_instances`, decrements
    /// `count_remaining`, and sets `next_scheduled_on` to the new instance (or
    /// `None` on termination). Returns the [`Advance`] outcome.
    ///
    /// Materialize-on-completion (Data Model §7.2): the anchor is the scheduled
    /// date for [`RecurrenceMode::Fixed`] and the completion date for
    /// [`RecurrenceMode::AfterCompletion`].
    pub fn advance(
        &mut self,
        completed_instance_on: Day,
        completion_date: Day,
    ) -> Result<Advance, ReminderError> {
        self.complete_instances.push(completed_instance_on);

        // A count budget of 0 (or reaching 0 by completing this instance) ends it.
        if let Some(0) = self.count_remaining {
            return Ok(self.terminate());
        }
        let remaining_after = self.count_remaining.map(|c| c.saturating_sub(1));
        if remaining_after == Some(0) {
            self.count_remaining = remaining_after;
            return Ok(self.terminate());
        }

        let anchor = match self.mode {
            RecurrenceMode::Fixed => completed_instance_on.as_naive(),
            RecurrenceMode::AfterCompletion => completion_date.as_naive(),
        };

        let next = compute_next_after(&self.rrule, anchor)?;
        let Some(next) = next else {
            self.count_remaining = remaining_after;
            return Ok(self.terminate());
        };

        // Respect the dedicated series bound.
        if let Some(until) = self.until_on {
            if next > until.as_naive() {
                self.count_remaining = remaining_after;
                return Ok(self.terminate());
            }
        }

        self.count_remaining = remaining_after;
        let day = Day::from_naive(next);
        self.next_scheduled_on = Some(day);
        Ok(Advance::Next(day))
    }

    /// The next materialized instance date, if the series is live.
    #[must_use]
    pub fn next_instance(&self) -> Option<Day> {
        self.next_scheduled_on
    }

    fn terminate(&mut self) -> Advance {
        self.next_scheduled_on = None;
        Advance::Terminated
    }
}

/// Compute the first occurrence of `rrule_pattern` strictly after `anchor`.
///
/// Anchors `DTSTART` at `anchor` (00:00:00 UTC) and asks the `rrule` crate for the
/// next date. RFC-5545 always yields `DTSTART` as the first occurrence, so the
/// first date `> anchor` is the answer. Returns `None` when the pattern is
/// exhausted.
fn compute_next_after(
    rrule_pattern: &str,
    anchor: NaiveDate,
) -> Result<Option<NaiveDate>, ReminderError> {
    let dtstart = anchor.format("DTSTART:%Y%m%dT000000Z").to_string();
    let trimmed = rrule_pattern.trim();
    let rule_line = if trimmed.to_ascii_uppercase().starts_with("RRULE:") {
        trimmed.to_string()
    } else {
        format!("RRULE:{trimmed}")
    };
    let input = format!("{dtstart}\n{rule_line}");

    // The `Err` type is `rrule`'s parse error; we only need its `Display`, so it
    // is left to inference rather than named (keeps us off `rrule`'s error path).
    let set: RRuleSet = input
        .parse()
        .map_err(|e| ReminderError::Recurrence(format!("{e}")))?;

    // A handful of occurrences is always enough to clear the anchor for any
    // single-pattern rule (the next match is `dates[1]` in the common case).
    let result = set.all(16);
    for dt in result.dates {
        let d = dt.date_naive();
        if d > anchor {
            return Ok(Some(d));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::Id;

    fn day(s: &str) -> Day {
        Day::from_naive(NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap())
    }

    fn rule(pattern: &str, mode: RecurrenceMode) -> RecurrenceRule {
        RecurrenceRule {
            entity_id: Id::new(),
            rrule: pattern.to_string(),
            mode,
            next_scheduled_on: Some(day("2026-07-23")),
            until_on: None,
            count_remaining: None,
            complete_instances: Vec::new(),
        }
    }

    #[test]
    fn fixed_advances_from_scheduled_date() {
        let mut r = rule("FREQ=DAILY;INTERVAL=3", RecurrenceMode::Fixed);
        // Completed the 07-23 instance late, on 07-30. Fixed ignores completion date.
        let out = r.advance(day("2026-07-23"), day("2026-07-30")).unwrap();
        assert_eq!(out, Advance::Next(day("2026-07-26")));
        assert_eq!(r.next_scheduled_on, Some(day("2026-07-26")));
        assert_eq!(r.complete_instances, vec![day("2026-07-23")]);
    }

    #[test]
    fn after_completion_advances_from_completion_date() {
        let mut r = rule("FREQ=DAILY;INTERVAL=3", RecurrenceMode::AfterCompletion);
        // every! → 3 days after the completion date, not the scheduled date.
        let out = r.advance(day("2026-07-23"), day("2026-07-30")).unwrap();
        assert_eq!(out, Advance::Next(day("2026-08-02")));
    }

    #[test]
    fn weekly_byday_picks_next_weekday() {
        // 2026-07-23 is a Thursday; next MO/WE/FR after it is Friday 07-24.
        let mut r = rule("FREQ=WEEKLY;BYDAY=MO,WE,FR", RecurrenceMode::Fixed);
        let out = r.advance(day("2026-07-23"), day("2026-07-23")).unwrap();
        // Nearest configured weekday strictly after Thu 07-23 is Fri 07-24.
        assert_eq!(out, Advance::Next(day("2026-07-24")));
    }

    #[test]
    fn count_remaining_terminates_series() {
        let mut r = rule("FREQ=DAILY", RecurrenceMode::Fixed);
        r.count_remaining = Some(1);
        let out = r.advance(day("2026-07-23"), day("2026-07-23")).unwrap();
        assert_eq!(out, Advance::Terminated);
        assert_eq!(r.next_scheduled_on, None);
        assert_eq!(r.count_remaining, Some(0));
    }

    #[test]
    fn until_on_terminates_series() {
        let mut r = rule("FREQ=DAILY", RecurrenceMode::Fixed);
        r.until_on = Some(day("2026-07-23"));
        // Next daily instance (07-24) is past the bound → terminate.
        let out = r.advance(day("2026-07-23"), day("2026-07-23")).unwrap();
        assert_eq!(out, Advance::Terminated);
        assert_eq!(r.next_scheduled_on, None);
    }

    #[test]
    fn mode_and_string_roundtrip() {
        assert_eq!(
            RecurrenceMode::from_db_str("after_completion"),
            Some(RecurrenceMode::AfterCompletion)
        );
        assert_eq!(RecurrenceMode::Fixed.as_str(), "fixed");
        assert_eq!(RecurrenceMode::from_db_str("bogus"), None);
    }

    #[test]
    fn bad_rrule_is_a_recurrence_error() {
        let mut r = rule("FREQ=NONSENSE", RecurrenceMode::Fixed);
        let err = r.advance(day("2026-07-23"), day("2026-07-23")).unwrap_err();
        assert!(matches!(err, ReminderError::Recurrence(_)));
    }
}

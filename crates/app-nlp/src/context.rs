//! The resolution context a caller supplies for a parse. Implements the "resolve
//! relative dates against *now*" requirement of **Feature Specs §2.1** while keeping
//! [`parse`](crate::parse) **pure** (HLD §8.2): all clock/zone state is injected, so
//! the same input + context is deterministic and the live preview is cancellable.
//!
//! This crate depends only on `chrono` (no `chrono-tz` / IANA database), so the
//! caller resolves the user's current local time *and* its UTC offset — carried here
//! as a [`chrono::DateTime<FixedOffset>`] — plus the IANA zone *name* (a passthrough
//! string stored on [`ReminderSpec::tz`](crate::ReminderSpec)).

use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Weekday};

/// Everything the grammar needs to resolve relative dates/times deterministically.
#[derive(Clone, Debug)]
pub struct ParseContext {
    /// The user's *current* local time, including the active UTC offset. The offset
    /// is used to convert a resolved local wall-time into an absolute UTC instant
    /// ([`ReminderSpec::fire_at`](crate::ReminderSpec)); the date/time components are
    /// the "now" that `tomorrow`, `next friday`, `3pm`, … resolve against.
    pub now: DateTime<FixedOffset>,
    /// The origin IANA timezone *name* (e.g. `America/New_York`). Passed straight
    /// through to `ReminderSpec.tz`; never parsed here.
    pub tz: String,
    /// First day of the week. Default [`Weekday::Mon`]; affects only "this/next
    /// week" style phrases, not weekday names.
    pub week_start: Weekday,
}

impl ParseContext {
    /// Construct a context from the caller-resolved local `now` and IANA zone name.
    /// `week_start` defaults to Monday (override the field directly to change it).
    #[must_use]
    pub fn new(now: DateTime<FixedOffset>, tz: impl Into<String>) -> Self {
        Self {
            now,
            tz: tz.into(),
            week_start: Weekday::Mon,
        }
    }

    /// The local wall-date of `now`.
    #[must_use]
    pub fn today(&self) -> NaiveDate {
        self.now.date_naive()
    }

    /// The local wall clock of `now` as a naive datetime (for "is this in the
    /// past?" comparisons).
    #[must_use]
    pub fn now_naive(&self) -> NaiveDateTime {
        self.now.naive_local()
    }

    /// The weekday of `today`.
    #[must_use]
    pub fn today_weekday(&self) -> Weekday {
        self.today().weekday()
    }

    /// Convert a *local wall-clock* naive datetime into epoch-milliseconds UTC using
    /// `now`'s fixed offset. Never panics: a nonexistent/ambiguous local time (DST)
    /// cannot occur under a fixed offset, but the fallback treats the naive time as
    /// UTC rather than unwrapping.
    #[must_use]
    pub fn local_to_utc_ms(&self, local: NaiveDateTime) -> i64 {
        self.now
            .offset()
            .from_local_datetime(&local)
            .single()
            .map_or_else(
                || local.and_utc().timestamp_millis(),
                |dt| dt.timestamp_millis(),
            )
    }
}

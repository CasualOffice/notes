//! Time types. Implements Data Model §1 (Time).
//!
//! The store uses two representations:
//! - [`Timestamp`] — absolute instant, epoch-milliseconds UTC (`INTEGER`).
//! - [`Day`]       — calendar wall-date `YYYY-MM-DD`, no zone (`TEXT`), used for
//!   `daily_date`, `start_on`, `deadline_on`, `next_scheduled_on`.
//!
//! The workspace standardizes on `chrono` (not `time`) because the `rrule` crate
//! already pulls `chrono` into the tree; a second datetime stack would be waste.

use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// An absolute instant as epoch-milliseconds UTC. Persisted as SQLite `INTEGER`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// Current wall-clock instant.
    #[must_use]
    pub fn now() -> Self {
        Self(Utc::now().timestamp_millis())
    }

    /// Construct from raw epoch-milliseconds.
    #[must_use]
    pub const fn from_millis(ms: i64) -> Self {
        Self(ms)
    }

    /// Raw epoch-milliseconds.
    #[must_use]
    pub const fn as_millis(&self) -> i64 {
        self.0
    }
}

impl fmt::Debug for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Timestamp({}ms)", self.0)
    }
}

/// A calendar wall-date (`YYYY-MM-DD`, no timezone). Persisted as SQLite `TEXT`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Day(pub NaiveDate);

impl Day {
    /// Today's date in the given fixed offset is out of scope here; callers pass
    /// the local wall-date. This constructs from an explicit [`NaiveDate`].
    #[must_use]
    pub const fn from_naive(d: NaiveDate) -> Self {
        Self(d)
    }

    /// The underlying [`NaiveDate`].
    #[must_use]
    pub const fn as_naive(&self) -> NaiveDate {
        self.0
    }
}

impl fmt::Display for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // ISO-8601 calendar date, exactly the `TEXT` form stored in SQLite.
        write!(f, "{}", self.0.format("%Y-%m-%d"))
    }
}

impl fmt::Debug for Day {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Day({self})")
    }
}

impl FromStr for Day {
    type Err = chrono::ParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(NaiveDate::parse_from_str(s, "%Y-%m-%d")?))
    }
}

impl Serialize for Day {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Day {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_roundtrips_iso() {
        let d = Day::from_str("2026-07-23").unwrap();
        assert_eq!(d.to_string(), "2026-07-23");
        assert_eq!(serde_json::to_string(&d).unwrap(), "\"2026-07-23\"");
        let back: Day = serde_json::from_str("\"2026-07-23\"").unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn timestamp_is_transparent_integer() {
        let t = Timestamp::from_millis(1_700_000_000_000);
        assert_eq!(serde_json::to_string(&t).unwrap(), "1700000000000");
    }
}

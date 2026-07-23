//! Hybrid Logical Clock. Implements Data Model §1 (concurrency seam) and §11.2.
//!
//! Serialized form (in `entity.hlc`, `link.hlc`, `entity_op.hlc`, NDJSON journals)
//! is the sortable string `"<physical_ms>:<counter>:<node>"`. Ordering is
//! lexicographic-compatible: `(physical_ms, counter, node)`. The seam is *recorded*
//! in v1, not reconciled — `sync-core` consumes it later without a re-model.

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;

use crate::time::Timestamp;

/// A hybrid logical clock value.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Hlc {
    /// Physical component: epoch-milliseconds UTC.
    pub physical_ms: i64,
    /// Logical counter, breaking ties within the same millisecond.
    pub counter: u32,
    /// Node identifier (stable per install/replica).
    pub node: String,
}

impl Hlc {
    /// Seed a clock at the current wall time for `node`.
    #[must_use]
    pub fn now(node: impl Into<String>) -> Self {
        Self {
            physical_ms: Timestamp::now().as_millis(),
            counter: 0,
            node: node.into(),
        }
    }

    /// Construct explicitly.
    #[must_use]
    pub fn new(physical_ms: i64, counter: u32, node: impl Into<String>) -> Self {
        Self {
            physical_ms,
            counter,
            node: node.into(),
        }
    }

    /// Advance the clock for a local event observed at `wall_ms`.
    ///
    /// If wall time moved forward, adopt it and reset the counter; otherwise bump
    /// the counter to preserve monotonicity under a stalled/regressed clock.
    pub fn tick(&mut self, wall_ms: i64) {
        if wall_ms > self.physical_ms {
            self.physical_ms = wall_ms;
            self.counter = 0;
        } else {
            self.counter = self.counter.saturating_add(1);
        }
    }

    /// Advance using the current wall clock, returning the new value.
    pub fn tick_now(&mut self) -> Self {
        self.tick(Timestamp::now().as_millis());
        self.clone()
    }
}

impl Ord for Hlc {
    fn cmp(&self, other: &Self) -> Ordering {
        self.physical_ms
            .cmp(&other.physical_ms)
            .then(self.counter.cmp(&other.counter))
            .then_with(|| self.node.cmp(&other.node))
    }
}

impl PartialOrd for Hlc {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}:{}", self.physical_ms, self.counter, self.node)
    }
}

impl fmt::Debug for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Hlc({self})")
    }
}

/// Error parsing an [`Hlc`] from its `"<ms>:<counter>:<node>"` string form.
#[derive(Debug, thiserror::Error)]
pub enum HlcParseError {
    #[error("HLC must have form '<physical_ms>:<counter>:<node>'")]
    Shape,
    #[error("HLC physical_ms is not a valid integer")]
    PhysicalMs,
    #[error("HLC counter is not a valid integer")]
    Counter,
}

impl FromStr for Hlc {
    type Err = HlcParseError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Node ids never contain ':' (enforced at mint time); split at most twice.
        let mut parts = s.splitn(3, ':');
        let ms = parts.next().ok_or(HlcParseError::Shape)?;
        let counter = parts.next().ok_or(HlcParseError::Shape)?;
        let node = parts.next().ok_or(HlcParseError::Shape)?;
        if node.is_empty() {
            return Err(HlcParseError::Shape);
        }
        Ok(Self {
            physical_ms: ms.parse().map_err(|_| HlcParseError::PhysicalMs)?,
            counter: counter.parse().map_err(|_| HlcParseError::Counter)?,
            node: node.to_owned(),
        })
    }
}

impl Serialize for Hlc {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::from_str(&s).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrips_string_form() {
        let h = Hlc::new(1_726_000_000_123, 3, "nodeA");
        assert_eq!(h.to_string(), "1726000000123:3:nodeA");
        assert_eq!(Hlc::from_str("1726000000123:3:nodeA").unwrap(), h);
    }

    #[test]
    fn tick_monotonic_within_same_ms() {
        let mut h = Hlc::new(1000, 0, "n");
        h.tick(1000);
        assert_eq!(h.counter, 1);
        h.tick(2000);
        assert_eq!((h.physical_ms, h.counter), (2000, 0));
    }

    #[test]
    fn ordering_is_physical_then_counter() {
        let a = Hlc::new(1000, 0, "z");
        let b = Hlc::new(1000, 1, "a");
        assert!(a < b);
    }

    #[test]
    fn serde_is_string() {
        let h = Hlc::new(1, 2, "n");
        assert_eq!(serde_json::to_string(&h).unwrap(), "\"1:2:n\"");
    }
}

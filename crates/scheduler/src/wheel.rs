//! **Layer A** — the in-memory min-heap timer wheel keyed on `fire_at`
//! (Architecture §6, HLD §8.3). Rebuilt from SQLite on boot; owned by one Tokio
//! task (see [`crate::service`]). This module is the *pure, synchronous* core so
//! it is exhaustively testable under a simulated clock.
//!
//! Removal from a binary heap is O(n), so the wheel uses **lazy deletion**: an
//! authoritative `HashMap<Id, Armed>` holds each reminder's current fire time and
//! a monotonically bumped generation; heap slots carry the generation they were
//! pushed with and are discarded on pop if they no longer match (re-armed, snoozed,
//! or disarmed). A per-process `delivered` set makes a single armed instance fire
//! **exactly once** — the wheel's half of the cross-layer de-dup contract.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use app_domain::ReminderId;

/// The authoritative current arming of a reminder in the wheel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Armed {
    fire_at_ms: i64,
    generation: u64,
}

/// A heap slot. Ordered by `fire_at_ms` (min-heap via [`Reverse`]), then `id`/`gen`
/// for a total, deterministic order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Slot {
    fire_at_ms: i64,
    id: ReminderId,
    generation: u64,
}

impl PartialOrd for Slot {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Slot {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.fire_at_ms
            .cmp(&other.fire_at_ms)
            .then_with(|| self.id.cmp(&other.id))
            .then_with(|| self.generation.cmp(&other.generation))
    }
}

/// The Layer-A timer wheel.
#[derive(Debug, Default)]
pub struct TimerWheel {
    armed: HashMap<ReminderId, Armed>,
    heap: BinaryHeap<Reverse<Slot>>,
    next_generation: u64,
    /// Ids already delivered this process — the wheel's exactly-once guarantee.
    delivered: std::collections::HashSet<ReminderId>,
}

impl TimerWheel {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Arm (or re-arm) `id` to fire at `fire_at_ms`. Re-arming supersedes any prior
    /// slot (older generations become stale) and re-enables delivery — so a snoozed
    /// reminder re-armed for a new instant can fire again.
    pub fn arm(&mut self, id: ReminderId, fire_at_ms: i64) {
        let generation = self.next_generation;
        self.next_generation += 1;
        self.armed.insert(
            id,
            Armed {
                fire_at_ms,
                generation,
            },
        );
        self.heap.push(Reverse(Slot {
            fire_at_ms,
            id,
            generation,
        }));
        self.delivered.remove(&id);
    }

    /// Disarm `id` (edit/complete/cancel patched Layer A). The live heap slot is
    /// left to be discarded lazily on pop.
    pub fn disarm(&mut self, id: ReminderId) {
        self.armed.remove(&id);
        self.delivered.remove(&id);
    }

    /// Rebuild the wheel from SQLite on boot: clear everything and arm each active
    /// reminder at its effective fire instant.
    pub fn rebuild(&mut self, active: impl IntoIterator<Item = (ReminderId, i64)>) {
        self.armed.clear();
        self.heap.clear();
        self.delivered.clear();
        self.next_generation = 0;
        for (id, fire_at_ms) in active {
            self.arm(id, fire_at_ms);
        }
    }

    /// The earliest live (non-stale) fire instant, or `None` if the wheel is idle.
    /// Skips stale slots eagerly so callers can sleep on a real deadline.
    #[must_use]
    pub fn next_fire_at(&mut self) -> Option<i64> {
        while let Some(Reverse(slot)) = self.heap.peek().copied() {
            if self.is_live(&slot) {
                return Some(slot.fire_at_ms);
            }
            self.heap.pop();
        }
        None
    }

    /// Pop every reminder due at or before `now_ms`, in fire order, deduping so each
    /// armed instance is returned at most once. Returned ids are candidates for
    /// delivery; the caller still runs each through the [`crate::FireGate`] for
    /// cross-layer de-dup.
    #[must_use]
    pub fn pop_due(&mut self, now_ms: i64) -> Vec<ReminderId> {
        let mut fired = Vec::new();
        while let Some(Reverse(slot)) = self.heap.peek().copied() {
            if slot.fire_at_ms > now_ms {
                break;
            }
            self.heap.pop();
            if !self.is_live(&slot) {
                continue; // stale (re-armed / disarmed) — discard
            }
            if self.delivered.contains(&slot.id) {
                continue; // already delivered this instance
            }
            self.delivered.insert(slot.id);
            self.armed.remove(&slot.id);
            fired.push(slot.id);
        }
        fired
    }

    /// Number of distinct armed reminders.
    #[must_use]
    pub fn len(&self) -> usize {
        self.armed.len()
    }

    /// Whether nothing is armed.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.armed.is_empty()
    }

    /// A slot is live iff it matches the authoritative current generation for its id.
    fn is_live(&self, slot: &Slot) -> bool {
        matches!(self.armed.get(&slot.id), Some(a) if a.generation == slot.generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::Id;

    // ---- fire tolerance: due exactly at/after fire_at, never before ---------

    #[test]
    fn fires_at_or_after_deadline_not_before() {
        let mut w = TimerWheel::new();
        let a = Id::new();
        w.arm(a, 1_000);

        // one ms early → nothing due
        assert!(w.pop_due(999).is_empty());
        assert_eq!(w.next_fire_at(), Some(1_000));

        // exactly on time → fires (± well within the N7 1s tolerance)
        assert_eq!(w.pop_due(1_000), vec![a]);
        assert!(w.is_empty());
    }

    #[test]
    fn returns_due_in_fire_order() {
        let mut w = TimerWheel::new();
        let (a, b, c) = (Id::new(), Id::new(), Id::new());
        w.arm(b, 3_000);
        w.arm(a, 1_000);
        w.arm(c, 2_000);
        let due = w.pop_due(10_000);
        assert_eq!(due, vec![a, c, b]);
    }

    // ---- de-dup: an armed instance fires exactly once -----------------------

    #[test]
    fn same_instance_never_fires_twice() {
        let mut w = TimerWheel::new();
        let a = Id::new();
        w.arm(a, 1_000);
        assert_eq!(w.pop_due(5_000), vec![a]);
        // second sweep past the same deadline yields nothing
        assert!(w.pop_due(5_000).is_empty());
        assert!(w.pop_due(9_000).is_empty());
    }

    #[test]
    fn rearm_supersedes_stale_slot() {
        let mut w = TimerWheel::new();
        let a = Id::new();
        w.arm(a, 1_000);
        // snooze → re-arm for later; the old 1_000 slot must not fire
        w.arm(a, 10_000);
        assert!(w.pop_due(5_000).is_empty(), "stale early slot suppressed");
        assert_eq!(w.next_fire_at(), Some(10_000));
        assert_eq!(w.pop_due(10_000), vec![a]);
    }

    #[test]
    fn disarm_prevents_fire() {
        let mut w = TimerWheel::new();
        let a = Id::new();
        w.arm(a, 1_000);
        w.disarm(a);
        assert!(w.pop_due(5_000).is_empty());
        assert_eq!(w.next_fire_at(), None);
        assert!(w.is_empty());
    }

    #[test]
    fn rearm_after_delivery_allows_second_fire() {
        // snooze-after-fire: same id delivered, then armed again for a new instant.
        let mut w = TimerWheel::new();
        let a = Id::new();
        w.arm(a, 1_000);
        assert_eq!(w.pop_due(1_000), vec![a]);
        w.arm(a, 2_000); // re-arm clears the delivered flag
        assert_eq!(w.pop_due(2_000), vec![a]);
    }

    // ---- rebuild-from-boot --------------------------------------------------

    #[test]
    fn rebuild_replaces_contents() {
        let mut w = TimerWheel::new();
        w.arm(Id::new(), 500);
        let a = Id::new();
        let b = Id::new();
        w.rebuild([(a, 1_000), (b, 2_000)]);
        assert_eq!(w.len(), 2);
        assert_eq!(w.pop_due(2_000), vec![a, b]);
    }
}

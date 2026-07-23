//! The state-gated de-dup seam. De-dup is **state-gated, not timer-gated** (HLD
//! §8.3): whichever layer fires first flips `pending|snoozed → fired`; the other
//! (and any duplicate timer) no-ops. The authority is `reminder.state` in SQLite —
//! the single logical DB writer — so this crate only defines the trait and an
//! in-memory reference implementation for tests.

use std::collections::HashMap;
use std::sync::Mutex;

use app_domain::ReminderId;
use reminders::ReminderState;

use crate::error::SchedulerError;

/// The atomic "who delivers this reminder?" arbiter.
///
/// Implemented over the SQLite writer in production (a single conditional
/// `UPDATE ... WHERE state IN ('pending','snoozed')`), so exactly one caller sees
/// `Ok(true)` across both scheduler layers.
pub trait FireGate: Send + Sync {
    /// Atomically claim delivery of `id`: flip `pending|snoozed → fired` iff the
    /// reminder is currently active. Returns `true` if *this* call performed the
    /// flip (the caller now owns delivery), `false` if it was already claimed /
    /// no longer deliverable (a de-dup no-op).
    fn try_claim(&self, id: ReminderId) -> Result<bool, SchedulerError>;
}

/// An in-memory [`FireGate`] backing a `HashMap<ReminderId, ReminderState>` — used
/// as the reference de-dup implementation in tests and as a fallback in
/// integration harnesses that have no DB yet.
#[derive(Debug, Default)]
pub struct InMemoryGate {
    states: Mutex<HashMap<ReminderId, ReminderState>>,
}

impl InMemoryGate {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Seed / overwrite the known state of a reminder.
    pub fn set_state(&self, id: ReminderId, state: ReminderState) {
        self.states
            .lock()
            .expect("gate mutex poisoned")
            .insert(id, state);
    }

    /// Read back the current state (test convenience).
    #[must_use]
    pub fn state(&self, id: ReminderId) -> Option<ReminderState> {
        self.states
            .lock()
            .expect("gate mutex poisoned")
            .get(&id)
            .copied()
    }
}

impl FireGate for InMemoryGate {
    fn try_claim(&self, id: ReminderId) -> Result<bool, SchedulerError> {
        let mut states = self.states.lock().expect("gate mutex poisoned");
        match states.get(&id).copied() {
            Some(s) if s.is_active() => {
                states.insert(id, ReminderState::Fired);
                Ok(true)
            }
            // Unknown ids are treated as not-deliverable rather than invented.
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::Id;

    #[test]
    fn first_claim_wins_second_noops() {
        let gate = InMemoryGate::new();
        let id = Id::new();
        gate.set_state(id, ReminderState::Pending);

        assert!(gate.try_claim(id).unwrap(), "first claim owns delivery");
        assert!(!gate.try_claim(id).unwrap(), "second claim de-dups");
        assert_eq!(gate.state(id), Some(ReminderState::Fired));
    }

    #[test]
    fn snoozed_is_claimable_unknown_is_not() {
        let gate = InMemoryGate::new();
        let snoozed = Id::new();
        gate.set_state(snoozed, ReminderState::Snoozed);
        assert!(gate.try_claim(snoozed).unwrap());

        let unknown = Id::new();
        assert!(!gate.try_claim(unknown).unwrap());
    }

    #[test]
    fn terminal_states_are_not_claimable() {
        let gate = InMemoryGate::new();
        let id = Id::new();
        gate.set_state(id, ReminderState::Canceled);
        assert!(!gate.try_claim(id).unwrap());
    }
}

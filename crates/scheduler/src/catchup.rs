//! The missed-reminder **catch-up sweep** (Architecture §6, HLD §8.3). Runs on
//! launch *and* on wake-from-sleep: any reminder that was due while the app was
//! closed (`state='pending' AND fire_at < now`) is coalesced into **one grouped
//! notification**, its rows flipped to `missed`, and surfaced in the in-app inbox.
//!
//! Pure logic: it takes a snapshot of active reminders + `now` and returns the set
//! to sweep. The caller performs the SQLite `UPDATE ... SET state='missed'` and
//! emits `ReminderMissedSwept` + one `ReminderFired{grouped:true}`.

use app_domain::ReminderId;
use reminders::Reminder;

/// The result of a catch-up sweep.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CatchUp {
    /// Reminders that were due before `now` and are to be marked `missed`, in
    /// fire order (earliest first).
    pub missed: Vec<ReminderId>,
}

impl CatchUp {
    /// Whether the sweep found anything (drives whether a grouped notification and
    /// [`app_domain::AppEvent::ReminderMissedSwept`] are emitted).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.missed.is_empty()
    }

    /// How many reminders were swept (the grouped-notification count).
    #[must_use]
    pub fn len(&self) -> usize {
        self.missed.len()
    }
}

/// Sweep `reminders` for anything due strictly before `now_ms`.
///
/// A reminder is missed when it is *active* (`pending`/`snoozed`) and its effective
/// fire instant already passed. Snoozed reminders whose `snoozed_until` elapsed
/// while closed are swept too — they were owed a fire. Results are sorted by fire
/// instant so the grouped notification reads oldest-first.
#[must_use]
pub fn sweep<'a>(reminders: impl IntoIterator<Item = &'a Reminder>, now_ms: i64) -> CatchUp {
    let mut due: Vec<(i64, ReminderId)> = reminders
        .into_iter()
        .filter(|r| r.state.is_active())
        .filter_map(|r| {
            let fire = r.effective_fire_at().as_millis();
            (fire < now_ms).then_some((fire, r.entity_id))
        })
        .collect();
    due.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    CatchUp {
        missed: due.into_iter().map(|(_, id)| id).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::Timestamp;
    use reminders::ReminderState;

    fn rem(fire_ms: i64, state: ReminderState) -> Reminder {
        let mut r = Reminder::new(Timestamp::from_millis(fire_ms), "UTC");
        r.state = state;
        r
    }

    #[test]
    fn sweeps_only_past_due_active_reminders() {
        let past1 = rem(1_000, ReminderState::Pending);
        let past2 = rem(500, ReminderState::Pending);
        let future = rem(9_000, ReminderState::Pending);
        let done = rem(1_000, ReminderState::Fired); // already delivered, not swept
        let list = [&past1, &past2, &future, &done];

        let out = sweep(list.iter().copied(), 5_000);
        // ordered oldest-first
        assert_eq!(out.missed, vec![past2.entity_id, past1.entity_id]);
        assert_eq!(out.len(), 2);
        assert!(!out.is_empty());
    }

    #[test]
    fn snoozed_past_snooze_is_swept_on_effective_instant() {
        let mut r = rem(1_000, ReminderState::Pending);
        r.snooze(Timestamp::from_millis(2_000)).unwrap();
        // snoozed_until (2_000) < now (5_000) → owed a fire, sweep it
        let out = sweep([&r], 5_000);
        assert_eq!(out.missed, vec![r.entity_id]);

        // but not yet past the snooze deadline → nothing
        let out2 = sweep([&r], 1_500);
        assert!(out2.is_empty());
    }

    #[test]
    fn empty_when_nothing_past_due() {
        let future = rem(9_000, ReminderState::Pending);
        assert!(sweep([&future], 1_000).is_empty());
    }
}

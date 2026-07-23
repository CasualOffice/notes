//! Layer-B 14-day **horizon reconcile** helpers (Architecture §6.1 invariant 2:
//! "the 14-day horizon is re-swept on every launch and wake so Layer B never
//! drifts"). Pure selection logic over a backend; the backend does the OS calls.

use crate::backend::OsNotificationBackend;
use crate::capability::ScheduledReminder;
use crate::error::SchedulerError;

const MS_PER_DAY: i64 = 86_400_000;

/// Whether `fire_at_ms` falls inside the backend's horizon measured from `now_ms`.
/// A `RunningOnly` (Linux) capability has a zero-day horizon → always `false`.
#[must_use]
pub fn within_horizon(fire_at_ms: i64, now_ms: i64, horizon_days: u16) -> bool {
    if horizon_days == 0 {
        return false;
    }
    let end = now_ms + i64::from(horizon_days) * MS_PER_DAY;
    fire_at_ms >= now_ms && fire_at_ms <= end
}

/// The subset of `active` reminders that should hold an OS one-shot right now: due
/// in the future and within the backend's horizon.
#[must_use]
pub fn horizon_subset(
    active: &[ScheduledReminder],
    now_ms: i64,
    horizon_days: u16,
) -> Vec<&ScheduledReminder> {
    active
        .iter()
        .filter(|r| within_horizon(r.fire_at.as_millis(), now_ms, horizon_days))
        .collect()
}

/// Re-sync Layer B to the current active set (launch / wake). On a `RunningOnly`
/// platform this is a no-op (honest: there is no OS layer). Otherwise it hands the
/// backend exactly the reminders within its horizon to (re)register.
pub fn reconcile(
    backend: &dyn OsNotificationBackend,
    active: &[ScheduledReminder],
    now_ms: i64,
) -> Result<(), SchedulerError> {
    let cap = backend.capability();
    let horizon = cap.horizon_days();
    if horizon == 0 {
        return Ok(());
    }
    let subset: Vec<ScheduledReminder> = horizon_subset(active, now_ms, horizon)
        .into_iter()
        .cloned()
        .collect();
    backend.reconcile(&subset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::LinuxBackend;
    use app_domain::{Id, Timestamp};

    fn sched(fire_ms: i64) -> ScheduledReminder {
        ScheduledReminder {
            reminder_id: Id::new(),
            fire_at: Timestamp::from_millis(fire_ms),
            tz: "UTC".into(),
            body: None,
            target: None,
        }
    }

    #[test]
    fn within_horizon_bounds() {
        let now = 0;
        assert!(within_horizon(MS_PER_DAY, now, 14));
        assert!(within_horizon(14 * MS_PER_DAY, now, 14));
        assert!(!within_horizon(15 * MS_PER_DAY, now, 14)); // beyond horizon
        assert!(!within_horizon(-1, now, 14)); // in the past
        assert!(!within_horizon(MS_PER_DAY, now, 0)); // no OS layer
    }

    #[test]
    fn subset_filters_to_horizon() {
        let a = sched(MS_PER_DAY);
        let b = sched(30 * MS_PER_DAY);
        let list = vec![a.clone(), b];
        let sub = horizon_subset(&list, 0, 14);
        assert_eq!(sub.len(), 1);
        assert_eq!(sub[0].reminder_id, a.reminder_id);
    }

    #[test]
    fn linux_reconcile_is_noop_ok() {
        let list = vec![sched(MS_PER_DAY)];
        assert!(reconcile(&LinuxBackend, &list, 0).is_ok());
    }
}

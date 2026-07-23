//! Status / completion transition helpers (Feature Specs §3.5 "Logbook &
//! completion"). Pure functions that compute the `(status, completed_at)` a
//! mutation should persist — they never touch the DB (the op-log write and the
//! `link(rel='reminds')`/recurrence side effects belong to `app-service` /
//! `storage`). Completing and canceling both record a completion instant and
//! move the task to the Logbook; canceling is deliberately distinct from
//! completing (Feature Specs §3.5). Recurrence materialisation of the *next*
//! instance is out of scope here (owned by the `reminders` crate, Data Model §7).

use app_domain::Timestamp;

use crate::domain::{ProjectStatus, TaskStatus};
use crate::error::TaskError;

/// The pair of fields a status change writes back to a `task` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskStatusChange {
    /// The new `task.status`.
    pub status: TaskStatus,
    /// The new `task.completed_at` (set when closing, cleared when reopening).
    pub completed_at: Option<Timestamp>,
}

/// Complete a task: `status='completed'`, `completed_at = now` (Feature Specs §3.5).
#[must_use]
pub fn complete_task(now: Timestamp) -> TaskStatusChange {
    TaskStatusChange {
        status: TaskStatus::Completed,
        completed_at: Some(now),
    }
}

/// Cancel a task: `status='canceled'`, `completed_at = now`. Distinct from
/// completion but also records the closing instant (Feature Specs §3.5).
#[must_use]
pub fn cancel_task(now: Timestamp) -> TaskStatusChange {
    TaskStatusChange {
        status: TaskStatus::Canceled,
        completed_at: Some(now),
    }
}

/// Reopen a task from the Logbook: `status='open'`, `completed_at = NULL`.
#[must_use]
pub fn reopen_task() -> TaskStatusChange {
    TaskStatusChange {
        status: TaskStatus::Open,
        completed_at: None,
    }
}

/// Validate and compute a task transition from `from` to `to`.
///
/// All transitions between the three statuses are permitted (a Logbook task may
/// be reopened; a completed task may be re-marked canceled), but a **no-op**
/// transition to the same status is rejected so callers do not append an empty
/// op to the log.
///
/// # Errors
/// [`TaskError::IllegalTransition`] if `from == to`.
pub fn task_transition(
    from: TaskStatus,
    to: TaskStatus,
    now: Timestamp,
) -> Result<TaskStatusChange, TaskError> {
    if from == to {
        return Err(TaskError::IllegalTransition(format!(
            "task already in status {:?}",
            from.as_str()
        )));
    }
    Ok(match to {
        TaskStatus::Completed => complete_task(now),
        TaskStatus::Canceled => cancel_task(now),
        TaskStatus::Open => reopen_task(),
    })
}

/// The pair of fields a status change writes back to a `project` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectStatusChange {
    /// The new `project.status`.
    pub status: ProjectStatus,
    /// The new `project.completed_at`.
    pub completed_at: Option<Timestamp>,
}

/// Validate and compute a project transition (Data Model §6.2). Closing sets
/// `completed_at`; reactivating clears it. Rejects a no-op.
///
/// # Errors
/// [`TaskError::IllegalTransition`] if `from == to`.
pub fn project_transition(
    from: ProjectStatus,
    to: ProjectStatus,
    now: Timestamp,
) -> Result<ProjectStatusChange, TaskError> {
    if from == to {
        return Err(TaskError::IllegalTransition(format!(
            "project already in status {:?}",
            from.as_str()
        )));
    }
    Ok(ProjectStatusChange {
        status: to,
        completed_at: if to.is_closed() { Some(now) } else { None },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn complete_and_cancel_record_time() {
        let now = Timestamp::from_millis(1_700_000_000_000);
        let c = complete_task(now);
        assert_eq!(c.status, TaskStatus::Completed);
        assert_eq!(c.completed_at, Some(now));

        let x = cancel_task(now);
        assert_eq!(x.status, TaskStatus::Canceled);
        assert_eq!(x.completed_at, Some(now));
    }

    #[test]
    fn reopen_clears_time() {
        let r = reopen_task();
        assert_eq!(r.status, TaskStatus::Open);
        assert_eq!(r.completed_at, None);
    }

    #[test]
    fn transition_rejects_noop_allows_others() {
        let now = Timestamp::now();
        assert!(task_transition(TaskStatus::Open, TaskStatus::Open, now).is_err());

        let done = task_transition(TaskStatus::Open, TaskStatus::Completed, now).unwrap();
        assert_eq!(done.status, TaskStatus::Completed);
        assert!(done.completed_at.is_some());

        // Logbook → reopen is allowed and clears completion.
        let re = task_transition(TaskStatus::Completed, TaskStatus::Open, now).unwrap();
        assert_eq!(re.status, TaskStatus::Open);
        assert_eq!(re.completed_at, None);
    }

    #[test]
    fn project_transition_sets_and_clears_time() {
        let now = Timestamp::now();
        let done =
            project_transition(ProjectStatus::Active, ProjectStatus::Completed, now).unwrap();
        assert_eq!(done.status, ProjectStatus::Completed);
        assert!(done.completed_at.is_some());

        let reopened =
            project_transition(ProjectStatus::Completed, ProjectStatus::Active, now).unwrap();
        assert_eq!(reopened.completed_at, None);

        assert!(project_transition(ProjectStatus::Active, ProjectStatus::Active, now).is_err());
    }
}

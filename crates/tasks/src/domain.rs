//! Domain types for the planning pillar. Faithful in-memory mirrors of the
//! **Data Model §6** detail tables — one struct per table, columns and nothing
//! more. Spine-owned fields (`title`, `created_at`, `updated_at`, `hlc`,
//! `deleted_at`) live on the `entity` row (Data Model §3.2), never on a detail
//! struct, so they are deliberately absent here.
//!
//! Tables implemented: `area` (§6.1), `project` (§6.2), `task` (§6.3),
//! `heading` (§6.4), `checklist_item` (§6.5).

use app_domain::{AreaId, Day, Id, PersonId, ProjectId, RecurrenceRuleId, TaskId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::error::TaskError;

/// A `heading.id` — a plain UUIDv7, **not** a spine entity (Data Model §6.4).
///
/// (app-domain has no dedicated alias for this; it exposes only the generic
/// [`Id`]. Recorded as an integration note rather than mutating app-domain.)
pub type HeadingId = Id;

/// A `checklist_item.id` — a plain UUIDv7, not a spine entity (Data Model §6.5).
pub type ChecklistItemId = Id;

// ---------------------------------------------------------------------------
// Status enums
// ---------------------------------------------------------------------------

/// `task.status` (Data Model §6.3 CHECK-equivalent: `open|completed|canceled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Actionable; participates in the live buckets.
    Open,
    /// Finished; lives in the Logbook (Feature Specs §3.5).
    Completed,
    /// Abandoned; distinct from completed, also in the Logbook.
    Canceled,
}

impl TaskStatus {
    /// The exact string stored in `task.status`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
        }
    }

    /// Parse from the stored `task.status` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "open" => Self::Open,
            "completed" => Self::Completed,
            "canceled" => Self::Canceled,
            _ => return None,
        })
    }

    /// Whether a task in this status belongs to the Logbook (Feature Specs §3.1).
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Canceled)
    }
}

/// `project.status` (Data Model §6.2: `active|completed|canceled`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectStatus {
    /// In progress.
    Active,
    /// Finished.
    Completed,
    /// Abandoned.
    Canceled,
}

impl ProjectStatus {
    /// The exact string stored in `project.status`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Completed => "completed",
            Self::Canceled => "canceled",
        }
    }

    /// Parse from the stored `project.status` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "active" => Self::Active,
            "completed" => Self::Completed,
            "canceled" => Self::Canceled,
            _ => return None,
        })
    }

    /// Whether the project is finished or abandoned.
    #[must_use]
    pub const fn is_closed(self) -> bool {
        matches!(self, Self::Completed | Self::Canceled)
    }
}

/// A typed view over `task.priority` (Data Model §6.3: `INTEGER 0..3`, from an
/// inline `!priority`). The struct field stays a raw [`u8`] to mirror the column
/// exactly; this enum is the ergonomic, range-checked projection of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    /// `0` — no priority.
    None,
    /// `1` — low.
    Low,
    /// `2` — medium.
    Medium,
    /// `3` — high.
    High,
}

impl Priority {
    /// The stored integer `0..=3`.
    #[must_use]
    pub const fn as_i64(self) -> i64 {
        match self {
            Self::None => 0,
            Self::Low => 1,
            Self::Medium => 2,
            Self::High => 3,
        }
    }

    /// Parse from a stored integer; anything outside `0..=3` is a validation error.
    pub fn from_i64(v: i64) -> Result<Self, TaskError> {
        Ok(match v {
            0 => Self::None,
            1 => Self::Low,
            2 => Self::Medium,
            3 => Self::High,
            other => {
                return Err(TaskError::InvalidField(format!(
                    "priority {other} out of range 0..=3"
                )))
            }
        })
    }
}

// ---------------------------------------------------------------------------
// Detail-table structs
// ---------------------------------------------------------------------------

/// `area` (Data Model §6.1) — a top-level life bucket ("Work", "Home").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Area {
    /// `area.entity_id` (spine `kind='area'`).
    pub entity_id: AreaId,
    /// `area.order_key` — fractional index for sibling ordering.
    pub order_key: String,
    /// `area.icon` — optional glyph.
    pub icon: Option<String>,
}

/// `project` (Data Model §6.2) — belongs to an area, may itself be dated,
/// may back a note.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// `project.entity_id` (spine `kind='project'`).
    pub entity_id: ProjectId,
    /// `project.area_id` — `kind='area'`; `None` = loose project.
    pub area_id: Option<AreaId>,
    /// `project.note_id` — optional backing note (`kind='note'`).
    pub note_id: Option<Id>,
    /// `project.status`.
    pub status: ProjectStatus,
    /// `project.start_on` — DAY (projects may be dated).
    pub start_on: Option<Day>,
    /// `project.deadline_on` — DAY.
    pub deadline_on: Option<Day>,
    /// `project.completed_at` — TS set when the project closes.
    pub completed_at: Option<Timestamp>,
    /// `project.order_key`.
    pub order_key: String,
}

/// `task` (Data Model §6.3) — the atomic actionable unit.
///
/// The three date/alert concepts are **never** conflated (Data Model §6.3):
/// - [`start_on`](Self::start_on) is the *When* — it **hides** the task until
///   that day;
/// - [`deadline_on`](Self::deadline_on) is the *due* — it shows a badge but
///   **never hides**;
/// - the alert time is a separate `reminder` row (Data Model §7), not on `task`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    /// `task.entity_id` (spine `kind='task'`).
    pub entity_id: TaskId,
    /// `task.project_id` — `kind='project'`.
    pub project_id: Option<ProjectId>,
    /// `task.area_id` — `kind='area'` (a loose task in an area).
    pub area_id: Option<AreaId>,
    /// `task.heading_id` — a section within a project (`heading.id`).
    pub heading_id: Option<HeadingId>,
    /// `task.parent_task_id` — a nested subtask's parent (`kind='task'`).
    pub parent_task_id: Option<TaskId>,
    /// `task.notes_md` — lightweight task body (markdown).
    pub notes_md: Option<String>,
    /// `task.status`.
    pub status: TaskStatus,
    /// `task.priority` — raw `0..=3` (mirror of the column). Use
    /// [`Task::priority_level`] for the typed view.
    pub priority: u8,
    /// `task.someday` — `true` = the Someday bucket (deferred; hidden from
    /// Today/Upcoming/Anytime until activated).
    pub someday: bool,
    /// `task.start_on` — DAY: **When/scheduled**, hides until this date.
    pub start_on: Option<Day>,
    /// `task.deadline_on` — DAY: **due**, does not hide.
    pub deadline_on: Option<Day>,
    /// `task.completed_at` — TS set when the task closes.
    pub completed_at: Option<Timestamp>,
    /// `task.order_key` — fractional index for O(1) drag-reorder.
    pub order_key: String,
    /// `task.assignee_person_id` — `kind='person'`; owner carried from a
    /// promoted action item (only if extracted from evidence).
    pub assignee_person_id: Option<PersonId>,
    /// `task.recurrence_id` — `kind='recurrence_rule'` (template task).
    pub recurrence_id: Option<RecurrenceRuleId>,
}

impl Task {
    /// The typed priority view; errors if the raw column is out of `0..=3`.
    pub fn priority_level(&self) -> Result<Priority, TaskError> {
        Priority::from_i64(i64::from(self.priority))
    }

    /// Validate the structured fields that carry an in-range invariant.
    /// (Date/foreign-key validity is enforced storage-side at persist time.)
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.priority > 3 {
            return Err(TaskError::InvalidField(format!(
                "priority {} out of range 0..=3",
                self.priority
            )));
        }
        crate::order_key::validate_key(&self.order_key)?;
        Ok(())
    }
}

/// `heading` (Data Model §6.4) — a lightweight in-project section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Heading {
    /// `heading.id` (UUIDv7, not a spine entity).
    pub id: HeadingId,
    /// `heading.project_id` — owning project (`kind='project'`).
    pub project_id: ProjectId,
    /// `heading.title`.
    pub title: String,
    /// `heading.order_key`.
    pub order_key: String,
}

/// `checklist_item` (Data Model §6.5) — a flat, ordered, in-task step, distinct
/// from a nested subtask (which is a full `task` with its own `parent_task_id`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChecklistItem {
    /// `checklist_item.id` (UUIDv7, not a spine entity).
    pub id: ChecklistItemId,
    /// `checklist_item.task_id` — owning task (`kind='task'`).
    pub task_id: TaskId,
    /// `checklist_item.text`.
    pub text: String,
    /// `checklist_item.checked`.
    pub checked: bool,
    /// `checklist_item.order_key`.
    pub order_key: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_status_roundtrips() {
        for s in [
            TaskStatus::Open,
            TaskStatus::Completed,
            TaskStatus::Canceled,
        ] {
            assert_eq!(TaskStatus::from_db_str(s.as_str()), Some(s));
        }
        assert_eq!(TaskStatus::from_db_str("bogus"), None);
        assert!(TaskStatus::Completed.is_closed());
        assert!(!TaskStatus::Open.is_closed());
    }

    #[test]
    fn project_status_roundtrips() {
        for s in [
            ProjectStatus::Active,
            ProjectStatus::Completed,
            ProjectStatus::Canceled,
        ] {
            assert_eq!(ProjectStatus::from_db_str(s.as_str()), Some(s));
        }
        assert_eq!(ProjectStatus::from_db_str("open"), None); // task word, not a project status
    }

    #[test]
    fn priority_range_checked() {
        for (v, p) in [
            (0, Priority::None),
            (1, Priority::Low),
            (2, Priority::Medium),
            (3, Priority::High),
        ] {
            assert_eq!(Priority::from_i64(v).unwrap(), p);
            assert_eq!(p.as_i64(), v);
        }
        assert!(Priority::from_i64(4).is_err());
        assert!(Priority::from_i64(-1).is_err());
    }
}

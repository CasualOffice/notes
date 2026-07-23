//! Derived-bucket query compilation. **Buckets are queries over fields, never
//! stored states** (Data Model §6.3, Feature Specs §3.1).
//!
//! This module returns the exact SQL for each bucket plus an in-memory
//! classifier that mirrors it bit-for-bit (used for reactive membership updates —
//! Feature Specs AC-3.5 — and as the test oracle for the SQL). It holds **no**
//! database handle: the `storage` crate prepares the returned text against the
//! SQLCipher connection (the WebView never sees SQL — CLAUDE.md invariant).
//!
//! ## The five buckets
//!
//! All queries read the live task join `task t JOIN entity e ON e.id =
//! t.entity_id` and filter `e.deleted_at IS NULL` (the `deleted_at` flag lives on
//! the spine `entity` row, Data Model §3.1 — `task` has no such column). One
//! bound parameter, `:today`, is the caller's local wall-date as a `YYYY-MM-DD`
//! string; because `DAY` values are ISO-8601 text, `<=`/`>` string comparison is
//! exactly chronological.
//!
//! The predicates realise the Things-style **mutually-exclusive partition** of
//! open tasks (each open, non-someday task lands in exactly one of
//! Today/Upcoming/Anytime), satisfying AC-3.1 (a future `start_on` is *only* in
//! Upcoming) and AC-3.2 (a `deadline_on = today` shows in Today, overriding
//! Anytime):
//!
//! | Bucket | Predicate (with `e.deleted_at IS NULL`) | Doc |
//! |---|---|---|
//! | **Today** | `status='open' AND someday=0 AND (start_on <= :today OR deadline_on <= :today)` | DM §6.3 / FS §3.1 |
//! | **Upcoming** | `status='open' AND someday=0 AND (start_on IS NULL OR start_on > :today) AND (deadline_on IS NULL OR deadline_on > :today) AND (start_on > :today OR deadline_on > :today)` | FS §3.1 |
//! | **Anytime** | `status='open' AND someday=0 AND start_on IS NULL AND deadline_on IS NULL` | DM §6.3 / FS §3.1 |
//! | **Someday** | `status='open' AND someday=1` | DM §6.3 / FS §3.1 |
//! | **Logbook** | `status IN ('completed','canceled')` | FS §3.1 |
//!
//! The Upcoming guard uses explicit `IS NULL` arms rather than `NOT (…)` so that
//! SQL three-valued logic (a `NULL` date is *absent*, not *unknown-excluded*)
//! keeps a start-only or deadline-only future task in the bucket.
//!
//! `start_on` **hides** (When); `deadline_on` shows a badge but never hides; the
//! alert time is a separate `reminder` — never conflated (Data Model §6.3).

use app_domain::{Bucket, Day};

use crate::domain::Task;

/// The `task`-detail + spine columns every bucket query projects, in a fixed
/// order so `storage` can map rows positionally. `t` = `task`, `e` = `entity`.
pub const TASK_SELECT_COLUMNS: &str = "\
t.entity_id, e.title, t.project_id, t.area_id, t.heading_id, t.parent_task_id, \
t.notes_md, t.status, t.priority, t.someday, t.start_on, t.deadline_on, \
t.completed_at, t.order_key, t.assignee_person_id, t.recurrence_id, \
e.created_at, e.updated_at";

/// The live-task FROM/JOIN clause shared by every bucket.
pub const TASK_FROM: &str = "FROM task t JOIN entity e ON e.id = t.entity_id";

/// The base liveness filter (soft-delete lives on the spine row).
pub const LIVE_FILTER: &str = "e.deleted_at IS NULL";

// --- per-bucket WHERE fragments (excluding the shared LIVE_FILTER) ----------

/// Today predicate (Data Model §6.3 / Feature Specs §3.1).
pub const TODAY_WHERE: &str =
    "t.status = 'open' AND t.someday = 0 AND (t.start_on <= :today OR t.deadline_on <= :today)";

/// Upcoming predicate (Feature Specs §3.1) — future-dated, not yet in Today.
pub const UPCOMING_WHERE: &str = "t.status = 'open' AND t.someday = 0 \
AND (t.start_on IS NULL OR t.start_on > :today) \
AND (t.deadline_on IS NULL OR t.deadline_on > :today) \
AND (t.start_on > :today OR t.deadline_on > :today)";

/// Anytime predicate (Data Model §6.3 / Feature Specs §3.1) — actionable, undated.
pub const ANYTIME_WHERE: &str =
    "t.status = 'open' AND t.someday = 0 AND t.start_on IS NULL AND t.deadline_on IS NULL";

/// Someday predicate (Data Model §6.3 / Feature Specs §3.1) — deferred.
pub const SOMEDAY_WHERE: &str = "t.status = 'open' AND t.someday = 1";

/// Logbook predicate (Feature Specs §3.1) — completed or canceled.
pub const LOGBOOK_WHERE: &str = "t.status IN ('completed', 'canceled')";

/// **Opt-in** clause for the Data Model §6.3 "OR a reminder fires today" arm of
/// Today. Left out of [`TODAY_WHERE`] by default because it needs a `reminder`
/// join and DST-correct day math (the reminder carries an IANA `tz`, Data Model
/// §7); this fragment is an approximation over UTC and is documented as such for
/// the integration phase to refine. `OR` it into the Today predicate to enable.
pub const TODAY_REMINDER_FIRES_CLAUSE: &str = "EXISTS (SELECT 1 FROM reminder r \
WHERE r.target_kind = 'task' AND r.target_id = t.entity_id \
AND r.state = 'pending' AND date(r.fire_at / 1000, 'unixepoch') = :today)";

/// The five queryable task views. A superset of [`app_domain::Bucket`] (which
/// covers only the four *live* buckets) with the closed-task **Logbook** added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QueryBucket {
    /// Scheduled for today-or-earlier, or due today-or-overdue.
    Today,
    /// Future-dated.
    Upcoming,
    /// Actionable but undated.
    Anytime,
    /// Explicitly deferred.
    Someday,
    /// Completed or canceled (reverse-chronological).
    Logbook,
}

impl From<Bucket> for QueryBucket {
    fn from(b: Bucket) -> Self {
        match b {
            Bucket::Today => Self::Today,
            Bucket::Upcoming => Self::Upcoming,
            Bucket::Anytime => Self::Anytime,
            Bucket::Someday => Self::Someday,
        }
    }
}

impl QueryBucket {
    /// The WHERE fragment (excluding [`LIVE_FILTER`]) for this bucket.
    #[must_use]
    pub const fn where_clause(self) -> &'static str {
        match self {
            Self::Today => TODAY_WHERE,
            Self::Upcoming => UPCOMING_WHERE,
            Self::Anytime => ANYTIME_WHERE,
            Self::Someday => SOMEDAY_WHERE,
            Self::Logbook => LOGBOOK_WHERE,
        }
    }

    /// The ORDER BY for this bucket. Live buckets order by fractional index
    /// (drag-reorder, FS §3.4); Upcoming groups by date first; Logbook is
    /// reverse-chronological by completion (FS §3.1/§3.5).
    #[must_use]
    pub const fn order_by(self) -> &'static str {
        match self {
            Self::Today | Self::Anytime | Self::Someday => "ORDER BY t.order_key ASC",
            Self::Upcoming => "ORDER BY COALESCE(t.start_on, t.deadline_on) ASC, t.order_key ASC",
            Self::Logbook => "ORDER BY t.completed_at DESC",
        }
    }

    /// Whether the query binds the `:today` parameter. Only [`Today`](Self::Today)
    /// and [`Upcoming`](Self::Upcoming) reference it; `Anytime`, `Someday`, and
    /// `Logbook` are date-independent, so binding `:today` there would be an
    /// unknown-parameter error at prepare/bind time.
    #[must_use]
    pub const fn needs_today_param(self) -> bool {
        matches!(self, Self::Today | Self::Upcoming)
    }

    /// The complete, ready-to-`prepare` SELECT statement for this bucket.
    ///
    /// Bind `:today` (a `YYYY-MM-DD` string) only when
    /// [`needs_today_param`](Self::needs_today_param) is `true` (Today and Upcoming).
    #[must_use]
    pub fn sql(self) -> String {
        format!(
            "SELECT {cols} {from} WHERE {live} AND ({pred}) {order}",
            cols = TASK_SELECT_COLUMNS,
            from = TASK_FROM,
            live = LIVE_FILTER,
            pred = self.where_clause(),
            order = self.order_by(),
        )
    }
}

/// In-memory twin of the SQL predicates: the live bucket an open task belongs to,
/// or `None` when it is closed (Logbook) — mirror this for reactive membership
/// (Feature Specs AC-3.5). The caller must pre-filter soft-deleted entities
/// (`entity.deleted_at`), which this detail struct does not carry.
///
/// Guaranteed to agree with [`QueryBucket::where_clause`] for the four live
/// buckets; the module tests assert the partition is total and exclusive.
#[must_use]
pub fn classify(task: &Task, today: Day) -> Option<Bucket> {
    use crate::domain::TaskStatus;

    if task.status != TaskStatus::Open {
        return None; // Logbook (completed/canceled)
    }
    if task.someday {
        return Some(Bucket::Someday);
    }
    let start_due = task.start_on.is_some_and(|d| d <= today);
    let deadline_due = task.deadline_on.is_some_and(|d| d <= today);
    if start_due || deadline_due {
        return Some(Bucket::Today);
    }
    let start_future = task.start_on.is_some_and(|d| d > today);
    let deadline_future = task.deadline_on.is_some_and(|d| d > today);
    if start_future || deadline_future {
        return Some(Bucket::Upcoming);
    }
    // Neither dated nor deferred → actionable now.
    Some(Bucket::Anytime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::TaskStatus;
    use app_domain::{Id, Timestamp};
    use std::str::FromStr;

    fn day(s: &str) -> Day {
        Day::from_str(s).unwrap()
    }

    fn task_with(
        status: TaskStatus,
        someday: bool,
        start_on: Option<&str>,
        deadline_on: Option<&str>,
    ) -> Task {
        Task {
            entity_id: Id::new(),
            project_id: None,
            area_id: None,
            heading_id: None,
            parent_task_id: None,
            notes_md: None,
            status,
            priority: 0,
            someday,
            start_on: start_on.map(day),
            deadline_on: deadline_on.map(day),
            completed_at: if status == TaskStatus::Open {
                None
            } else {
                Some(Timestamp::now())
            },
            order_key: "V".to_string(),
            assignee_person_id: None,
            recurrence_id: None,
        }
    }

    #[test]
    fn every_bucket_sql_is_well_formed() {
        for b in [
            QueryBucket::Today,
            QueryBucket::Upcoming,
            QueryBucket::Anytime,
            QueryBucket::Someday,
            QueryBucket::Logbook,
        ] {
            let sql = b.sql();
            assert!(sql.starts_with("SELECT "), "{sql}");
            assert!(sql.contains("FROM task t JOIN entity e"));
            assert!(sql.contains("e.deleted_at IS NULL"));
            assert!(sql.contains("ORDER BY"));
            assert_eq!(sql.contains(":today"), b.needs_today_param());
        }
    }

    #[test]
    fn classify_matches_acceptance_criteria() {
        let today = day("2026-07-23");

        // AC-3.1: start_on = future Monday → only Upcoming.
        let t = task_with(TaskStatus::Open, false, Some("2026-07-27"), None);
        assert_eq!(classify(&t, today), Some(Bucket::Upcoming));

        // AC-3.1 cont.: on that Monday it moves to Today.
        assert_eq!(classify(&t, day("2026-07-27")), Some(Bucket::Today));

        // AC-3.2: deadline_on = today, no start → Today (not Anytime).
        let t = task_with(TaskStatus::Open, false, None, Some("2026-07-23"));
        assert_eq!(classify(&t, today), Some(Bucket::Today));

        // Anytime: no dates.
        let t = task_with(TaskStatus::Open, false, None, None);
        assert_eq!(classify(&t, today), Some(Bucket::Anytime));

        // Someday wins over dates.
        let t = task_with(TaskStatus::Open, true, Some("2026-07-01"), None);
        assert_eq!(classify(&t, today), Some(Bucket::Someday));

        // Overdue start → Today.
        let t = task_with(TaskStatus::Open, false, Some("2026-07-01"), None);
        assert_eq!(classify(&t, today), Some(Bucket::Today));

        // Future deadline only → Upcoming.
        let t = task_with(TaskStatus::Open, false, None, Some("2026-08-01"));
        assert_eq!(classify(&t, today), Some(Bucket::Upcoming));

        // Closed tasks are Logbook (None from classify).
        let t = task_with(TaskStatus::Completed, false, None, None);
        assert_eq!(classify(&t, today), None);
        let t = task_with(TaskStatus::Canceled, false, None, None);
        assert_eq!(classify(&t, today), None);
    }

    #[test]
    fn open_nonsomeday_tasks_partition_exactly_once() {
        // Exhaustively confirm the live buckets are mutually exclusive & total
        // for every date combination of {none, past, today, future}.
        let today = day("2026-07-23");
        let dates = [
            None,
            Some("2026-07-01"),
            Some("2026-07-23"),
            Some("2026-08-01"),
        ];
        for &s in &dates {
            for &d in &dates {
                let t = task_with(TaskStatus::Open, false, s, d);
                let bucket = classify(&t, today).expect("open task always classifies");
                // Someday is impossible here (someday=false); ensure it's one of
                // the three live, non-someday buckets.
                assert!(matches!(
                    bucket,
                    Bucket::Today | Bucket::Upcoming | Bucket::Anytime
                ));
            }
        }
    }
}

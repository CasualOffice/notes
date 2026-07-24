//! Projection: Casual Note items -> calendar events (doc §5).
//!
//! See `docs/casual-note-calendar.md` §5. These are pure, non-destructive
//! mappings: each produced event carries a [`SourceRef`] back-link and a
//! deterministic `UID` derived from that ref, so re-projecting the same item is
//! stable (idempotent) and the reverse [`detect_source_ref`] helper can recover
//! the origin.
//!
//! To keep this crate standalone (no dependency on the `tasks` / `reminders` /
//! `notes` crates), callers pass plain input structs holding just the fields the
//! projection needs — never the pillar entities themselves.

use chrono::{NaiveDate, TimeZone, Utc};

use app_domain::{Id, Timestamp};

use crate::error::{CalendarError, CalendarResult};
use crate::model::{
    AlarmAction, AlarmTrigger, CalendarEvent, EventAlarm, EventStatus, SourceKind, SourceRef,
    Transparency,
};

/// Default duration (seconds) for a scheduled task with a time but no explicit end.
const DEFAULT_TASK_SECS: i64 = 3_600;

/// A scheduled task, reduced to the fields projection needs (doc §5 row 1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskInput {
    /// The task entity id (`kind='task'`).
    pub id: Id,
    /// Task title -> event `SUMMARY`.
    pub title: String,
    /// `start_on` wall-date, if scheduled as all-day.
    pub start_on: Option<NaiveDate>,
    /// `deadline_on` wall-date, if any.
    pub deadline_on: Option<NaiveDate>,
    /// A specific timed instant, if the task is time-boxed (takes precedence over
    /// the all-day dates).
    pub scheduled_at: Option<Timestamp>,
    /// IANA tz for `scheduled_at`; `None` = floating.
    pub tz: Option<String>,
    /// Optional notes -> event `DESCRIPTION`.
    pub notes: Option<String>,
    /// Whether the task is completed (a completed task projects a `CANCELLED`
    /// event, so downstream sync removes/tombstones it — doc §5 "completing the
    /// task updates/removes the event").
    pub completed: bool,
}

/// A reminder, reduced to the fields projection needs (doc §5 row 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReminderInput {
    /// The reminder entity id (`kind='reminder'`).
    pub id: Id,
    /// Reminder title -> event `SUMMARY` and the alarm `DESCRIPTION`.
    pub title: String,
    /// Absolute fire instant.
    pub fire_at: Timestamp,
    /// IANA tz the reminder fires in (reminders always carry a tz — Data Model §1).
    pub tz: String,
    /// Optional recurrence (raw `RRULE` value); recurrence uses the same engine.
    pub rrule: Option<String>,
    /// Lead time in seconds before `fire_at` that the alarm should trigger
    /// (`None`/`0` = fire exactly at the event start).
    pub lead_secs: Option<i64>,
    /// Optional notes -> event `DESCRIPTION`.
    pub notes: Option<String>,
}

/// A meeting session, reduced to the fields projection needs (doc §5 row 3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MeetingInput {
    /// The session entity id (`kind='session'`).
    pub id: Id,
    /// Meeting title -> event `SUMMARY`.
    pub title: String,
    /// Recording/meeting start.
    pub start: Timestamp,
    /// Recording/meeting end.
    pub end: Timestamp,
    /// IANA tz; `None` = UTC/floating.
    pub tz: Option<String>,
    /// Optional location -> event `LOCATION`.
    pub location: Option<String>,
    /// Optional summary/description -> event `DESCRIPTION`.
    pub summary: Option<String>,
}

/// Deterministic, collision-resistant `UID` for a projected event: it encodes the
/// source marker so re-projection is stable and the origin is self-describing.
#[must_use]
pub fn projected_uid(source: &SourceRef) -> String {
    format!("{}@casual-note", source.marker_value())
}

/// Project a scheduled task into an event (doc §5). A task with a `scheduled_at`
/// becomes a timed event; one with only `start_on`/`deadline_on` becomes an
/// all-day event. Errors if the task carries no schedulable date at all.
pub fn task_to_event(task: &TaskInput, calendar_id: Id) -> CalendarResult<CalendarEvent> {
    let source = SourceRef {
        kind: SourceKind::Task,
        entity_id: task.id,
    };
    let (start_utc, end_utc, all_day, tz) = if let Some(at) = task.scheduled_at {
        (
            at,
            Timestamp::from_millis(at.as_millis() + DEFAULT_TASK_SECS * 1000),
            false,
            task.tz.clone(),
        )
    } else {
        let start_date = task.start_on.or(task.deadline_on).ok_or_else(|| {
            CalendarError::Projection(
                "task has neither scheduled_at, start_on, nor deadline_on".to_string(),
            )
        })?;
        let end_date = task.deadline_on.or(task.start_on).unwrap_or(start_date);
        let end_exclusive = end_date.succ_opt().ok_or_else(|| {
            CalendarError::Projection("deadline date overflow computing all-day end".to_string())
        })?;
        (
            Timestamp::from_millis(date_utc_ms(start_date)?),
            Timestamp::from_millis(date_utc_ms(end_exclusive)?),
            true,
            None,
        )
    };

    let mut event = CalendarEvent::new(
        calendar_id,
        projected_uid(&source),
        &task.title,
        start_utc,
        end_utc,
    );
    event.all_day = all_day;
    event.tz = tz;
    event.description = task.notes.clone();
    event.status = if task.completed {
        EventStatus::Cancelled
    } else {
        EventStatus::Confirmed
    };
    event.source_ref = Some(source);
    Ok(event)
}

/// Project a reminder into an event carrying a `VALARM` (doc §5 row 2). The event
/// is a zero-length instant at `fire_at`; the alarm fires `lead_secs` before it.
#[must_use]
pub fn reminder_to_event(reminder: &ReminderInput, calendar_id: Id) -> CalendarEvent {
    let source = SourceRef {
        kind: SourceKind::Reminder,
        entity_id: reminder.id,
    };
    let mut event = CalendarEvent::new(
        calendar_id,
        projected_uid(&source),
        &reminder.title,
        reminder.fire_at,
        reminder.fire_at,
    );
    event.tz = Some(reminder.tz.clone());
    event.rrule = reminder.rrule.clone();
    event.description = reminder.notes.clone();
    // Reminders don't consume busy time.
    event.transparency = Transparency::Transparent;
    event.source_ref = Some(source);

    let lead = reminder.lead_secs.unwrap_or(0);
    event.alarms.push(EventAlarm {
        action: AlarmAction::Display,
        // Negative offset = fire *before* the event start.
        trigger: AlarmTrigger::Relative {
            offset_secs: -lead,
            related_end: false,
        },
        description: Some(reminder.title.clone()),
        summary: None,
        repeat: None,
        repeat_interval_secs: None,
    });
    event
}

/// Project a meeting session into an event spanning the recording (doc §5 row 3).
#[must_use]
pub fn meeting_to_event(meeting: &MeetingInput, calendar_id: Id) -> CalendarEvent {
    let source = SourceRef {
        kind: SourceKind::Meeting,
        entity_id: meeting.id,
    };
    let mut event = CalendarEvent::new(
        calendar_id,
        projected_uid(&source),
        &meeting.title,
        meeting.start,
        meeting.end,
    );
    event.tz = meeting.tz.clone();
    event.location = meeting.location.clone();
    event.description = meeting.summary.clone();
    event.source_ref = Some(source);
    event
}

/// Reverse marker detection (doc §5): recover the Casual Note origin of an event.
///
/// Prefers the parsed [`SourceRef`] (from the `X-CASUAL-NOTE-SOURCE` property);
/// falls back to decoding a projected `UID` of the form `<kind>:<uuid>@casual-note`.
/// Returns `None` for a genuinely external event with no marker.
#[must_use]
pub fn detect_source_ref(event: &CalendarEvent) -> Option<SourceRef> {
    if let Some(sr) = event.source_ref {
        return Some(sr);
    }
    let marker = event.uid.strip_suffix("@casual-note")?;
    SourceRef::parse_marker(marker)
}

/// Midnight UTC epoch-ms for an all-day wall-date.
fn date_utc_ms(date: NaiveDate) -> CalendarResult<i64> {
    let ndt = date.and_hms_opt(0, 0, 0).ok_or_else(|| {
        CalendarError::Projection("all-day date produced an invalid midnight".to_string())
    })?;
    Ok(Utc.from_utc_datetime(&ndt).timestamp_millis())
}

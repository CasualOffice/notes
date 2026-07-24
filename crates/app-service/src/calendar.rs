//! Unified calendar agenda (HLD §6 `calendar.*`, calendar doc §5). **Read-mostly
//! projection — this pass adds no storage tables.** The agenda is rebuilt on every
//! call by loading the already-persisted pillar rows in range (scheduled `task`s,
//! `reminder`s, and meeting `session`s) and projecting each into a
//! [`calendar::CalendarEvent`] via the pure `calendar` crate helpers
//! (`task_to_event` / `reminder_to_event` / `meeting_to_event`). Full event
//! persistence + CalDAV account sync is a later pass behind the same projection.
//!
//! Both entry points ([`Service::calendar_agenda`], [`Service::calendar_export_ics`])
//! share one projection pass; agenda returns a lean, source-tagged
//! [`AgendaEvent`](crate::dto::AgendaEvent) list, export serializes the same events
//! to an RFC 5545 ICS string via [`calendar::write_ics`].

use app_domain::{AppError, AppResult, Id};
use calendar::{
    meeting_to_event, reminder_to_event, task_to_event, CalendarEvent, EventStatus, MeetingInput,
    ReminderInput, SourceKind, TaskInput,
};
use chrono::NaiveDate;
use rusqlite::Row;

use crate::dto::AgendaEvent;
use crate::Service;

/// Parse a `YYYY-MM-DD` wall-date column, tolerating NULL / malformed values.
fn parse_day(s: &Option<String>) -> Option<NaiveDate> {
    s.as_deref().and_then(|v| v.parse::<NaiveDate>().ok())
}

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}

/// Whether a projected event overlaps the inclusive `[from, to]` epoch-ms window.
/// A zero-length instant (a reminder) is kept when it lands inside the window.
fn in_range(ev: &CalendarEvent, from: i64, to: i64) -> bool {
    ev.start_utc.as_millis() <= to && ev.end_utc.as_millis() >= from
}

impl Service {
    /// `calendar.agenda` — the merged, start-sorted, source-tagged agenda across
    /// scheduled tasks, reminders, and meetings within `[from_ms, to_ms]` (absolute
    /// UTC epoch-ms). Pure read: re-projects live rows, persists nothing.
    pub fn calendar_agenda(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<AgendaEvent>> {
        let events = self.agenda_events(from_ms, to_ms)?;
        Ok(events.iter().map(agenda_dto).collect())
    }

    /// `calendar.export_ics` — the same in-range projection serialized to an
    /// RFC 5545 ICS document (calendar §2/§9), for "export my week" / share.
    pub fn calendar_export_ics(&self, from_ms: i64, to_ms: i64) -> AppResult<String> {
        let events = self.agenda_events(from_ms, to_ms)?;
        calendar::write_ics(&events)
            .map_err(|e| AppError::Internal(format!("ics export failed: {e}")))
    }

    /// Shared projection pass: load persisted tasks/reminders/meetings overlapping
    /// the window and project each to a [`CalendarEvent`], merged + start-sorted.
    fn agenda_events(&self, from_ms: i64, to_ms: i64) -> AppResult<Vec<CalendarEvent>> {
        // One synthetic owning calendar for this read (never persisted); projection
        // requires a calendar id but the agenda is calendar-agnostic here.
        let cal = Id::new();
        let mut out: Vec<CalendarEvent> = Vec::new();

        self.read(|c| {
            // -- Scheduled tasks (start_on / deadline_on) -----------------------
            let mut tstmt = c.prepare(
                "SELECT t.entity_id, e.title, t.start_on, t.deadline_on, t.notes_md, t.status \
                 FROM task t JOIN entity e ON e.id = t.entity_id \
                 WHERE e.deleted_at IS NULL \
                   AND (t.start_on IS NOT NULL OR t.deadline_on IS NOT NULL)",
            )?;
            let tasks = tstmt
                .query_map([], map_task_input)?
                .collect::<Result<Vec<_>, _>>()?;
            for ti in tasks {
                // A task whose only dates failed to parse is not schedulable; skip.
                if ti.start_on.is_none() && ti.deadline_on.is_none() {
                    continue;
                }
                if let Ok(ev) = task_to_event(&ti, cal) {
                    out.push(ev);
                }
            }

            // -- Reminders (pending / snoozed) ----------------------------------
            let mut rstmt = c.prepare(
                "SELECT entity_id, body, fire_at, tz, snoozed_until \
                 FROM reminder WHERE state IN ('pending','snoozed')",
            )?;
            let reminders = rstmt
                .query_map([], map_reminder_input)?
                .collect::<Result<Vec<_>, _>>()?;
            for ri in reminders {
                out.push(reminder_to_event(&ri, cal));
            }

            // -- Meetings (sessions with a start) -------------------------------
            let mut sstmt = c.prepare(
                "SELECT s.entity_id, e.title, s.started_at, s.ended_at, s.duration_ms \
                 FROM session s JOIN entity e ON e.id = s.entity_id \
                 WHERE e.deleted_at IS NULL AND s.started_at IS NOT NULL",
            )?;
            let meetings = sstmt
                .query_map([], map_meeting_input)?
                .collect::<Result<Vec<_>, _>>()?;
            for mi in meetings {
                out.push(meeting_to_event(&mi, cal));
            }

            Ok(())
        })?;

        out.retain(|ev| in_range(ev, from_ms, to_ms));
        out.sort_by_key(|ev| ev.start_utc.as_millis());
        Ok(out)
    }
}

/// Map a task row to the projection input (dates parsed leniently; unparseable
/// dates become `None` and the caller skips a fully-undated task).
fn map_task_input(r: &Row<'_>) -> rusqlite::Result<TaskInput> {
    let start_on: Option<String> = r.get(2)?;
    let deadline_on: Option<String> = r.get(3)?;
    let status: String = r.get(5)?;
    let completed = tasks::TaskStatus::from_db_str(&status).is_some_and(|s| s.is_closed());
    Ok(TaskInput {
        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)),
        title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        start_on: parse_day(&start_on),
        deadline_on: parse_day(&deadline_on),
        scheduled_at: None,
        tz: None,
        notes: r.get(4)?,
        completed,
    })
}

/// Map a reminder row to the projection input. A snoozed reminder fires at its
/// `snoozed_until`, mirroring the scheduler's re-arm (Data Model §7).
fn map_reminder_input(r: &Row<'_>) -> rusqlite::Result<ReminderInput> {
    let body: Option<String> = r.get(1)?;
    let fire_at: i64 = r.get(2)?;
    let snoozed: Option<i64> = r.get(4)?;
    Ok(ReminderInput {
        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)),
        title: body.unwrap_or_else(|| "Reminder".to_string()),
        fire_at: app_domain::Timestamp::from_millis(snoozed.unwrap_or(fire_at)),
        tz: r.get(3)?,
        rrule: None,
        lead_secs: None,
        notes: None,
    })
}

/// Map a session row to the meeting projection input. An in-progress session with
/// no `ended_at` projects a zero-length event at its start.
fn map_meeting_input(r: &Row<'_>) -> rusqlite::Result<MeetingInput> {
    let started_at: i64 = r.get(2)?;
    let ended_at: Option<i64> = r.get(3)?;
    let duration_ms: Option<i64> = r.get(4)?;
    let end = ended_at
        .or_else(|| duration_ms.map(|d| started_at + d.max(0)))
        .unwrap_or(started_at);
    Ok(MeetingInput {
        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)),
        title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
        start: app_domain::Timestamp::from_millis(started_at),
        end: app_domain::Timestamp::from_millis(end),
        tz: None,
        location: None,
        summary: None,
    })
}

/// Flatten a projected [`CalendarEvent`] into the lean, source-tagged wire shape.
fn agenda_dto(ev: &CalendarEvent) -> AgendaEvent {
    let (source, source_id) = match ev.source_ref {
        Some(sr) => (source_str(sr.kind).to_string(), sr.entity_id.to_string()),
        // Every agenda event is projected, so a source_ref is always present; fall
        // back to the event id defensively rather than panic.
        None => ("unknown".to_string(), ev.id.to_string()),
    };
    AgendaEvent {
        uid: ev.uid.clone(),
        title: ev.title.clone(),
        start_ms: ev.start_utc.as_millis(),
        end_ms: ev.end_utc.as_millis(),
        all_day: ev.all_day,
        source,
        source_id,
        status: status_str(ev.status).to_string(),
        location: ev.location.clone(),
        description: ev.description.clone(),
    }
}

const fn source_str(k: SourceKind) -> &'static str {
    match k {
        SourceKind::Task => "task",
        SourceKind::Reminder => "reminder",
        SourceKind::Meeting => "meeting",
    }
}

const fn status_str(s: EventStatus) -> &'static str {
    match s {
        EventStatus::Confirmed => "confirmed",
        EventStatus::Tentative => "tentative",
        EventStatus::Cancelled => "cancelled",
    }
}

#[cfg(test)]
mod tests {
    use crate::dto::{NewReminder, NewTask};
    use crate::{EventSink, Service};
    use app_domain::Id;
    use storage::{Paths, Store};

    fn svc() -> Service {
        let dir = std::env::temp_dir().join(format!("cn-cal-{}", Id::new()));
        let store = Store::open_memory(Paths::new(dir)).expect("open_memory");
        let sink: EventSink = Box::new(|_| {});
        Service::new(store, "test", sink)
    }

    #[test]
    fn agenda_merges_and_tags_sources_in_range() {
        let s = svc();
        // A task due today (all-day) and a reminder firing today.
        let day = crate::util::today_local();
        s.tasks_create(NewTask {
            title: "Ship the beta".into(),
            deadline_on: Some(day.clone()),
            ..NewTask::default()
        })
        .unwrap();
        let now = s.now_ms();
        s.reminders_create(NewReminder {
            target: None,
            fire_at: now,
            tz: "UTC".into(),
            body: Some("Call Sam".into()),
        })
        .unwrap();

        // A wide window covering all of today.
        let from = now - 86_400_000;
        let to = now + 86_400_000;
        let agenda = s.calendar_agenda(from, to).unwrap();
        assert_eq!(agenda.len(), 2, "task + reminder both project into range");
        assert!(agenda.iter().any(|e| e.source == "task"));
        assert!(agenda.iter().any(|e| e.source == "reminder"));
        // Sorted by start.
        assert!(agenda[0].start_ms <= agenda[1].start_ms);

        // Export the same window as ICS.
        let ics = s.calendar_export_ics(from, to).unwrap();
        assert!(ics.starts_with("BEGIN:VCALENDAR"));
        assert!(ics.contains("BEGIN:VEVENT"));
    }

    #[test]
    fn agenda_excludes_out_of_range_items() {
        let s = svc();
        let now = s.now_ms();
        s.reminders_create(NewReminder {
            target: None,
            fire_at: now,
            tz: "UTC".into(),
            body: Some("Later".into()),
        })
        .unwrap();
        // A window entirely before the reminder.
        let agenda = s
            .calendar_agenda(now - 20_000_000, now - 10_000_000)
            .unwrap();
        assert!(agenda.is_empty(), "reminder outside the window is excluded");
    }
}

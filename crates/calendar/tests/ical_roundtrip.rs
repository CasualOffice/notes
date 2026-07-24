//! Acceptance tests for the calendar engine (doc §9 gate 1 + §5 projection).
//!
//! See `docs/casual-note-calendar.md`:
//! - §2 / §9: ICS export -> import must round-trip a representative corpus
//!   (recurrence, all-day, timezone, alarms, RECURRENCE-ID exceptions) losslessly.
//! - §5: projection of Casual Note items (task / reminder / meeting) to events,
//!   with `source_ref` stamping and reverse marker detection.
//!
//! Round-trip equality is *semantic*: the parser mints a fresh entity [`Id`] and
//! is handed the target `calendar_id`, so those two fields are normalized before
//! comparison. `etag` is never serialized (it is CalDAV transport state), so the
//! corpus uses `etag: None`.

use chrono::NaiveDate;

use app_domain::{Id, Timestamp};
use calendar::model::{
    AlarmAction, AlarmTrigger, CalendarEvent, EventAlarm, EventStatus, RecurrenceId, SourceKind,
    SourceRef, Transparency,
};
use calendar::{
    detect_source_ref, meeting_to_event, parse_ics, projected_uid, reminder_to_event,
    task_to_event, write_ics, MeetingInput, ReminderInput, TaskInput,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialize one event, parse it back, and normalize the fields the parser is
/// entitled to change (fresh `id`; `calendar_id` is supplied on parse).
fn roundtrip(ev: &CalendarEvent) -> CalendarEvent {
    let text = write_ics(std::slice::from_ref(ev)).expect("write_ics");
    // The serialized document must be well-formed RFC 5545.
    assert!(
        text.starts_with("BEGIN:VCALENDAR\r\n"),
        "missing VCALENDAR head"
    );
    assert!(text.contains("END:VCALENDAR\r\n"), "missing VCALENDAR tail");
    assert!(text.contains("BEGIN:VEVENT\r\n"), "missing VEVENT");
    let mut parsed = parse_ics(&text, ev.calendar_id).expect("parse_ics");
    assert_eq!(parsed.len(), 1, "expected exactly one VEVENT to parse back");
    let mut got = parsed.pop().unwrap();
    got.id = ev.id; // parser assigns a fresh entity id — normalize it out
    got
}

fn assert_roundtrips(ev: &CalendarEvent) {
    let got = roundtrip(ev);
    assert_eq!(&got, ev, "event did not round-trip losslessly");
}

fn ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    let ndt = NaiveDate::from_ymd_opt(y, mo, d)
        .unwrap()
        .and_hms_opt(h, mi, 0)
        .unwrap();
    Timestamp::from_millis(chrono::Utc.from_utc_datetime(&ndt).timestamp_millis())
}

use chrono::TimeZone;

// ---------------------------------------------------------------------------
// ICS round-trip corpus (doc §9, gate 1)
// ---------------------------------------------------------------------------

#[test]
fn roundtrip_utc_timed_event() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "utc-1@ex",
        "Standup",
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 9, 30),
    );
    ev.tz = Some("UTC".into());
    ev.location = Some("Room A; floor 3".into()); // exercises TEXT escaping of ';'
    ev.description = Some("line1\nline2, with comma".into());
    ev.status = EventStatus::Tentative;
    ev.transparency = Transparency::Transparent;
    ev.sequence = 4;
    ev.last_modified = Some(ms(2026, 7, 20, 8, 0));
    ev.created = Some(ms(2026, 7, 19, 8, 0));
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_floating_local_event() {
    // No tz, not all-day -> floating local wall-clock.
    let cal = Id::new();
    let ev = CalendarEvent::new(
        cal,
        "float-1@ex",
        "Floating",
        ms(2026, 1, 2, 14, 0),
        ms(2026, 1, 2, 15, 0),
    );
    assert!(ev.tz.is_none());
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_tzid_event() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "tz-1@ex",
        "NYC meeting",
        // 2026-07-23 13:00Z == 09:00 America/New_York (EDT, UTC-4)
        ms(2026, 7, 23, 13, 0),
        ms(2026, 7, 23, 14, 0),
    );
    ev.tz = Some("America/New_York".into());
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(
        text.contains("TZID=America/New_York"),
        "TZID param must be emitted"
    );
    assert!(text.contains("T090000"), "local wall-clock must be 09:00");
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_all_day_event() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "allday-1@ex",
        "Conference",
        ms(2026, 7, 23, 0, 0),
        ms(2026, 7, 25, 0, 0), // exclusive end: covers the 23rd and 24th
    );
    ev.all_day = true;
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(
        text.contains("DTSTART;VALUE=DATE:20260723"),
        "all-day DTSTART must serialize as a date-only VALUE=DATE, got:\n{text}"
    );
    assert!(
        text.contains("DTEND;VALUE=DATE:20260725"),
        "all-day DTEND must be the exclusive day-after date"
    );
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_recurring_with_exdate() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "rec-1@ex",
        "Weekly",
        ms(2026, 7, 6, 9, 0),
        ms(2026, 7, 6, 9, 30),
    );
    ev.tz = Some("UTC".into());
    ev.rrule = Some("FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6".into());
    ev.exdates = vec![ms(2026, 7, 8, 9, 0)]; // skip the 2nd occurrence
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(text.contains("RRULE:FREQ=WEEKLY;BYDAY=MO,WE;COUNT=6"));
    assert!(text.contains("EXDATE:"));
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_event_with_valarm() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "alarm-1@ex",
        "Call",
        ms(2026, 7, 23, 16, 0),
        ms(2026, 7, 23, 16, 30),
    );
    ev.tz = Some("UTC".into());
    ev.alarms.push(EventAlarm {
        action: AlarmAction::Display,
        trigger: AlarmTrigger::Relative {
            offset_secs: -900,
            related_end: false,
        },
        description: Some("Call starting soon".into()),
        summary: None,
        repeat: Some(2),
        repeat_interval_secs: Some(300),
    });
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(text.contains("BEGIN:VALARM"));
    assert!(
        text.contains("TRIGGER:-PT15M"),
        "relative trigger should be -PT15M, got:\n{text}"
    );
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_valarm_absolute_and_related_end() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "alarm-2@ex",
        "Deadline",
        ms(2026, 7, 23, 16, 0),
        ms(2026, 7, 23, 17, 0),
    );
    ev.tz = Some("UTC".into());
    ev.alarms.push(EventAlarm {
        action: AlarmAction::Audio,
        trigger: AlarmTrigger::Absolute(ms(2026, 7, 23, 15, 45)),
        description: None,
        summary: None,
        repeat: None,
        repeat_interval_secs: None,
    });
    ev.alarms.push(EventAlarm {
        action: AlarmAction::Display,
        trigger: AlarmTrigger::Relative {
            offset_secs: -600,
            related_end: true,
        },
        description: Some("wrap up".into()),
        summary: None,
        repeat: None,
        repeat_interval_secs: None,
    });
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(
        text.contains("RELATED=END"),
        "second alarm should be RELATED=END"
    );
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_recurrence_id_exception() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "rec-2@ex",
        "Moved instance",
        ms(2026, 7, 13, 10, 0),
        ms(2026, 7, 13, 11, 0),
    );
    ev.tz = Some("UTC".into());
    ev.recurrence_id = Some(RecurrenceId {
        instant: ms(2026, 7, 13, 9, 0), // original instance was at 09:00
        all_day: false,
        tz: Some("UTC".into()),
        this_and_future: false,
    });
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(text.contains("RECURRENCE-ID"), "must emit RECURRENCE-ID");
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_this_and_future_recurrence_id() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "rec-3@ex",
        "Rescheduled series",
        ms(2026, 7, 13, 11, 0),
        ms(2026, 7, 13, 12, 0),
    );
    ev.tz = Some("UTC".into());
    ev.recurrence_id = Some(RecurrenceId {
        instant: ms(2026, 7, 13, 9, 0),
        all_day: false,
        tz: Some("UTC".into()),
        this_and_future: true,
    });
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(text.contains("RANGE=THISANDFUTURE"));
    assert_roundtrips(&ev);
}

#[test]
fn roundtrip_source_ref_marker() {
    let cal = Id::new();
    let src = SourceRef {
        kind: SourceKind::Task,
        entity_id: Id::new(),
    };
    let mut ev = CalendarEvent::new(
        cal,
        projected_uid(&src),
        "From task",
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 10, 0),
    );
    ev.tz = Some("UTC".into());
    ev.source_ref = Some(src);
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    assert!(
        text.contains("X-CASUAL-NOTE-SOURCE:task:"),
        "marker must be serialized"
    );
    assert_roundtrips(&ev);
}

#[test]
fn multi_event_document_roundtrips_all() {
    let cal = Id::new();
    let a = CalendarEvent::new(
        cal,
        "m-a@ex",
        "A",
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 10, 0),
    );
    let mut b = CalendarEvent::new(
        cal,
        "m-b@ex",
        "B",
        ms(2026, 7, 24, 0, 0),
        ms(2026, 7, 25, 0, 0),
    );
    b.all_day = true;
    let text = write_ics(&[a.clone(), b.clone()]).unwrap();
    let parsed = parse_ics(&text, cal).unwrap();
    assert_eq!(parsed.len(), 2);
    assert_eq!(parsed[0].uid, "m-a@ex");
    assert_eq!(parsed[1].uid, "m-b@ex");
    assert!(parsed[1].all_day);
}

#[test]
fn parser_tolerates_and_skips_vtimezone() {
    // A VTIMEZONE block must be skipped, not misparsed as event content.
    let cal = Id::new();
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VTIMEZONE\r\nTZID:America/New_York\r\nBEGIN:STANDARD\r\nDTSTART:20261101T020000\r\nTZOFFSETFROM:-0400\r\nTZOFFSETTO:-0500\r\nEND:STANDARD\r\nEND:VTIMEZONE\r\nBEGIN:VEVENT\r\nUID:skip-1@ex\r\nDTSTART:20260723T130000Z\r\nDTEND:20260723T140000Z\r\nSUMMARY:After tz\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed = parse_ics(ics, cal).unwrap();
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].uid, "skip-1@ex");
    assert_eq!(parsed[0].title, "After tz");
}

#[test]
fn parser_resolves_dtstart_plus_duration() {
    let cal = Id::new();
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nUID:dur-1@ex\r\nDTSTART:20260723T090000Z\r\nDURATION:PT1H30M\r\nSUMMARY:Dur\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let parsed = parse_ics(ics, cal).unwrap();
    assert_eq!(parsed.len(), 1);
    let secs = (parsed[0].end_utc.as_millis() - parsed[0].start_utc.as_millis()) / 1000;
    assert_eq!(secs, 5400, "DTSTART+DURATION must resolve DTEND");
}

#[test]
fn parser_rejects_event_without_uid() {
    let cal = Id::new();
    let ics = "BEGIN:VCALENDAR\r\nBEGIN:VEVENT\r\nDTSTART:20260723T090000Z\r\nSUMMARY:No uid\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    assert!(
        parse_ics(ics, cal).is_err(),
        "missing UID must be a parse error"
    );
}

#[test]
fn long_summary_is_folded_and_unfolds_cleanly() {
    let cal = Id::new();
    let long = "x".repeat(200);
    let mut ev = CalendarEvent::new(
        cal,
        "fold-1@ex",
        &long,
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 10, 0),
    );
    ev.tz = Some("UTC".into());
    let text = write_ics(std::slice::from_ref(&ev)).unwrap();
    // Every physical line must respect the 75-octet fold limit.
    for line in text.split("\r\n") {
        assert!(
            line.len() <= 75,
            "unfolded content line exceeds 75 octets: {}",
            line.len()
        );
    }
    let got = roundtrip(&ev);
    assert_eq!(
        got.title, long,
        "folded long value must unfold to the original"
    );
}

// ---------------------------------------------------------------------------
// Recurrence expansion (doc §3.2, shared rrule engine)
// ---------------------------------------------------------------------------

#[test]
fn occurrences_expands_daily_count() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "occ-1@ex",
        "Daily",
        ms(2026, 7, 1, 9, 0),
        ms(2026, 7, 1, 9, 30),
    );
    ev.tz = Some("UTC".into());
    ev.rrule = Some("FREQ=DAILY;COUNT=3".into());
    let occ = ev.occurrences(50).unwrap();
    assert_eq!(occ.len(), 3);
    assert_eq!(occ[0], ms(2026, 7, 1, 9, 0));
    assert_eq!(occ[1], ms(2026, 7, 2, 9, 0));
    assert_eq!(occ[2], ms(2026, 7, 3, 9, 0));
}

#[test]
fn occurrences_honors_exdate() {
    let cal = Id::new();
    let mut ev = CalendarEvent::new(
        cal,
        "occ-2@ex",
        "Daily",
        ms(2026, 7, 1, 9, 0),
        ms(2026, 7, 1, 9, 30),
    );
    ev.tz = Some("UTC".into());
    ev.rrule = Some("FREQ=DAILY;COUNT=5".into());
    ev.exdates = vec![ms(2026, 7, 3, 9, 0)];
    let occ = ev.occurrences(50).unwrap();
    assert_eq!(occ.len(), 4, "one instance excluded by EXDATE");
    assert!(!occ.contains(&ms(2026, 7, 3, 9, 0)));
}

#[test]
fn occurrences_non_recurring_yields_single_start() {
    let cal = Id::new();
    let ev = CalendarEvent::new(
        cal,
        "occ-3@ex",
        "Once",
        ms(2026, 7, 1, 9, 0),
        ms(2026, 7, 1, 9, 30),
    );
    let occ = ev.occurrences(50).unwrap();
    assert_eq!(occ, vec![ms(2026, 7, 1, 9, 0)]);
}

// ---------------------------------------------------------------------------
// Projection: Casual Note items -> events (doc §5)
// ---------------------------------------------------------------------------

#[test]
fn task_projection_timed() {
    let cal = Id::new();
    let tid = Id::new();
    let task = TaskInput {
        id: tid,
        title: "Write report".into(),
        start_on: None,
        deadline_on: None,
        scheduled_at: Some(ms(2026, 7, 23, 14, 0)),
        tz: Some("UTC".into()),
        notes: Some("draft first".into()),
        completed: false,
    };
    let ev = task_to_event(&task, cal).unwrap();
    assert_eq!(ev.calendar_id, cal);
    assert_eq!(ev.title, "Write report");
    assert!(!ev.all_day);
    assert_eq!(ev.start_utc, ms(2026, 7, 23, 14, 0));
    assert_eq!(ev.end_utc, ms(2026, 7, 23, 15, 0), "default 1h duration");
    assert_eq!(ev.status, EventStatus::Confirmed);
    assert_eq!(ev.description.as_deref(), Some("draft first"));
    let src = ev.source_ref.expect("source_ref stamped");
    assert_eq!(src.kind, SourceKind::Task);
    assert_eq!(src.entity_id, tid);
    assert_eq!(ev.uid, format!("task:{tid}@casual-note"));
}

#[test]
fn task_projection_all_day_from_dates() {
    let cal = Id::new();
    let task = TaskInput {
        id: Id::new(),
        title: "Sprint".into(),
        start_on: NaiveDate::from_ymd_opt(2026, 7, 20),
        deadline_on: NaiveDate::from_ymd_opt(2026, 7, 24),
        scheduled_at: None,
        tz: None,
        notes: None,
        completed: false,
    };
    let ev = task_to_event(&task, cal).unwrap();
    assert!(ev.all_day);
    assert_eq!(ev.start_utc, ms(2026, 7, 20, 0, 0));
    // Exclusive end = day after the deadline (the 25th).
    assert_eq!(ev.end_utc, ms(2026, 7, 25, 0, 0));
}

#[test]
fn completed_task_projects_cancelled() {
    let cal = Id::new();
    let task = TaskInput {
        id: Id::new(),
        title: "Done".into(),
        start_on: None,
        deadline_on: None,
        scheduled_at: Some(ms(2026, 7, 23, 9, 0)),
        tz: None,
        notes: None,
        completed: true,
    };
    let ev = task_to_event(&task, cal).unwrap();
    assert_eq!(ev.status, EventStatus::Cancelled);
}

#[test]
fn task_without_any_date_errors() {
    let cal = Id::new();
    let task = TaskInput {
        id: Id::new(),
        title: "No schedule".into(),
        start_on: None,
        deadline_on: None,
        scheduled_at: None,
        tz: None,
        notes: None,
        completed: false,
    };
    assert!(task_to_event(&task, cal).is_err());
}

#[test]
fn reminder_projection_has_valarm() {
    let cal = Id::new();
    let rid = Id::new();
    let reminder = ReminderInput {
        id: rid,
        title: "Take meds".into(),
        fire_at: ms(2026, 7, 23, 8, 0),
        tz: "America/New_York".into(),
        rrule: Some("FREQ=DAILY".into()),
        lead_secs: Some(600),
        notes: None,
    };
    let ev = reminder_to_event(&reminder, cal);
    assert_eq!(ev.start_utc, ms(2026, 7, 23, 8, 0));
    assert_eq!(ev.transparency, Transparency::Transparent);
    assert_eq!(ev.rrule.as_deref(), Some("FREQ=DAILY"));
    assert_eq!(ev.tz.as_deref(), Some("America/New_York"));
    assert_eq!(ev.alarms.len(), 1);
    match ev.alarms[0].trigger {
        AlarmTrigger::Relative {
            offset_secs,
            related_end,
        } => {
            assert_eq!(offset_secs, -600, "alarm fires 10 min before the reminder");
            assert!(!related_end);
        }
        AlarmTrigger::Absolute(_) => panic!("expected a relative trigger"),
    }
    let src = ev.source_ref.expect("source_ref");
    assert_eq!(src.kind, SourceKind::Reminder);
    assert_eq!(src.entity_id, rid);
}

#[test]
fn meeting_projection_spans_recording() {
    let cal = Id::new();
    let sid = Id::new();
    let meeting = MeetingInput {
        id: sid,
        title: "Design sync".into(),
        start: ms(2026, 7, 23, 15, 0),
        end: ms(2026, 7, 23, 16, 0),
        tz: Some("UTC".into()),
        location: Some("Zoom".into()),
        summary: Some("agenda".into()),
    };
    let ev = meeting_to_event(&meeting, cal);
    assert_eq!(ev.start_utc, ms(2026, 7, 23, 15, 0));
    assert_eq!(ev.end_utc, ms(2026, 7, 23, 16, 0));
    assert_eq!(ev.location.as_deref(), Some("Zoom"));
    assert_eq!(ev.description.as_deref(), Some("agenda"));
    let src = ev.source_ref.expect("source_ref");
    assert_eq!(src.kind, SourceKind::Meeting);
    assert_eq!(src.entity_id, sid);
}

#[test]
fn detect_source_ref_prefers_property() {
    let cal = Id::new();
    let src = SourceRef {
        kind: SourceKind::Meeting,
        entity_id: Id::new(),
    };
    let mut ev = CalendarEvent::new(
        cal,
        "external-uid@somewhere",
        "X",
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 10, 0),
    );
    ev.source_ref = Some(src);
    let got = detect_source_ref(&ev).expect("detected");
    assert_eq!(got, src);
}

#[test]
fn detect_source_ref_falls_back_to_uid_marker() {
    let cal = Id::new();
    let src = SourceRef {
        kind: SourceKind::Task,
        entity_id: Id::new(),
    };
    // No source_ref field, but the UID encodes the marker.
    let ev = CalendarEvent::new(
        cal,
        projected_uid(&src),
        "X",
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 10, 0),
    );
    let got = detect_source_ref(&ev).expect("detected via uid");
    assert_eq!(got, src);
}

#[test]
fn detect_source_ref_none_for_external_event() {
    let cal = Id::new();
    let ev = CalendarEvent::new(
        cal,
        "meeting-1234@google.com",
        "External",
        ms(2026, 7, 23, 9, 0),
        ms(2026, 7, 23, 10, 0),
    );
    assert!(detect_source_ref(&ev).is_none());
}

#[test]
fn projection_output_roundtrips_through_ics() {
    // End-to-end: a projected event must survive ICS export/import and still be
    // recognized as Casual-Note-originated on the far side.
    let cal = Id::new();
    let rid = Id::new();
    let reminder = ReminderInput {
        id: rid,
        title: "Standup".into(),
        fire_at: ms(2026, 7, 23, 9, 0),
        tz: "UTC".into(),
        rrule: Some("FREQ=WEEKLY;BYDAY=MO".into()),
        lead_secs: Some(300),
        notes: Some("daily".into()),
    };
    let ev = reminder_to_event(&reminder, cal);
    let got = roundtrip(&ev);
    assert_eq!(&got, &ev);
    let src = detect_source_ref(&got).expect("marker survives round-trip");
    assert_eq!(src.entity_id, rid);
    assert_eq!(src.kind, SourceKind::Reminder);
}

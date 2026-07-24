//! RFC 5545 (iCalendar) import and export.
//!
//! See `docs/casual-note-calendar.md` §2 / §9 (ICS interchange, lossless
//! round-trip acceptance gate). This module parses a `VCALENDAR` stream of
//! `VEVENT`s (with `RRULE`, all-day `DATE` vs `DATE-TIME`, `TZID`, `VALARM`,
//! `UID`, `SEQUENCE`, `LAST-MODIFIED`, and `RECURRENCE-ID` override instances)
//! into [`CalendarEvent`]s, and serializes them back. `TZID` values are resolved
//! against the bundled IANA database (`chrono-tz`); `VTIMEZONE` blocks are
//! tolerated and skipped (the offset is recomputed from the zone id).
//!
//! Round-trip contract: export then import reproduces the semantic event
//! (start/end instants, all-day flag, tz, recurrence, alarms, exceptions). It is
//! not guaranteed byte-identical — folding, property order, and `DURATION`→`DTEND`
//! normalization may differ — but no modeled field is lost.

use chrono::{DateTime, NaiveDate, NaiveDateTime, TimeZone, Utc};

use app_domain::{Id, Timestamp};

use crate::error::{CalendarError, CalendarResult};
use crate::model::{
    AlarmAction, AlarmTrigger, CalendarEvent, EventAlarm, EventStatus, RecurrenceId, SourceRef,
    Transparency,
};

/// The `PRODID` stamped on exported calendars.
pub const PRODID: &str = "-//Casual Note//Calendar//EN";
/// The custom property carrying the [`SourceRef`] back-link (doc §5).
pub const X_SOURCE: &str = "X-CASUAL-NOTE-SOURCE";

// ===========================================================================
// Public API
// ===========================================================================

/// Parse a `VCALENDAR` text stream into events, assigning each to `calendar_id`.
///
/// Non-`VEVENT` components (`VTIMEZONE`, `VTODO`, `VJOURNAL`, `VFREEBUSY`) are
/// skipped. Each `VEVENT` — including `RECURRENCE-ID` override instances that
/// share a `UID` with their master — becomes one [`CalendarEvent`].
pub fn parse_ics(input: &str, calendar_id: Id) -> CalendarResult<Vec<CalendarEvent>> {
    let lines = unfold(input);
    let mut events = Vec::new();
    let mut ev: Option<EventBuilder> = None;
    let mut alarm: Option<AlarmBuilder> = None;
    // Nesting depth inside components we deliberately skip (e.g. VTIMEZONE).
    let mut skip_depth: usize = 0;

    for (idx, raw) in lines.iter().enumerate() {
        let lineno = idx + 1;
        if raw.trim().is_empty() {
            continue;
        }
        let cl = parse_content_line(raw, lineno)?;
        let upper = cl.value.to_ascii_uppercase();
        match cl.name.as_str() {
            "BEGIN" if upper == "VCALENDAR" => { /* wrapper — no-op */ }
            "END" if upper == "VCALENDAR" => { /* wrapper — no-op */ }
            "BEGIN" if upper == "VEVENT" && skip_depth == 0 => {
                if ev.is_some() {
                    return Err(parse_err(lineno, "nested VEVENT is not allowed"));
                }
                ev = Some(EventBuilder::default());
            }
            "END" if upper == "VEVENT" && skip_depth == 0 => {
                let b = ev
                    .take()
                    .ok_or_else(|| parse_err(lineno, "END:VEVENT without BEGIN:VEVENT"))?;
                events.push(b.build(calendar_id, lineno)?);
            }
            "BEGIN" if upper == "VALARM" && skip_depth == 0 && ev.is_some() => {
                alarm = Some(AlarmBuilder::default());
            }
            "END" if upper == "VALARM" && skip_depth == 0 && alarm.is_some() => {
                let a = alarm
                    .take()
                    .ok_or_else(|| parse_err(lineno, "END:VALARM without BEGIN:VALARM"))?;
                if let Some(e) = ev.as_mut() {
                    e.alarms.push(a.build(lineno)?);
                }
            }
            "BEGIN" => skip_depth += 1,
            "END" => skip_depth = skip_depth.saturating_sub(1),
            _ => {
                if skip_depth > 0 {
                    continue;
                }
                if let Some(a) = alarm.as_mut() {
                    a.consume(&cl, lineno)?;
                } else if let Some(e) = ev.as_mut() {
                    e.consume(&cl, lineno)?;
                }
                // otherwise: a top-level VCALENDAR property (VERSION/PRODID) — ignore.
            }
        }
    }

    if ev.is_some() {
        return Err(parse_err(
            lines.len(),
            "unterminated VEVENT (missing END:VEVENT)",
        ));
    }
    Ok(events)
}

/// Serialize events into a single `VCALENDAR` document (CRLF line endings, folded
/// at 75 octets per RFC 5545 §3.1).
pub fn write_ics(events: &[CalendarEvent]) -> CalendarResult<String> {
    let mut out = String::new();
    push_folded(&mut out, "BEGIN:VCALENDAR");
    push_folded(&mut out, "VERSION:2.0");
    push_folded(&mut out, &format!("PRODID:{PRODID}"));
    push_folded(&mut out, "CALSCALE:GREGORIAN");
    for ev in events {
        write_vevent(&mut out, ev)?;
    }
    push_folded(&mut out, "END:VCALENDAR");
    Ok(out)
}

/// Serialize a single event as a `VEVENT` block (without the enclosing
/// `VCALENDAR`). Useful for CalDAV `PUT` bodies in the sync phase.
pub fn event_to_vevent(event: &CalendarEvent) -> CalendarResult<String> {
    let mut out = String::new();
    write_vevent(&mut out, event)?;
    Ok(out)
}

// ===========================================================================
// Serialization internals
// ===========================================================================

fn write_vevent(out: &mut String, ev: &CalendarEvent) -> CalendarResult<()> {
    push_folded(out, "BEGIN:VEVENT");
    push_folded(out, &format!("UID:{}", ev.uid));

    // DTSTAMP is required by RFC 5545; derive a stable-ish value (not modeled, so
    // it does not affect round-trip equality — the parser ignores it).
    let stamp = ev.last_modified.or(ev.created).unwrap_or(ev.start_utc);
    push_folded(out, &format!("DTSTAMP:{}", format_utc_basic(stamp)?));

    push_folded(out, &format!("SUMMARY:{}", escape_text(&ev.title)));

    let (sp, sv) = dt_parts(ev.start_utc, ev.all_day, &ev.tz)?;
    push_folded(out, &format!("DTSTART{sp}:{sv}"));
    let (ep, evl) = dt_parts(ev.end_utc, ev.all_day, &ev.tz)?;
    push_folded(out, &format!("DTEND{ep}:{evl}"));

    if let Some(loc) = &ev.location {
        push_folded(out, &format!("LOCATION:{}", escape_text(loc)));
    }
    if let Some(desc) = &ev.description {
        push_folded(out, &format!("DESCRIPTION:{}", escape_text(desc)));
    }
    push_folded(out, &format!("STATUS:{}", ev.status.as_ical()));
    push_folded(out, &format!("TRANSP:{}", ev.transparency.as_ical()));
    push_folded(out, &format!("SEQUENCE:{}", ev.sequence));

    if let Some(rule) = &ev.rrule {
        push_folded(out, &format!("RRULE:{rule}"));
    }
    if !ev.exdates.is_empty() {
        let mut vals = Vec::with_capacity(ev.exdates.len());
        for t in &ev.exdates {
            vals.push(format_utc_basic(*t)?);
        }
        push_folded(out, &format!("EXDATE:{}", vals.join(",")));
    }
    if let Some(rid) = &ev.recurrence_id {
        let (rp, rv) = dt_parts(rid.instant, rid.all_day, &rid.tz)?;
        let range = if rid.this_and_future {
            ";RANGE=THISANDFUTURE"
        } else {
            ""
        };
        push_folded(out, &format!("RECURRENCE-ID{rp}{range}:{rv}"));
    }
    if let Some(c) = ev.created {
        push_folded(out, &format!("CREATED:{}", format_utc_basic(c)?));
    }
    if let Some(m) = ev.last_modified {
        push_folded(out, &format!("LAST-MODIFIED:{}", format_utc_basic(m)?));
    }
    if let Some(sr) = &ev.source_ref {
        push_folded(out, &format!("{X_SOURCE}:{}", sr.marker_value()));
    }
    for a in &ev.alarms {
        write_valarm(out, a)?;
    }
    push_folded(out, "END:VEVENT");
    Ok(())
}

fn write_valarm(out: &mut String, a: &EventAlarm) -> CalendarResult<()> {
    push_folded(out, "BEGIN:VALARM");
    push_folded(out, &format!("ACTION:{}", a.action.as_ical()));
    match a.trigger {
        AlarmTrigger::Relative {
            offset_secs,
            related_end,
        } => {
            let rel = if related_end { ";RELATED=END" } else { "" };
            push_folded(
                out,
                &format!("TRIGGER{rel}:{}", format_duration(offset_secs)),
            );
        }
        AlarmTrigger::Absolute(ts) => {
            push_folded(
                out,
                &format!("TRIGGER;VALUE=DATE-TIME:{}", format_utc_basic(ts)?),
            );
        }
    }
    if let Some(d) = &a.description {
        push_folded(out, &format!("DESCRIPTION:{}", escape_text(d)));
    }
    if let Some(s) = &a.summary {
        push_folded(out, &format!("SUMMARY:{}", escape_text(s)));
    }
    if let Some(r) = a.repeat {
        push_folded(out, &format!("REPEAT:{r}"));
    }
    if let Some(iv) = a.repeat_interval_secs {
        push_folded(out, &format!("DURATION:{}", format_duration(iv)));
    }
    push_folded(out, "END:VALARM");
    Ok(())
}

// ===========================================================================
// Content-line parsing
// ===========================================================================

/// One unfolded content line: `NAME;PARAM=val:value`.
struct ContentLine {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

impl ContentLine {
    /// Case-insensitive lookup of the first parameter with `name`.
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

fn parse_content_line(line: &str, lineno: usize) -> CalendarResult<ContentLine> {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i] != b';' && bytes[i] != b':' {
        i += 1;
    }
    if i == 0 {
        return Err(parse_err(lineno, "content line has an empty property name"));
    }
    let name = line[..i].to_ascii_uppercase();
    let mut params = Vec::new();

    while i < bytes.len() && bytes[i] == b';' {
        i += 1; // consume ';'
        let pstart = i;
        while i < bytes.len() && bytes[i] != b'=' && bytes[i] != b';' && bytes[i] != b':' {
            i += 1;
        }
        let pname = line[pstart..i].to_ascii_uppercase();
        let mut pval = String::new();
        if i < bytes.len() && bytes[i] == b'=' {
            i += 1; // consume '='
            let vstart = i;
            let mut in_quote = false;
            while i < bytes.len() {
                let c = bytes[i];
                if c == b'"' {
                    in_quote = !in_quote;
                } else if !in_quote && (c == b';' || c == b':') {
                    break;
                }
                i += 1;
            }
            pval = line[vstart..i].trim_matches('"').to_string();
        }
        params.push((pname, pval));
    }

    if i < bytes.len() && bytes[i] == b':' {
        i += 1; // consume ':'
        Ok(ContentLine {
            name,
            params,
            value: line[i..].to_string(),
        })
    } else {
        Err(parse_err(
            lineno,
            "content line is missing its ':' value separator",
        ))
    }
}

// ===========================================================================
// Builders
// ===========================================================================

#[derive(Default)]
struct EventBuilder {
    uid: Option<String>,
    title: Option<String>,
    dtstart: Option<(Timestamp, bool, Option<String>)>,
    dtend: Option<(Timestamp, bool, Option<String>)>,
    duration: Option<i64>,
    rrule: Option<String>,
    exdates: Vec<Timestamp>,
    location: Option<String>,
    description: Option<String>,
    status: Option<EventStatus>,
    transp: Option<Transparency>,
    sequence: Option<u32>,
    created: Option<Timestamp>,
    last_modified: Option<Timestamp>,
    recurrence_id: Option<RecurrenceId>,
    source_ref: Option<SourceRef>,
    alarms: Vec<EventAlarm>,
}

impl EventBuilder {
    fn consume(&mut self, cl: &ContentLine, lineno: usize) -> CalendarResult<()> {
        match cl.name.as_str() {
            "UID" => self.uid = Some(cl.value.clone()),
            "SUMMARY" => self.title = Some(unescape_text(&cl.value)),
            "DTSTART" => self.dtstart = Some(parse_datetime(&cl.value, cl, lineno)?),
            "DTEND" => self.dtend = Some(parse_datetime(&cl.value, cl, lineno)?),
            "DURATION" => self.duration = Some(parse_duration(&cl.value, lineno)?),
            "RRULE" => self.rrule = Some(cl.value.clone()),
            "EXDATE" => {
                for part in cl.value.split(',') {
                    let (ts, _, _) = parse_datetime(part, cl, lineno)?;
                    self.exdates.push(ts);
                }
            }
            "LOCATION" => self.location = Some(unescape_text(&cl.value)),
            "DESCRIPTION" => self.description = Some(unescape_text(&cl.value)),
            "STATUS" => self.status = Some(EventStatus::from_ical(&cl.value)),
            "TRANSP" => self.transp = Some(Transparency::from_ical(&cl.value)),
            "SEQUENCE" => {
                self.sequence = Some(
                    cl.value
                        .trim()
                        .parse()
                        .map_err(|_| parse_err(lineno, "SEQUENCE is not a valid integer"))?,
                );
            }
            "CREATED" => self.created = Some(parse_datetime(&cl.value, cl, lineno)?.0),
            "LAST-MODIFIED" => self.last_modified = Some(parse_datetime(&cl.value, cl, lineno)?.0),
            "RECURRENCE-ID" => {
                let (ts, all_day, tz) = parse_datetime(&cl.value, cl, lineno)?;
                let this_and_future = cl
                    .param("RANGE")
                    .is_some_and(|r| r.eq_ignore_ascii_case("THISANDFUTURE"));
                self.recurrence_id = Some(RecurrenceId {
                    instant: ts,
                    all_day,
                    tz,
                    this_and_future,
                });
            }
            n if n.eq_ignore_ascii_case(X_SOURCE) => {
                self.source_ref = SourceRef::parse_marker(cl.value.trim());
            }
            _ => { /* unmodeled property — ignored, not an error */ }
        }
        Ok(())
    }

    fn build(self, calendar_id: Id, lineno: usize) -> CalendarResult<CalendarEvent> {
        let uid = self
            .uid
            .ok_or_else(|| parse_err(lineno, "VEVENT is missing the required UID"))?;
        let (start_utc, all_day, tz) = self
            .dtstart
            .ok_or_else(|| parse_err(lineno, "VEVENT is missing the required DTSTART"))?;

        // Resolve the end: explicit DTEND, else DTSTART+DURATION, else a default
        // (one day for all-day events, zero-length otherwise).
        let end_utc = if let Some((e, _, _)) = self.dtend {
            e
        } else if let Some(d) = self.duration {
            Timestamp::from_millis(start_utc.as_millis() + d * 1000)
        } else if all_day {
            Timestamp::from_millis(start_utc.as_millis() + 86_400_000)
        } else {
            start_utc
        };

        Ok(CalendarEvent {
            id: Id::new(),
            calendar_id,
            uid,
            title: self.title.unwrap_or_default(),
            start_utc,
            end_utc,
            all_day,
            tz,
            rrule: self.rrule,
            exdates: self.exdates,
            location: self.location,
            description: self.description,
            status: self.status.unwrap_or_default(),
            transparency: self.transp.unwrap_or_default(),
            sequence: self.sequence.unwrap_or(0),
            created: self.created,
            last_modified: self.last_modified,
            etag: None,
            recurrence_id: self.recurrence_id,
            source_ref: self.source_ref,
            alarms: self.alarms,
        })
    }
}

#[derive(Default)]
struct AlarmBuilder {
    action: Option<AlarmAction>,
    trigger: Option<AlarmTrigger>,
    description: Option<String>,
    summary: Option<String>,
    repeat: Option<u32>,
    repeat_interval_secs: Option<i64>,
}

impl AlarmBuilder {
    fn consume(&mut self, cl: &ContentLine, lineno: usize) -> CalendarResult<()> {
        match cl.name.as_str() {
            "ACTION" => self.action = Some(AlarmAction::from_ical(&cl.value)),
            "TRIGGER" => self.trigger = Some(parse_trigger(cl, lineno)?),
            "DESCRIPTION" => self.description = Some(unescape_text(&cl.value)),
            "SUMMARY" => self.summary = Some(unescape_text(&cl.value)),
            "REPEAT" => {
                self.repeat = Some(
                    cl.value
                        .trim()
                        .parse()
                        .map_err(|_| parse_err(lineno, "VALARM REPEAT is not a valid integer"))?,
                );
            }
            "DURATION" => self.repeat_interval_secs = Some(parse_duration(&cl.value, lineno)?),
            _ => { /* ignored */ }
        }
        Ok(())
    }

    fn build(self, lineno: usize) -> CalendarResult<EventAlarm> {
        let trigger = self
            .trigger
            .ok_or_else(|| parse_err(lineno, "VALARM is missing the required TRIGGER"))?;
        Ok(EventAlarm {
            action: self.action.unwrap_or(AlarmAction::Display),
            trigger,
            description: self.description,
            summary: self.summary,
            repeat: self.repeat,
            repeat_interval_secs: self.repeat_interval_secs,
        })
    }
}

fn parse_trigger(cl: &ContentLine, lineno: usize) -> CalendarResult<AlarmTrigger> {
    let is_datetime = cl
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE-TIME"))
        || cl.value.trim_end_matches(['Z', 'z']).contains('T') && cl.value.len() >= 15;
    if is_datetime {
        let (ts, _, _) = parse_datetime(&cl.value, cl, lineno)?;
        Ok(AlarmTrigger::Absolute(ts))
    } else {
        let related_end = cl
            .param("RELATED")
            .is_some_and(|r| r.eq_ignore_ascii_case("END"));
        let offset_secs = parse_duration(&cl.value, lineno)?;
        Ok(AlarmTrigger::Relative {
            offset_secs,
            related_end,
        })
    }
}

// ===========================================================================
// Date-time <-> RFC 5545 value conversion
// ===========================================================================

/// Parse an RFC 5545 DATE or DATE-TIME value into `(instant, all_day, tz)`.
fn parse_datetime(
    value: &str,
    cl: &ContentLine,
    lineno: usize,
) -> CalendarResult<(Timestamp, bool, Option<String>)> {
    let value = value.trim();
    let is_date = cl
        .param("VALUE")
        .is_some_and(|v| v.eq_ignore_ascii_case("DATE"))
        || (!value.contains('T') && value.len() == 8);

    if is_date {
        let d = NaiveDate::parse_from_str(value, "%Y%m%d")
            .map_err(|e| parse_err(lineno, &format!("invalid DATE value '{value}': {e}")))?;
        let ndt = d
            .and_hms_opt(0, 0, 0)
            .ok_or_else(|| parse_err(lineno, "DATE produced an invalid midnight instant"))?;
        return Ok((Timestamp::from_millis(utc_ms(&ndt)), true, None));
    }

    let has_z = value.ends_with('Z') || value.ends_with('z');
    let core = value.trim_end_matches(['Z', 'z']);
    let ndt = NaiveDateTime::parse_from_str(core, "%Y%m%dT%H%M%S")
        .map_err(|e| parse_err(lineno, &format!("invalid DATE-TIME value '{value}': {e}")))?;

    if has_z {
        Ok((
            Timestamp::from_millis(utc_ms(&ndt)),
            false,
            Some("UTC".into()),
        ))
    } else if let Some(tzid) = cl.param("TZID") {
        let tz: chrono_tz::Tz = tzid
            .parse()
            .map_err(|_| CalendarError::UnknownTimezone(tzid.to_string()))?;
        let dt = resolve_local(ndt, tz, lineno)?;
        Ok((
            Timestamp::from_millis(dt.with_timezone(&Utc).timestamp_millis()),
            false,
            Some(tzid.to_string()),
        ))
    } else {
        // Floating local time: interpret as UTC wall-clock, no tz recorded.
        Ok((Timestamp::from_millis(utc_ms(&ndt)), false, None))
    }
}

/// Render `(instant, all_day, tz)` back to an RFC 5545 `(params, value)` pair.
fn dt_parts(ts: Timestamp, all_day: bool, tz: &Option<String>) -> CalendarResult<(String, String)> {
    let dt = utc_dt(ts)?;
    if all_day {
        return Ok((";VALUE=DATE".to_string(), dt.format("%Y%m%d").to_string()));
    }
    match tz.as_deref() {
        Some("UTC") => Ok((String::new(), dt.format("%Y%m%dT%H%M%SZ").to_string())),
        Some(other) => {
            let zone: chrono_tz::Tz = other
                .parse()
                .map_err(|_| CalendarError::UnknownTimezone(other.to_string()))?;
            let local = dt.with_timezone(&zone);
            Ok((
                format!(";TZID={other}"),
                local.format("%Y%m%dT%H%M%S").to_string(),
            ))
        }
        None => Ok((String::new(), dt.format("%Y%m%dT%H%M%S").to_string())),
    }
}

/// UTC basic-format string with the `Z` suffix (e.g. `20260723T090000Z`).
pub(crate) fn format_utc_basic(ts: Timestamp) -> CalendarResult<String> {
    Ok(utc_dt(ts)?.format("%Y%m%dT%H%M%SZ").to_string())
}

fn utc_ms(ndt: &NaiveDateTime) -> i64 {
    Utc.from_utc_datetime(ndt).timestamp_millis()
}

fn utc_dt(ts: Timestamp) -> CalendarResult<DateTime<Utc>> {
    Utc.timestamp_millis_opt(ts.as_millis())
        .single()
        .ok_or_else(|| {
            CalendarError::InvalidDateTime(format!("epoch-ms {} out of range", ts.as_millis()))
        })
}

/// Resolve a naive local time in `tz`, choosing the earliest instant on fall-back
/// ambiguity and nudging forward across a spring-forward gap.
fn resolve_local(
    ndt: NaiveDateTime,
    tz: chrono_tz::Tz,
    lineno: usize,
) -> CalendarResult<DateTime<chrono_tz::Tz>> {
    use chrono::offset::LocalResult;
    match ndt.and_local_timezone(tz) {
        LocalResult::Single(dt) => Ok(dt),
        LocalResult::Ambiguous(earlier, _later) => Ok(earlier),
        LocalResult::None => {
            let shifted = ndt
                .checked_add_signed(chrono::Duration::hours(1))
                .ok_or_else(|| parse_err(lineno, "local time overflow resolving DST gap"))?;
            match shifted.and_local_timezone(tz) {
                LocalResult::Single(dt) => Ok(dt),
                LocalResult::Ambiguous(dt, _) => Ok(dt),
                LocalResult::None => Err(parse_err(
                    lineno,
                    "local time falls in an unresolvable DST gap",
                )),
            }
        }
    }
}

// ===========================================================================
// ISO 8601 duration (RFC 5545 §3.3.6)
// ===========================================================================

fn parse_duration(s: &str, lineno: usize) -> CalendarResult<i64> {
    let s = s.trim();
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') {
        (-1i64, r)
    } else {
        (1i64, s.strip_prefix('+').unwrap_or(s))
    };
    let rest = rest
        .strip_prefix('P')
        .ok_or_else(|| parse_err(lineno, &format!("duration '{s}' must start with 'P'")))?;

    let mut secs: i64 = 0;
    let mut num = String::new();
    for c in rest.chars() {
        match c {
            'T' => { /* date/time separator — no value */ }
            '0'..='9' => num.push(c),
            'W' => secs += take_num(&mut num, lineno)? * 604_800,
            'D' => secs += take_num(&mut num, lineno)? * 86_400,
            'H' => secs += take_num(&mut num, lineno)? * 3_600,
            'M' => secs += take_num(&mut num, lineno)? * 60,
            'S' => secs += take_num(&mut num, lineno)?,
            other => {
                return Err(parse_err(
                    lineno,
                    &format!("unexpected char '{other}' in duration '{s}'"),
                ))
            }
        }
    }
    Ok(sign * secs)
}

fn take_num(num: &mut String, lineno: usize) -> CalendarResult<i64> {
    let n = num
        .parse::<i64>()
        .map_err(|_| parse_err(lineno, "malformed number in duration"))?;
    num.clear();
    Ok(n)
}

fn format_duration(total: i64) -> String {
    let sign = if total < 0 { "-" } else { "" };
    let mut rem = total.abs();
    let days = rem / 86_400;
    rem %= 86_400;
    let hours = rem / 3_600;
    rem %= 3_600;
    let mins = rem / 60;
    let secs = rem % 60;

    let mut out = format!("{sign}P");
    if days > 0 {
        out.push_str(&format!("{days}D"));
    }
    // Emit a time section when there is any sub-day component, or when the whole
    // value is zero (canonical "PT0S").
    if hours > 0 || mins > 0 || secs > 0 || (days == 0) {
        out.push('T');
        if hours > 0 {
            out.push_str(&format!("{hours}H"));
        }
        if mins > 0 {
            out.push_str(&format!("{mins}M"));
        }
        if secs > 0 || (hours == 0 && mins == 0) {
            out.push_str(&format!("{secs}S"));
        }
    }
    out
}

// ===========================================================================
// TEXT escaping (RFC 5545 §3.3.11) and line folding/unfolding (§3.1)
// ===========================================================================

fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            ';' => out.push_str("\\;"),
            ',' => out.push_str("\\,"),
            '\n' => out.push_str("\\n"),
            '\r' => { /* drop bare CR; CRLF handled by folding */ }
            _ => out.push(c),
        }
    }
    out
}

fn unescape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n' | 'N') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Unfold RFC 5545 content lines: a leading space/tab continues the prior line.
fn unfold(input: &str) -> Vec<String> {
    let normalized = input.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = Vec::new();
    for raw in normalized.split('\n') {
        if let Some(rest) = raw.strip_prefix(' ').or_else(|| raw.strip_prefix('\t')) {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
                continue;
            }
        }
        lines.push(raw.to_string());
    }
    lines
}

/// Append `line`, folded to <=75 octets, terminated with CRLF (RFC 5545 §3.1).
fn push_folded(out: &mut String, line: &str) {
    let bytes = line.len();
    if bytes <= 75 {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }
    let mut start = 0;
    let mut first = true;
    while start < line.len() {
        // First line takes 75 octets; continuations reserve 1 for the leading space.
        let budget = if first { 75 } else { 74 };
        let mut end = (start + budget).min(line.len());
        while end > start && !line.is_char_boundary(end) {
            end -= 1;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&line[start..end]);
        out.push_str("\r\n");
        start = end;
        first = false;
    }
}

fn parse_err(lineno: usize, message: &str) -> CalendarError {
    CalendarError::IcsParse {
        line: Some(lineno),
        message: message.to_string(),
    }
}

//! Reminder create / snooze / cancel / upcoming (HLD §6, Data Model §7). The
//! `reminder` row is the durable truth; the host lifts the returned
//! [`ScheduleRequest`] into `scheduler::ScheduledReminder` to arm Layer A and, on
//! macOS/Windows, the Layer-B OS one-shot. **Linux has no OS layer** — reported
//! honestly via `os_layer=false` (HLD §9.3, CLAUDE.md capability honesty).

use app_domain::{AppError, AppEvent, AppResult, EntityRef, Id, Timestamp};
use rusqlite::{params, OptionalExtension, Row};
use serde_json::Value;
use storage::DetailTable;

use crate::dto::{NewReminder, ReminderView, ScheduleRequest};
use crate::notes::parse_id;
use crate::util::{self, Columns};
use crate::Service;

/// Whether this platform has an OS one-shot notification layer (Layer B). False on
/// Linux — never faked (HLD §9.3).
const fn has_os_layer() -> bool {
    cfg!(any(target_os = "macos", target_os = "windows"))
}

impl Service {
    /// `reminders.create` — persist a pending reminder; return its id + the schedule
    /// descriptor for the host to arm both layers.
    pub fn reminders_create(&self, input: NewReminder) -> AppResult<(String, ScheduleRequest)> {
        let id = Id::new();
        let now = self.now_ms();

        let mut cols = Columns::new();
        cols.insert("fire_at".into(), Value::Number(input.fire_at.into()));
        cols.insert("tz".into(), Value::String(input.tz.clone()));
        cols.insert("state".into(), Value::String("pending".into()));
        cols.insert("created_at".into(), Value::Number(now.into()));
        if let Some(t) = &input.target {
            cols.insert("target_kind".into(), Value::String(t.kind.as_str().into()));
            cols.insert("target_id".into(), Value::String(t.id.to_string()));
        }
        if let Some(b) = &input.body {
            cols.insert("body".into(), Value::String(b.clone()));
        }

        let title = input.body.clone().or_else(|| Some("Reminder".to_string()));
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "reminder",
            title,
            None,
            now,
            Some((DetailTable::Reminder, cols)),
        ))?;

        self.emit(AppEvent::ReminderScheduled {
            reminder_id: id,
            fire_at: Timestamp::from_millis(input.fire_at),
            os_layer: has_os_layer(),
        });

        Ok((
            id.to_string(),
            ScheduleRequest {
                reminder_id: id.to_string(),
                fire_at: input.fire_at,
                tz: input.tz,
                body: input.body,
                target: input.target,
                os_layer: has_os_layer(),
            },
        ))
    }

    /// `reminders.snooze` — re-arm at `until`; clears any stored OS handle first.
    pub fn reminders_snooze(&self, reminder_id: &str, until: i64) -> AppResult<ReminderView> {
        let id = parse_id(reminder_id)?;
        let now = self.now_ms();
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("reminder {reminder_id}")))?;

        let mut cols = Columns::new();
        cols.insert("state".into(), Value::String("snoozed".into()));
        cols.insert("snoozed_until".into(), Value::Number(until.into()));
        cols.insert("os_handle".into(), Value::Null);
        cols.insert("os_layer".into(), Value::Null);
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            None,
            now,
            Some((DetailTable::Reminder, cols)),
        ))?;

        self.emit(AppEvent::ReminderScheduled {
            reminder_id: id,
            fire_at: Timestamp::from_millis(until),
            os_layer: has_os_layer(),
        });
        self.read_reminder(id)
    }

    /// `reminders.cancel` — terminal cancel; clears the OS handle.
    pub fn reminders_cancel(&self, reminder_id: &str) -> AppResult<()> {
        let id = parse_id(reminder_id)?;
        let now = self.now_ms();
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("reminder {reminder_id}")))?;
        let mut cols = Columns::new();
        cols.insert("state".into(), Value::String("canceled".into()));
        cols.insert("os_handle".into(), Value::Null);
        cols.insert("os_layer".into(), Value::Null);
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            None,
            now,
            Some((DetailTable::Reminder, cols)),
        ))?;
        Ok(())
    }

    /// `reminders.upcoming` — pending reminders within `horizon_days` (default 14).
    pub fn reminders_upcoming(&self, horizon_days: Option<i64>) -> AppResult<Vec<ReminderView>> {
        let horizon = horizon_days.unwrap_or(14).max(0);
        let cutoff = self.now_ms() + horizon * 86_400_000;
        self.read(move |c| {
            let mut stmt = c.prepare(
                "SELECT entity_id, target_kind, target_id, fire_at, tz, state, snoozed_until, body \
                 FROM reminder WHERE state = 'pending' AND fire_at <= ?1 ORDER BY fire_at ASC",
            )?;
            let rows = stmt
                .query_map(params![cutoff], map_reminder_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// Load every active (pending/snoozed) reminder as a [`ScheduleRequest`] set —
    /// used by the host to rebuild Layer A on boot (HLD §8.3).
    pub fn active_schedule(&self) -> AppResult<Vec<ScheduleRequest>> {
        self.read(|c| {
            let mut stmt = c.prepare(
                "SELECT entity_id, target_kind, target_id, fire_at, tz, snoozed_until, body, state \
                 FROM reminder WHERE state IN ('pending','snoozed')",
            )?;
            let rows = stmt
                .query_map([], |r| {
                    let id: Vec<u8> = r.get(0)?;
                    let target_kind: Option<String> = r.get(1)?;
                    let target_id: Option<Vec<u8>> = r.get(2)?;
                    let fire_at: i64 = r.get(3)?;
                    let tz: String = r.get(4)?;
                    let snoozed: Option<i64> = r.get(5)?;
                    let body: Option<String> = r.get(6)?;
                    Ok(ScheduleRequest {
                        reminder_id: Id::from_bytes(to16(&id)).to_string(),
                        fire_at: snoozed.unwrap_or(fire_at),
                        tz,
                        body,
                        target: target_ref(target_kind.as_deref(), target_id.as_deref()),
                        os_layer: has_os_layer(),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    fn read_reminder(&self, id: Id) -> AppResult<ReminderView> {
        self.read(move |c| {
            c.query_row(
                "SELECT entity_id, target_kind, target_id, fire_at, tz, state, snoozed_until, body \
                 FROM reminder WHERE entity_id = ?1",
                params![id.as_bytes().as_slice()],
                map_reminder_row,
            )
            .optional()
            .map_err(Into::into)
        })?
        .ok_or_else(|| AppError::NotFound(format!("reminder {id}")))
    }
}

fn map_reminder_row(r: &Row<'_>) -> rusqlite::Result<ReminderView> {
    Ok(ReminderView {
        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)).to_string(),
        target_kind: r.get(1)?,
        target_id: r
            .get::<_, Option<Vec<u8>>>(2)?
            .map(|b| Id::from_bytes(to16(&b)).to_string()),
        fire_at: r.get(3)?,
        tz: r.get(4)?,
        state: r.get(5)?,
        snoozed_until: r.get(6)?,
        body: r.get(7)?,
    })
}

fn target_ref(kind: Option<&str>, id: Option<&[u8]>) -> Option<EntityRef> {
    let kind = app_domain::EntityKind::from_db_str(kind?)?;
    let id = Id::from_bytes(to16(id?));
    Some(EntityRef::new(kind, id))
}

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}

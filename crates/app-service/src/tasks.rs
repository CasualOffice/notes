//! Task / project / area workflows (HLD §6, Data Model §6). Buckets are **derived
//! queries** (`tasks::QueryBucket`), never stored. Reorder is a fractional-index
//! midpoint (`tasks::order_key`). Status changes flow through the pure
//! `tasks::transition` helpers before being committed as op-log entries.

use app_domain::{AppError, AppEvent, AppResult, Bucket, Id};
use rusqlite::{named_params, params, OptionalExtension, Row};
use serde_json::Value;
use storage::DetailTable;
use tasks::bucket::{LIVE_FILTER, TASK_FROM, TASK_SELECT_COLUMNS};
use tasks::{QueryBucket, TaskStatus};

use crate::dto::{NewTask, TaskPatch, TaskView};
use crate::notes::parse_id;
use crate::util::{self, Columns};
use crate::Service;

impl Service {
    /// `tasks.create`.
    pub fn tasks_create(&self, input: NewTask) -> AppResult<TaskView> {
        let id = Id::new();
        let now = self.now_ms();
        let order_key = self.next_order_key()?;

        let mut cols = Columns::new();
        cols.insert("status".into(), Value::String("open".into()));
        cols.insert("order_key".into(), Value::String(order_key));
        cols.insert(
            "priority".into(),
            Value::Number(input.priority.unwrap_or(0).clamp(0, 3).into()),
        );
        cols.insert(
            "someday".into(),
            Value::Bool(input.someday.unwrap_or(false)),
        );
        if let Some(v) = &input.notes_md {
            cols.insert("notes_md".into(), Value::String(v.clone()));
        }
        if let Some(v) = &input.start_on {
            cols.insert("start_on".into(), Value::String(v.clone()));
        }
        if let Some(v) = &input.deadline_on {
            cols.insert("deadline_on".into(), Value::String(v.clone()));
        }
        if let Some(v) = &input.project_id {
            parse_id(v)?;
            cols.insert("project_id".into(), Value::String(v.clone()));
        }
        if let Some(v) = &input.area_id {
            parse_id(v)?;
            cols.insert("area_id".into(), Value::String(v.clone()));
        }

        let title = (!input.title.trim().is_empty()).then(|| input.title.clone());
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "task",
            title,
            None,
            now,
            Some((DetailTable::Task, cols)),
        ))?;

        let view = self.read_task(id)?;
        self.emit(AppEvent::TaskChanged {
            task_id: id,
            bucket_hint: bucket_of(&view),
        });
        Ok(view)
    }

    /// `tasks.update` — per-field patch (LWW-by-HLC at the writer).
    pub fn tasks_update(&self, task_id: &str, patch: TaskPatch) -> AppResult<TaskView> {
        let id = parse_id(task_id)?;
        let now = self.now_ms();
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("task {task_id}")))?;
        if spine.kind != "task" {
            return Err(AppError::Validation(format!("{task_id} is not a task")));
        }

        let mut cols = Columns::new();
        if let Some(v) = &patch.notes_md {
            cols.insert("notes_md".into(), Value::String(v.clone()));
        }
        if let Some(v) = patch.priority {
            cols.insert("priority".into(), Value::Number(v.clamp(0, 3).into()));
        }
        if let Some(v) = patch.someday {
            cols.insert("someday".into(), Value::Bool(v));
        }
        if let Some(v) = &patch.start_on {
            cols.insert("start_on".into(), Value::String(v.clone()));
        }
        if let Some(v) = &patch.deadline_on {
            cols.insert("deadline_on".into(), Value::String(v.clone()));
        }
        if let Some(v) = &patch.project_id {
            parse_id(v)?;
            cols.insert("project_id".into(), Value::String(v.clone()));
        }
        if let Some(v) = &patch.area_id {
            parse_id(v)?;
            cols.insert("area_id".into(), Value::String(v.clone()));
        }
        if let Some(s) = &patch.status {
            let status = TaskStatus::from_db_str(s)
                .ok_or_else(|| AppError::Validation(format!("bad task status {s}")))?;
            cols.insert("status".into(), Value::String(status.as_str().into()));
            cols.insert(
                "completed_at".into(),
                if status.is_closed() {
                    Value::Number(now.into())
                } else {
                    Value::Null
                },
            );
        }

        let new_title = patch.title.clone();
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            new_title,
            now,
            Some((DetailTable::Task, cols)),
        ))?;

        let view = self.read_task(id)?;
        self.emit(AppEvent::TaskChanged {
            task_id: id,
            bucket_hint: bucket_of(&view),
        });
        Ok(view)
    }

    /// `tasks.complete` — close a task (advance recurrence deferred to a later phase).
    pub fn tasks_complete(&self, task_id: &str, at: Option<i64>) -> AppResult<TaskView> {
        let id = parse_id(task_id)?;
        let now = at.unwrap_or_else(|| self.now_ms());
        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("task {task_id}")))?;

        let change = tasks::complete_task(app_domain::Timestamp::from_millis(now));
        let mut cols = Columns::new();
        cols.insert(
            "status".into(),
            Value::String(change.status.as_str().into()),
        );
        cols.insert(
            "completed_at".into(),
            Value::Number(change.completed_at.map_or(now, |t| t.as_millis()).into()),
        );
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            None,
            now,
            Some((DetailTable::Task, cols)),
        ))?;

        let view = self.read_task(id)?;
        // Recurrence spawning (Data Model §7) is a later-phase wiring.
        self.emit(AppEvent::TaskCompleted {
            task_id: id,
            recurrence_spawned: None,
        });
        Ok(view)
    }

    /// `tasks.reorder` — fractional-index midpoint between two neighbours (by id).
    pub fn tasks_reorder(
        &self,
        task_id: &str,
        before: Option<String>,
        after: Option<String>,
    ) -> AppResult<String> {
        let id = parse_id(task_id)?;
        let now = self.now_ms();
        let before_key = self.opt_order_key(before)?;
        let after_key = self.opt_order_key(after)?;
        let new_key = tasks::key_between(before_key.as_deref(), after_key.as_deref())
            .map_err(AppError::from)?;

        let spine = self
            .read(|c| Ok(util::read_spine(c, id)?))?
            .ok_or_else(|| AppError::NotFound(format!("task {task_id}")))?;
        self.commit(&util::update_op(
            id,
            self.next_hlc(),
            &spine,
            None,
            now,
            Some((
                DetailTable::Task,
                util::col1("order_key", Value::String(new_key.clone())),
            )),
        ))?;
        self.emit(AppEvent::TaskChanged {
            task_id: id,
            bucket_hint: None,
        });
        Ok(new_key)
    }

    /// `tasks.bucket` — the derived Today/Upcoming/Anytime/Someday query.
    pub fn tasks_bucket(&self, bucket: Bucket) -> AppResult<Vec<TaskView>> {
        let qb = QueryBucket::from(bucket);
        let sql = qb.sql();
        let today = util::today_local();
        self.read(move |c| {
            let mut stmt = c.prepare(&sql)?;
            let rows = stmt
                .query_map(named_params! { ":today": today }, map_task_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
    }

    /// `projects.create`.
    pub fn projects_create(&self, name: String, area_id: Option<String>) -> AppResult<String> {
        let id = Id::new();
        let now = self.now_ms();
        let order_key = tasks::order_key::initial_key();
        let mut cols = Columns::new();
        cols.insert("status".into(), Value::String("active".into()));
        cols.insert("order_key".into(), Value::String(order_key));
        if let Some(a) = &area_id {
            parse_id(a)?;
            cols.insert("area_id".into(), Value::String(a.clone()));
        }
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "project",
            (!name.trim().is_empty()).then_some(name),
            None,
            now,
            Some((DetailTable::Project, cols)),
        ))?;
        self.emit(AppEvent::ProjectChanged { project_id: id });
        Ok(id.to_string())
    }

    /// `areas.create`.
    pub fn areas_create(&self, name: String, icon: Option<String>) -> AppResult<String> {
        let id = Id::new();
        let now = self.now_ms();
        let mut cols = Columns::new();
        cols.insert(
            "order_key".into(),
            Value::String(tasks::order_key::initial_key()),
        );
        if let Some(i) = icon {
            cols.insert("icon".into(), Value::String(i));
        }
        self.commit(&util::create_op(
            id,
            self.next_hlc(),
            "area",
            (!name.trim().is_empty()).then_some(name),
            None,
            now,
            Some((DetailTable::Area, cols)),
        ))?;
        Ok(id.to_string())
    }

    // -- internal helpers ----------------------------------------------------

    fn read_task(&self, id: Id) -> AppResult<TaskView> {
        let sql = format!(
            "SELECT {TASK_SELECT_COLUMNS} {TASK_FROM} WHERE {LIVE_FILTER} AND t.entity_id = ?1"
        );
        self.read(move |c| {
            c.query_row(&sql, params![id.as_bytes().as_slice()], map_task_row)
                .optional()
                .map_err(Into::into)
        })?
        .ok_or_else(|| AppError::NotFound(format!("task {id}")))
    }

    /// The order_key for a new task: after the current maximum open key.
    fn next_order_key(&self) -> AppResult<String> {
        let max: Option<String> = self.read(|c| {
            c.query_row(
                "SELECT t.order_key FROM task t JOIN entity e ON e.id = t.entity_id \
                 WHERE e.deleted_at IS NULL AND t.status = 'open' \
                 ORDER BY t.order_key DESC LIMIT 1",
                [],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
        })?;
        Ok(match max {
            Some(k) => tasks::key_after(&k).map_err(AppError::from)?,
            None => tasks::order_key::initial_key(),
        })
    }

    fn opt_order_key(&self, task_id: Option<String>) -> AppResult<Option<String>> {
        let Some(tid) = task_id else { return Ok(None) };
        let id = parse_id(&tid)?;
        self.read(|c| {
            c.query_row(
                "SELECT order_key FROM task WHERE entity_id = ?1",
                params![id.as_bytes().as_slice()],
                |r| r.get::<_, String>(0),
            )
            .optional()
            .map_err(Into::into)
        })
    }
}

/// Map a full task+spine row (column order = [`TASK_SELECT_COLUMNS`]) to a view.
fn map_task_row(r: &Row<'_>) -> rusqlite::Result<TaskView> {
    let blob = |i: usize| -> rusqlite::Result<Option<String>> {
        Ok(r.get::<_, Option<Vec<u8>>>(i)?
            .map(|b| Id::from_bytes(to16(&b)).to_string()))
    };
    Ok(TaskView {
        id: Id::from_bytes(to16(&r.get::<_, Vec<u8>>(0)?)).to_string(),
        title: r.get(1)?,
        project_id: blob(2)?,
        area_id: blob(3)?,
        notes_md: r.get(6)?,
        status: r.get(7)?,
        priority: r.get(8)?,
        someday: r.get::<_, i64>(9)? != 0,
        start_on: r.get(10)?,
        deadline_on: r.get(11)?,
        completed_at: r.get(12)?,
        order_key: r.get(13)?,
    })
}

/// The live bucket a view belongs to (mirrors the SQL partition), for `bucket_hint`.
fn bucket_of(view: &TaskView) -> Option<Bucket> {
    let today: app_domain::Day = util::today_local().parse().ok()?;
    let day = |o: &Option<String>| o.as_deref().and_then(|s| s.parse::<app_domain::Day>().ok());
    let task = tasks::domain::Task {
        entity_id: view.id.parse().ok()?,
        project_id: None,
        area_id: None,
        heading_id: None,
        parent_task_id: None,
        notes_md: None,
        status: TaskStatus::from_db_str(&view.status)?,
        priority: view.priority.clamp(0, 3) as u8,
        someday: view.someday,
        start_on: day(&view.start_on),
        deadline_on: day(&view.deadline_on),
        completed_at: None,
        order_key: view.order_key.clone(),
        assignee_person_id: None,
        recurrence_id: None,
    };
    tasks::classify(&task, today)
}

fn to16(b: &[u8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    let n = b.len().min(16);
    out[..n].copy_from_slice(&b[..n]);
    out
}

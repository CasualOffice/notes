//! The append-only entity op-log — the crash-safe write-ahead and dormant sync
//! seam. Implements Data Model §11.2 (`entity_op`) and the truth-vs-projection
//! contract of §11.2 / §13.2.
//!
//! `entity_op` + `note.doc_json` + the detail tables are **truth**. This module
//! owns (a) appending an op to `entity_op`, and (b) *applying* an op to the spine
//! and detail tables. Replaying the whole log via [`apply_op`] reconstructs the
//! truth tables bit-for-bit — the master correctness oracle (see
//! [`crate::rebuild`]).
//!
//! To stay generic without ever letting the WebView-authored payload name raw
//! SQL, detail mutations are constrained to a **fixed table/column allowlist**
//! (Phase-1 spine-backed detail tables) and bound as parameters.

use std::collections::BTreeMap;

use app_domain::{Hlc, Id, OpId, Timestamp};
use rusqlite::types::Value as SqlValue;
use rusqlite::{params, params_from_iter, Transaction};
use serde::{Deserialize, Serialize};

use crate::error::{StorageError, StorageResult};

/// The `entity_op.kind` discriminator (Data Model §11.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Create,
    Update,
    Delete,
    Link,
    Unlink,
    FieldSet,
}

impl OpKind {
    /// The exact string stored in `entity_op.kind`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
            Self::Link => "link",
            Self::Unlink => "unlink",
            Self::FieldSet => "field_set",
        }
    }

    /// Parse from the stored string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "create" => Self::Create,
            "update" => Self::Update,
            "delete" => Self::Delete,
            "link" => Self::Link,
            "unlink" => Self::Unlink,
            "field_set" => Self::FieldSet,
            _ => return None,
        })
    }
}

/// Spine (`entity`) fields carried by an entity-mutation op.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SpineFields {
    /// `entity.kind` string (validated by the table CHECK on insert).
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub daily_date: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<i64>,
}

/// A detail-table upsert: an allowlisted table plus a column→value map.
/// `entity_id` is supplied by the op, never by the payload.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DetailFields {
    /// Target detail table (allowlisted — see [`DetailTable`]).
    pub table: DetailTable,
    /// Non-PK columns to set. Deterministically ordered via `BTreeMap`.
    pub columns: BTreeMap<String, serde_json::Value>,
}

/// The Phase-1 spine-backed detail tables an op may target.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetailTable {
    Note,
    Notebook,
    Tag,
    Task,
    Project,
    Area,
    Reminder,
    RecurrenceRule,
    Person,
}

impl DetailTable {
    const fn sql_name(self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Notebook => "notebook",
            Self::Tag => "tag",
            Self::Task => "task",
            Self::Project => "project",
            Self::Area => "area",
            Self::Reminder => "reminder",
            Self::RecurrenceRule => "recurrence_rule",
            Self::Person => "person",
        }
    }

    /// The settable (non-PK) columns permitted for this table. Any column outside
    /// this set is rejected — the allowlist is the injection guard.
    const fn allowed_columns(self) -> &'static [&'static str] {
        match self {
            Self::Note => &[
                "notebook_id",
                "doc_json",
                "doc_schema_version",
                "daily_date",
                "is_pinned",
                "content_hash",
                "word_count",
            ],
            Self::Notebook => &["parent_id", "order_key", "icon", "color"],
            Self::Tag => &["name", "display", "color", "schema_json"],
            Self::Task => &[
                "project_id",
                "area_id",
                "heading_id",
                "parent_task_id",
                "notes_md",
                "status",
                "priority",
                "someday",
                "start_on",
                "deadline_on",
                "completed_at",
                "order_key",
                "assignee_person_id",
                "recurrence_id",
            ],
            Self::Project => &[
                "area_id",
                "note_id",
                "status",
                "start_on",
                "deadline_on",
                "completed_at",
                "order_key",
            ],
            Self::Area => &["order_key", "icon"],
            Self::Reminder => &[
                "target_kind",
                "target_id",
                "target_block_id",
                "fire_at",
                "tz",
                "state",
                "snoozed_until",
                "os_handle",
                "os_layer",
                "recurrence_id",
                "body",
                "created_at",
            ],
            Self::RecurrenceRule => &[
                "rrule",
                "mode",
                "next_scheduled_on",
                "until_on",
                "count_remaining",
                "complete_instances",
            ],
            Self::Person => &["display", "canonical", "aliases", "email", "avatar_sha256"],
        }
    }

    /// Columns whose value is a UUIDv7 stored as a 16-byte BLOB. When such a
    /// column carries a hyphenated-string JSON value it is bound as a blob.
    const fn blob_columns(self) -> &'static [&'static str] {
        match self {
            Self::Note => &["notebook_id"],
            Self::Notebook => &["parent_id"],
            Self::Task => &[
                "project_id",
                "area_id",
                "heading_id",
                "parent_task_id",
                "assignee_person_id",
                "recurrence_id",
            ],
            Self::Project => &["area_id", "note_id"],
            Self::Reminder => &["target_id", "recurrence_id"],
            _ => &[],
        }
    }
}

/// A projected `block` row (Data Model §4.2). Blocks are not spine entities;
/// they are carried in the op-log so a full replay reconstructs them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BlockRow {
    pub note_id: Id,
    pub block_id: String,
    pub node_type: String,
    pub seq: i64,
    #[serde(default)]
    pub depth: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attrs_json: Option<String>,
    pub order_key: String,
}

/// A `link` row (Data Model §5.1).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LinkRow {
    pub id: Id,
    pub src_entity: Id,
    pub dst_entity: Id,
    pub rel: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub src_block_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dst_block_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_segment_ids: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_json: Option<String>,
    #[serde(default = "default_origin")]
    pub origin: String,
    pub created_at: i64,
    pub hlc: String,
}

fn default_origin() -> String {
    "user".to_string()
}

/// The payload body persisted in `entity_op.payload` (JSON).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "body", rename_all = "snake_case")]
pub enum OpBody {
    /// Create/update the spine row and (optionally) its detail row.
    EntitySet {
        spine: SpineFields,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<DetailFields>,
    },
    /// Soft-delete an entity (`entity.deleted_at = at`).
    EntityDelete { at: i64 },
    /// Upsert a projected block row.
    BlockSet { block: BlockRow },
    /// Upsert a link edge.
    LinkSet { link: LinkRow },
    /// Soft-delete a link edge.
    LinkDelete { link_id: Id, at: i64 },
}

/// One op-log entry: an `entity_op` row plus its typed body.
#[derive(Clone, Debug)]
pub struct EntityOp {
    pub op_id: OpId,
    pub entity_id: Id,
    pub kind: OpKind,
    pub hlc: Hlc,
    pub actor: String,
    pub body: OpBody,
    pub created_at: Timestamp,
}

impl EntityOp {
    /// Construct an op, deriving a sensible `kind` from the body when the caller
    /// does not care to distinguish `create`/`update`/`field_set`.
    #[must_use]
    pub fn new(entity_id: Id, hlc: Hlc, body: OpBody) -> Self {
        let kind = match &body {
            OpBody::EntitySet { .. } => OpKind::Update,
            OpBody::EntityDelete { .. } => OpKind::Delete,
            OpBody::BlockSet { .. } => OpKind::FieldSet,
            OpBody::LinkSet { .. } => OpKind::Link,
            OpBody::LinkDelete { .. } => OpKind::Unlink,
        };
        Self {
            op_id: OpId::new(),
            entity_id,
            kind,
            hlc,
            actor: "local".to_string(),
            body,
            created_at: Timestamp::now(),
        }
    }

    /// Override the derived `kind` (e.g. mark an `EntitySet` as `create`).
    #[must_use]
    pub fn with_kind(mut self, kind: OpKind) -> Self {
        self.kind = kind;
        self
    }
}

/// Append an op to `entity_op` (the durable log). Does **not** apply it — call
/// [`apply_op`] as well (the [`crate::Store`] facade does both atomically).
pub fn append_op(tx: &Transaction<'_>, op: &EntityOp) -> StorageResult<()> {
    let payload = serde_json::to_string(&op.body)?;
    tx.execute(
        "INSERT INTO entity_op(op_id, entity_id, kind, hlc, actor, payload, applied, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7)",
        params![
            op.op_id.to_string(),
            op.entity_id.as_bytes().as_slice(),
            op.kind.as_str(),
            op.hlc.to_string(),
            op.actor,
            payload,
            op.created_at.as_millis(),
        ],
    )?;
    Ok(())
}

/// Apply an op to the spine/detail/block/link tables (the truth projection).
/// Idempotent under re-application (upserts + soft-deletes), which is what makes
/// a full log replay bit-identical.
pub fn apply_op(tx: &Transaction<'_>, op: &EntityOp) -> StorageResult<()> {
    match &op.body {
        OpBody::EntitySet { spine, detail } => {
            upsert_entity(tx, op.entity_id, &op.hlc, spine)?;
            if let Some(d) = detail {
                upsert_detail(tx, op.entity_id, d)?;
            }
        }
        OpBody::EntityDelete { at } => {
            tx.execute(
                "UPDATE entity SET deleted_at = ?2, updated_at = ?2, hlc = ?3 WHERE id = ?1",
                params![op.entity_id.as_bytes().as_slice(), at, op.hlc.to_string()],
            )?;
        }
        OpBody::BlockSet { block } => upsert_block(tx, block)?,
        OpBody::LinkSet { link } => upsert_link(tx, link)?,
        OpBody::LinkDelete { link_id, at } => {
            tx.execute(
                "UPDATE link SET deleted_at = ?2 WHERE id = ?1",
                params![link_id.as_bytes().as_slice(), at],
            )?;
        }
    }
    Ok(())
}

/// Load every op from `entity_op` in insertion order for replay.
///
/// Ordering is by the implicit `rowid` (append-only, strictly increasing), which
/// is the exact order ops were committed — FK-safe on replay and independent of
/// same-millisecond ULID/UUID tie-breaks.
pub fn load_ordered(conn: &rusqlite::Connection) -> StorageResult<Vec<EntityOp>> {
    let mut stmt = conn.prepare(
        "SELECT op_id, entity_id, kind, hlc, actor, payload, created_at
         FROM entity_op ORDER BY rowid",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, Vec<u8>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, String>(5)?,
            r.get::<_, i64>(6)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (op_id, entity_id, kind, hlc, actor, payload, created_at) = row?;
        out.push(EntityOp {
            op_id: op_id
                .parse()
                .map_err(|_| StorageError::Invariant("bad op_id in entity_op".into()))?,
            entity_id: Id::from_bytes(to_16(&entity_id)?),
            kind: OpKind::from_db_str(&kind)
                .ok_or_else(|| StorageError::Invariant(format!("bad op kind '{kind}'")))?,
            hlc: hlc
                .parse()
                .map_err(|_| StorageError::Invariant("bad hlc in entity_op".into()))?,
            actor,
            body: serde_json::from_str(&payload)?,
            created_at: Timestamp::from_millis(created_at),
        });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Internal upsert helpers
// ---------------------------------------------------------------------------

fn upsert_entity(
    tx: &Transaction<'_>,
    id: Id,
    hlc: &Hlc,
    spine: &SpineFields,
) -> StorageResult<()> {
    tx.execute(
        "INSERT INTO entity(id, kind, title, daily_date, created_at, updated_at, hlc, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(id) DO UPDATE SET
            kind       = excluded.kind,
            title      = excluded.title,
            daily_date = excluded.daily_date,
            updated_at = excluded.updated_at,
            hlc        = excluded.hlc,
            deleted_at = excluded.deleted_at",
        params![
            id.as_bytes().as_slice(),
            spine.kind,
            spine.title,
            spine.daily_date,
            spine.created_at,
            spine.updated_at,
            hlc.to_string(),
            spine.deleted_at,
        ],
    )?;
    Ok(())
}

fn upsert_detail(tx: &Transaction<'_>, entity_id: Id, d: &DetailFields) -> StorageResult<()> {
    let allowed = d.table.allowed_columns();
    let blob_cols = d.table.blob_columns();

    // Validate every column against the allowlist before building any SQL.
    for name in d.columns.keys() {
        if !allowed.contains(&name.as_str()) {
            return Err(StorageError::UnsupportedOp(format!(
                "column '{}' not allowed on table '{}'",
                name,
                d.table.sql_name()
            )));
        }
    }

    // `BTreeMap` gives a stable column order.
    let cols: Vec<&String> = d.columns.keys().collect();
    let t = d.table.sql_name();

    // Bind values once, laid out as ?1 = entity_id, ?2.. = provided columns.
    let mut values: Vec<SqlValue> = Vec::with_capacity(cols.len() + 1);
    values.push(SqlValue::Blob(entity_id.as_bytes().to_vec()));
    for c in &cols {
        let v = &d.columns[*c];
        if blob_cols.contains(&c.as_str()) {
            values.push(json_id_to_blob(v)?);
        } else {
            values.push(json_to_sql(v)?);
        }
    }

    // Detail rows are event-sourced: a *create* op carries every NOT NULL
    // column, whereas an *update* op carries only the changed columns (a delta).
    // A single `INSERT ... ON CONFLICT DO UPDATE` cannot express this, because
    // SQLite enforces the INSERT's NOT NULL constraints before resolving the
    // conflict — so a partial delta would fail on an absent NOT NULL column
    // (e.g. `task.order_key`). Instead: UPDATE the provided columns of an
    // existing row; if none exists yet (a create), INSERT the full row.
    if !cols.is_empty() {
        let mut set_sql = String::new();
        for (i, c) in cols.iter().enumerate() {
            if i > 0 {
                set_sql.push_str(", ");
            }
            // Column placeholders stay ?2.. so `values` is reused verbatim.
            set_sql.push_str(&format!("{c} = ?{}", i + 2));
        }
        let update_sql = format!("UPDATE {t} SET {set_sql} WHERE entity_id = ?1");
        let affected = tx.execute(&update_sql, params_from_iter(values.iter()))?;
        if affected >= 1 {
            return Ok(());
        }
    }

    // Create path (row absent): INSERT entity_id + provided columns. On a create
    // op every NOT NULL column is present. `DO NOTHING` keeps re-apply idempotent.
    let mut col_sql = String::from("entity_id");
    let mut placeholders = String::from("?1");
    for (i, c) in cols.iter().enumerate() {
        col_sql.push_str(", ");
        col_sql.push_str(c);
        placeholders.push_str(&format!(", ?{}", i + 2));
    }
    let insert_sql = format!(
        "INSERT INTO {t}({col_sql}) VALUES ({placeholders})
         ON CONFLICT(entity_id) DO NOTHING"
    );
    tx.execute(&insert_sql, params_from_iter(values.iter()))?;
    Ok(())
}

fn upsert_block(tx: &Transaction<'_>, b: &BlockRow) -> StorageResult<()> {
    tx.execute(
        "INSERT INTO block(note_id, block_id, node_type, seq, depth, text_content, attrs_json, order_key)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
         ON CONFLICT(note_id, block_id) DO UPDATE SET
            node_type    = excluded.node_type,
            seq          = excluded.seq,
            depth        = excluded.depth,
            text_content = excluded.text_content,
            attrs_json   = excluded.attrs_json,
            order_key    = excluded.order_key",
        params![
            b.note_id.as_bytes().as_slice(),
            b.block_id,
            b.node_type,
            b.seq,
            b.depth,
            b.text_content,
            b.attrs_json,
            b.order_key,
        ],
    )?;
    Ok(())
}

fn upsert_link(tx: &Transaction<'_>, l: &LinkRow) -> StorageResult<()> {
    tx.execute(
        "INSERT INTO link(id, src_entity, dst_entity, rel, src_block_id, dst_block_id,
                          evidence_segment_ids, data_json, origin, created_at, hlc, deleted_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)
         ON CONFLICT(id) DO UPDATE SET
            src_entity           = excluded.src_entity,
            dst_entity           = excluded.dst_entity,
            rel                  = excluded.rel,
            src_block_id         = excluded.src_block_id,
            dst_block_id         = excluded.dst_block_id,
            evidence_segment_ids = excluded.evidence_segment_ids,
            data_json            = excluded.data_json,
            origin               = excluded.origin,
            hlc                  = excluded.hlc,
            deleted_at           = NULL",
        params![
            l.id.as_bytes().as_slice(),
            l.src_entity.as_bytes().as_slice(),
            l.dst_entity.as_bytes().as_slice(),
            l.rel,
            l.src_block_id,
            l.dst_block_id,
            l.evidence_segment_ids,
            l.data_json,
            l.origin,
            l.created_at,
            l.hlc,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Value conversion
// ---------------------------------------------------------------------------

fn json_to_sql(v: &serde_json::Value) -> StorageResult<SqlValue> {
    Ok(match v {
        serde_json::Value::Null => SqlValue::Null,
        serde_json::Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                SqlValue::Null
            }
        }
        serde_json::Value::String(s) => SqlValue::Text(s.clone()),
        // Arrays/objects (e.g. attrs_json, aliases) persist as JSON text.
        other => SqlValue::Text(serde_json::to_string(other)?),
    })
}

/// A UUIDv7 FK column: a hyphenated-string JSON value → 16-byte BLOB; `null`
/// stays `NULL`.
fn json_id_to_blob(v: &serde_json::Value) -> StorageResult<SqlValue> {
    match v {
        serde_json::Value::Null => Ok(SqlValue::Null),
        serde_json::Value::String(s) => {
            let id: Id = s
                .parse()
                .map_err(|_| StorageError::UnsupportedOp(format!("invalid uuid FK value '{s}'")))?;
            Ok(SqlValue::Blob(id.as_bytes().to_vec()))
        }
        _ => Err(StorageError::UnsupportedOp(
            "uuid FK column expects a string or null".into(),
        )),
    }
}

fn to_16(b: &[u8]) -> StorageResult<[u8; 16]> {
    b.try_into()
        .map_err(|_| StorageError::Invariant("entity id blob is not 16 bytes".into()))
}

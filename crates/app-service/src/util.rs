//! Shared helpers for the orchestration workflows: op-log construction, spine
//! reads, content hashing, block-id minting, and date/text utilities. Keeping op
//! construction here (not in the WebView) upholds the "WebView never sees SQL /
//! op vocabulary lives Rust-side" invariant (CLAUDE.md).

use std::collections::BTreeMap;

use app_domain::{Hlc, Id};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::Value;
use sha2::{Digest, Sha256};
use storage::{DetailFields, DetailTable, EntityOp, OpBody, OpKind, SpineFields};

/// A column map for a detail-table upsert.
pub(crate) type Columns = BTreeMap<String, Value>;

/// The minimal spine facts read back for a partial (detail-only) update, so an
/// `EntitySet` op can preserve `kind`/`created_at`/`title` while bumping
/// `updated_at`.
#[derive(Clone, Debug)]
pub(crate) struct Spine {
    pub kind: String,
    pub title: Option<String>,
    pub daily_date: Option<String>,
    pub created_at: i64,
    pub deleted_at: Option<i64>,
}

/// Read the spine row for `id`, or `None` if it does not exist.
pub(crate) fn read_spine(conn: &Connection, id: Id) -> rusqlite::Result<Option<Spine>> {
    conn.query_row(
        "SELECT kind, title, daily_date, created_at, deleted_at FROM entity WHERE id = ?1",
        params![id.as_bytes().as_slice()],
        |r| {
            Ok(Spine {
                kind: r.get(0)?,
                title: r.get(1)?,
                daily_date: r.get(2)?,
                created_at: r.get(3)?,
                deleted_at: r.get(4)?,
            })
        },
    )
    .optional()
}

/// Build a `create` op for a fresh entity (spine + optional detail).
pub(crate) fn create_op(
    id: Id,
    hlc: Hlc,
    kind: &str,
    title: Option<String>,
    daily_date: Option<String>,
    now: i64,
    detail: Option<(DetailTable, Columns)>,
) -> EntityOp {
    EntityOp::new(
        id,
        hlc,
        OpBody::EntitySet {
            spine: SpineFields {
                kind: kind.to_string(),
                title,
                daily_date,
                created_at: now,
                updated_at: now,
                deleted_at: None,
            },
            detail: detail.map(|(table, columns)| DetailFields { table, columns }),
        },
    )
    .with_kind(OpKind::Create)
}

/// Build an `update` op that preserves the existing spine `kind`/`created_at` and
/// bumps `updated_at`, optionally patching `title` and a detail column subset.
pub(crate) fn update_op(
    id: Id,
    hlc: Hlc,
    spine: &Spine,
    new_title: Option<String>,
    now: i64,
    detail: Option<(DetailTable, Columns)>,
) -> EntityOp {
    EntityOp::new(
        id,
        hlc,
        OpBody::EntitySet {
            spine: SpineFields {
                kind: spine.kind.clone(),
                title: new_title.or_else(|| spine.title.clone()),
                daily_date: spine.daily_date.clone(),
                created_at: spine.created_at,
                updated_at: now,
                deleted_at: spine.deleted_at,
            },
            detail: detail.map(|(table, columns)| DetailFields { table, columns }),
        },
    )
}

/// Build a soft-delete op for `id`.
pub(crate) fn delete_op(id: Id, hlc: Hlc, at: i64) -> EntityOp {
    EntityOp::new(id, hlc, OpBody::EntityDelete { at }).with_kind(OpKind::Delete)
}

/// SHA-256 of the note body, hex-encoded — the content-hash gate for re-projection
/// (HLD §8.1: re-saves without a body change don't re-project/re-embed).
pub(crate) fn content_hash(doc_json: &str) -> String {
    let mut h = Sha256::new();
    h.update(doc_json.as_bytes());
    let digest = h.finalize();
    let mut s = String::with_capacity(64);
    for b in digest {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Mint a stable, collision-resistant block id (22 hex chars from a UUIDv7).
/// (AC-1.1c wants a 22-char nanoid; UUIDv7 hex is deterministic-enough and unique.)
pub(crate) fn mint_block_id() -> String {
    let hex: String = Id::new()
        .as_bytes()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    hex[..22].to_string()
}

/// Word count across the concatenated block text (for `note.word_count`).
pub(crate) fn word_count(text: &str) -> i64 {
    text.split_whitespace().count() as i64
}

/// Today's local wall-date as `YYYY-MM-DD` (the `:today` bind for bucket/date
/// queries). DST-correct because it uses the OS local zone.
pub(crate) fn today_local() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Convenience: single-column detail map.
pub(crate) fn col1(k: &str, v: Value) -> Columns {
    let mut m = Columns::new();
    m.insert(k.to_string(), v);
    m
}

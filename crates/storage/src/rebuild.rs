//! Rebuild-from-log: reproject the derived tables from truth. Implements the
//! Data Model §11.2 / §13.2 rebuild contract and the CLAUDE.md correctness oracle
//! ("derived tables must be bit-reproducibly rebuildable from the log").
//!
//! Two operations:
//! - [`reproject_all_fts`] — rebuild the FTS5 indexes from the (already-present)
//!   spine + detail + block truth. Deterministic: rows are ordered by
//!   `entity.id`, so rowids are assigned identically every time.
//! - [`rebuild_from_log`] — the full cold rebuild: wipe the spine/detail/derived
//!   tables and replay every `entity_op` in insertion order, then reproject FTS.
//!   `entity_op` itself (truth) is never touched.

use rusqlite::{params, Transaction};

use crate::db::Db;
use crate::error::StorageResult;
use crate::oplog::{apply_op, load_ordered};

/// Rebuild all FTS5 indexes from truth, inside the caller's transaction.
///
/// Contentless FTS5 tables are cleared with the `'delete-all'` command and their
/// side maps truncated; rows are then re-inserted with deterministic rowids
/// (dense, 1-based, ordered by `entity.id`) so the projection is bit-identical
/// across rebuilds. Only `fts_note` / `fts_task` have Phase-1 source tables;
/// `fts_transcript` / `fts_chunk` are cleared and left empty until their pillars
/// land.
pub fn reproject_all_fts(tx: &Transaction<'_>) -> StorageResult<()> {
    // Clear every FTS index + map.
    for (fts, map) in [
        ("fts_note", "fts_note_map"),
        ("fts_task", "fts_task_map"),
        ("fts_transcript", "fts_transcript_map"),
        ("fts_chunk", "fts_chunk_map"),
    ] {
        tx.execute_batch(&format!(
            "INSERT INTO {fts}({fts}) VALUES('delete-all'); DELETE FROM {map};"
        ))?;
    }

    reproject_fts_note(tx)?;
    reproject_fts_task(tx)?;
    Ok(())
}

fn reproject_fts_note(tx: &Transaction<'_>) -> StorageResult<()> {
    // Live notes, deterministically ordered.
    let notes: Vec<(Vec<u8>, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT e.id, e.title
             FROM entity e JOIN note n ON n.entity_id = e.id
             WHERE e.deleted_at IS NULL
             ORDER BY e.id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        rows.collect::<Result<_, _>>()?
    };

    for (i, (id, title)) in notes.iter().enumerate() {
        let rowid = i as i64 + 1;
        // Body = concatenated block plaintext in document order.
        let body: String = {
            let mut stmt =
                tx.prepare("SELECT text_content FROM block WHERE note_id = ?1 ORDER BY seq")?;
            let parts: Vec<Option<String>> = stmt
                .query_map(params![id], |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            parts.into_iter().flatten().collect::<Vec<_>>().join(" ")
        };
        tx.execute(
            "INSERT INTO fts_note_map(rowid, entity_id) VALUES(?1, ?2)",
            params![rowid, id],
        )?;
        tx.execute(
            "INSERT INTO fts_note(rowid, title, body) VALUES(?1, ?2, ?3)",
            params![rowid, title, body],
        )?;
    }
    Ok(())
}

fn reproject_fts_task(tx: &Transaction<'_>) -> StorageResult<()> {
    let tasks: Vec<(Vec<u8>, Option<String>, Option<String>)> = {
        let mut stmt = tx.prepare(
            "SELECT e.id, e.title, t.notes_md
             FROM entity e JOIN task t ON t.entity_id = e.id
             WHERE e.deleted_at IS NULL
             ORDER BY e.id",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?;
        rows.collect::<Result<_, _>>()?
    };

    for (i, (id, title, notes_md)) in tasks.iter().enumerate() {
        let rowid = i as i64 + 1;
        tx.execute(
            "INSERT INTO fts_task_map(rowid, entity_id) VALUES(?1, ?2)",
            params![rowid, id],
        )?;
        tx.execute(
            "INSERT INTO fts_task(rowid, title, notes_md) VALUES(?1, ?2, ?3)",
            params![rowid, title, notes_md],
        )?;
    }
    Ok(())
}

/// Full cold rebuild: wipe spine/detail/derived tables, replay every op from
/// `entity_op`, and reproject the FTS indexes. The op-log (truth) is preserved.
///
/// FK checks are deferred to commit so the bulk wipe and the replay ordering are
/// both safe. This is the "master correctness oracle": a healthy store is
/// unchanged by it, and a corrupt derived index is repaired by it.
pub fn rebuild_from_log(db: &Db) -> StorageResult<()> {
    db.with_write(|tx| {
        tx.execute_batch("PRAGMA defer_foreign_keys = ON;")?;

        // Wipe truth-derived-from-log tables. Deleting the spine cascades to all
        // detail/block/link/attachment rows; `entity_op` has no FK and survives.
        tx.execute_batch("DELETE FROM entity;")?;

        // Replay in insertion order.
        let ops = load_ordered(tx)?;
        for op in &ops {
            apply_op(tx, op)?;
        }

        reproject_all_fts(tx)?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use app_domain::{Hlc, Id};

    use crate::db::{Db, DbConfig};
    use crate::oplog::{
        append_op, apply_op, BlockRow, DetailFields, DetailTable, EntityOp, LinkRow, OpBody,
        OpKind, SpineFields,
    };

    use super::*;

    /// A deterministic textual snapshot of every table we care about, plus a few
    /// FTS `MATCH` probes. Two byte-identical snapshots ⇒ bit-identical rebuild.
    fn snapshot(db: &Db) -> String {
        db.with_writer_conn(|c| {
            let mut out = String::new();

            let dump = |sql: &str, ncols: usize, label: &str, out: &mut String| {
                out.push_str(label);
                out.push('\n');
                let mut stmt = c.prepare(sql).unwrap();
                let mut rows = stmt.query([]).unwrap();
                while let Some(row) = rows.next().unwrap() {
                    for i in 0..ncols {
                        let v = row.get_ref(i).unwrap();
                        let cell = match v {
                            rusqlite::types::ValueRef::Null => "∅".to_string(),
                            rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                            rusqlite::types::ValueRef::Real(f) => format!("{f}"),
                            rusqlite::types::ValueRef::Text(t) => {
                                String::from_utf8_lossy(t).into_owned()
                            }
                            rusqlite::types::ValueRef::Blob(b) => {
                                b.iter().map(|x| format!("{x:02x}")).collect()
                            }
                        };
                        out.push_str(&cell);
                        out.push('|');
                    }
                    out.push('\n');
                }
            };

            dump(
                "SELECT id, kind, title, daily_date, deleted_at FROM entity ORDER BY id",
                5,
                "== entity ==",
                &mut out,
            );
            dump(
                "SELECT entity_id, notebook_id, doc_json, content_hash FROM note ORDER BY entity_id",
                4,
                "== note ==",
                &mut out,
            );
            dump(
                "SELECT note_id, block_id, seq, text_content FROM block ORDER BY note_id, seq",
                4,
                "== block ==",
                &mut out,
            );
            dump(
                "SELECT entity_id, status, notes_md, deadline_on FROM task ORDER BY entity_id",
                4,
                "== task ==",
                &mut out,
            );
            dump(
                "SELECT id, src_entity, dst_entity, rel, deleted_at FROM link ORDER BY id",
                5,
                "== link ==",
                &mut out,
            );
            dump(
                "SELECT rowid, entity_id FROM fts_note_map ORDER BY rowid",
                2,
                "== fts_note_map ==",
                &mut out,
            );
            dump(
                "SELECT rowid, entity_id FROM fts_task_map ORDER BY rowid",
                2,
                "== fts_task_map ==",
                &mut out,
            );
            // FTS content probes (contentless tables aren't directly selectable).
            dump(
                "SELECT rowid FROM fts_note WHERE fts_note MATCH 'meeting' ORDER BY rowid",
                1,
                "== fts_note MATCH meeting ==",
                &mut out,
            );
            dump(
                "SELECT rowid FROM fts_task WHERE fts_task MATCH 'ship' ORDER BY rowid",
                1,
                "== fts_task MATCH ship ==",
                &mut out,
            );
            Ok(out)
        })
        .unwrap()
    }

    fn commit(db: &Db, op: EntityOp) {
        db.with_write(|tx| {
            append_op(tx, &op)?;
            apply_op(tx, &op)?;
            reproject_all_fts(tx)?;
            Ok(())
        })
        .unwrap();
    }

    fn note_detail(notebook: Option<Id>, doc: &str, hash: &str) -> DetailFields {
        let mut columns = BTreeMap::new();
        columns.insert("doc_json".into(), serde_json::Value::String(doc.into()));
        columns.insert(
            "content_hash".into(),
            serde_json::Value::String(hash.into()),
        );
        columns.insert(
            "notebook_id".into(),
            notebook.map_or(serde_json::Value::Null, |n| {
                serde_json::Value::String(n.to_string())
            }),
        );
        DetailFields {
            table: DetailTable::Note,
            columns,
        }
    }

    #[test]
    fn small_op_sequence_rebuilds_bit_identically() {
        let db = Db::open(DbConfig::memory()).unwrap();
        let hlc = |ms: i64, c: u32| Hlc::new(ms, c, "nodeA");

        // 1) a notebook
        let notebook = Id::new();
        commit(
            &db,
            EntityOp::new(
                notebook,
                hlc(1000, 0),
                OpBody::EntitySet {
                    spine: SpineFields {
                        kind: "notebook".into(),
                        title: Some("Work".into()),
                        daily_date: None,
                        created_at: 1000,
                        updated_at: 1000,
                        deleted_at: None,
                    },
                    detail: Some(DetailFields {
                        table: DetailTable::Notebook,
                        columns: {
                            let mut m = BTreeMap::new();
                            m.insert("order_key".into(), serde_json::Value::String("a0".into()));
                            m
                        },
                    }),
                },
            )
            .with_kind(OpKind::Create),
        );

        // 2) a note in that notebook, with two blocks
        let note = Id::new();
        commit(
            &db,
            EntityOp::new(
                note,
                hlc(1001, 0),
                OpBody::EntitySet {
                    spine: SpineFields {
                        kind: "note".into(),
                        title: Some("Kickoff meeting".into()),
                        daily_date: None,
                        created_at: 1001,
                        updated_at: 1001,
                        deleted_at: None,
                    },
                    detail: Some(note_detail(Some(notebook), "{\"v\":1}", "hash-1")),
                },
            )
            .with_kind(OpKind::Create),
        );
        commit(
            &db,
            EntityOp::new(
                note,
                hlc(1002, 0),
                OpBody::BlockSet {
                    block: BlockRow {
                        note_id: note,
                        block_id: "b1".into(),
                        node_type: "paragraph".into(),
                        seq: 0,
                        depth: 0,
                        text_content: Some("Notes from the kickoff meeting".into()),
                        attrs_json: None,
                        order_key: "a0".into(),
                    },
                },
            ),
        );
        commit(
            &db,
            EntityOp::new(
                note,
                hlc(1003, 0),
                OpBody::BlockSet {
                    block: BlockRow {
                        note_id: note,
                        block_id: "b2".into(),
                        node_type: "paragraph".into(),
                        seq: 1,
                        depth: 0,
                        text_content: Some("Follow up next week".into()),
                        attrs_json: None,
                        order_key: "a1".into(),
                    },
                },
            ),
        );

        // 3) a tag entity + a tagged link from the note
        let tag = Id::new();
        commit(
            &db,
            EntityOp::new(
                tag,
                hlc(1004, 0),
                OpBody::EntitySet {
                    spine: SpineFields {
                        kind: "tag".into(),
                        title: Some("meeting".into()),
                        daily_date: None,
                        created_at: 1004,
                        updated_at: 1004,
                        deleted_at: None,
                    },
                    detail: Some(DetailFields {
                        table: DetailTable::Tag,
                        columns: {
                            let mut m = BTreeMap::new();
                            m.insert("name".into(), serde_json::Value::String("meeting".into()));
                            m.insert(
                                "display".into(),
                                serde_json::Value::String("Meeting".into()),
                            );
                            m
                        },
                    }),
                },
            )
            .with_kind(OpKind::Create),
        );
        let link = Id::new();
        commit(
            &db,
            EntityOp::new(
                note,
                hlc(1005, 0),
                OpBody::LinkSet {
                    link: LinkRow {
                        id: link,
                        src_entity: note,
                        dst_entity: tag,
                        rel: "tagged".into(),
                        src_block_id: None,
                        dst_block_id: None,
                        evidence_segment_ids: None,
                        data_json: None,
                        origin: "projected".into(),
                        created_at: 1005,
                        hlc: hlc(1005, 0).to_string(),
                    },
                },
            ),
        );

        // 4) a task, then update it, then a second task we soft-delete
        let task = Id::new();
        commit(
            &db,
            EntityOp::new(
                task,
                hlc(1006, 0),
                OpBody::EntitySet {
                    spine: SpineFields {
                        kind: "task".into(),
                        title: Some("Ship v1".into()),
                        daily_date: None,
                        created_at: 1006,
                        updated_at: 1006,
                        deleted_at: None,
                    },
                    detail: Some(DetailFields {
                        table: DetailTable::Task,
                        columns: {
                            let mut m = BTreeMap::new();
                            m.insert("status".into(), serde_json::Value::String("open".into()));
                            m.insert(
                                "notes_md".into(),
                                serde_json::Value::String("ship the first release".into()),
                            );
                            m.insert("order_key".into(), serde_json::Value::String("a0".into()));
                            m
                        },
                    }),
                },
            )
            .with_kind(OpKind::Create),
        );
        commit(
            &db,
            EntityOp::new(
                task,
                hlc(1007, 0),
                OpBody::EntitySet {
                    spine: SpineFields {
                        kind: "task".into(),
                        title: Some("Ship v1".into()),
                        daily_date: None,
                        created_at: 1006,
                        updated_at: 1007,
                        deleted_at: None,
                    },
                    detail: Some(DetailFields {
                        table: DetailTable::Task,
                        columns: {
                            let mut m = BTreeMap::new();
                            m.insert(
                                "deadline_on".into(),
                                serde_json::Value::String("2026-08-01".into()),
                            );
                            m
                        },
                    }),
                },
            ),
        );

        let task2 = Id::new();
        commit(
            &db,
            EntityOp::new(
                task2,
                hlc(1008, 0),
                OpBody::EntitySet {
                    spine: SpineFields {
                        kind: "task".into(),
                        title: Some("Obsolete".into()),
                        daily_date: None,
                        created_at: 1008,
                        updated_at: 1008,
                        deleted_at: None,
                    },
                    detail: Some(DetailFields {
                        table: DetailTable::Task,
                        columns: {
                            let mut m = BTreeMap::new();
                            m.insert("status".into(), serde_json::Value::String("open".into()));
                            m.insert("order_key".into(), serde_json::Value::String("a1".into()));
                            m
                        },
                    }),
                },
            )
            .with_kind(OpKind::Create),
        );
        commit(
            &db,
            EntityOp::new(task2, hlc(1009, 0), OpBody::EntityDelete { at: 1009 })
                .with_kind(OpKind::Delete),
        );

        // The incremental projection is the reference.
        let before = snapshot(&db);

        // Rebuild the world from the op-log and compare.
        rebuild_from_log(&db).unwrap();
        let after = snapshot(&db);

        assert_eq!(before, after, "rebuild-from-log must be bit-identical");

        // Rebuild is a fixpoint: doing it twice changes nothing.
        rebuild_from_log(&db).unwrap();
        assert_eq!(before, snapshot(&db), "rebuild must be idempotent");
    }
}

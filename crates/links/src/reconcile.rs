//! Edge write path: OR-Set upsert for authored edges and delete-and-reinsert
//! reconcile for `projected` edges (Data Model §5.1, HLD §8.1 `links.reconcile`).
//!
//! **Never dual-write.** An edge lives once (`src → dst`); the reverse "backlinks"
//! view is a read ([`crate::query::backlinks`]). Authored (`user`/`meeting`/
//! `ai_suggested`) edges use **OR-Set** semantics — add-wins revive of a tombstone,
//! remove-as-tombstone — so the dormant `sync-core` seam needs no re-model (HLD
//! §10). `projected` edges are rebuildable from `doc_json`, so they are hard-deleted
//! and reinserted wholesale each save.

use app_domain::Id;
use rusqlite::{params, Connection, OptionalExtension};

use crate::edge::{LinkOrigin, NewLink};
use crate::error::Result;

/// Outcome of a [`reconcile_projected`] call.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReconcileStats {
    /// Projected rows hard-deleted for the source entity.
    pub removed: usize,
    /// Projected rows (re)inserted.
    pub inserted: usize,
}

/// Upsert a single authored edge with OR-Set add-wins semantics.
///
/// Returns the id of the live edge: an existing live row (idempotent add), a
/// revived tombstone, or a freshly inserted row.
///
/// # Errors
/// Propagates any SQLite failure.
pub fn upsert_edge(conn: &Connection, edge: &NewLink, now_ms: i64, hlc: &str) -> Result<Id> {
    if let Some(id) = find_active(conn, edge)? {
        return Ok(id);
    }
    if let Some(id) = revive_tombstone(conn, edge, hlc)? {
        return Ok(id);
    }
    insert_link(conn, edge, now_ms, hlc, false).map(|id| id.expect("insert returns an id"))
}

/// Rebuild all `projected` edges for `src_entity`: hard-delete the old set, then
/// insert `edges` (deduplicated by the unique key via `INSERT OR IGNORE`).
///
/// # Errors
/// Propagates any SQLite failure.
pub fn reconcile_projected(
    conn: &Connection,
    src_entity: Id,
    edges: &[NewLink],
    now_ms: i64,
    hlc: &str,
) -> Result<ReconcileStats> {
    let src = &src_entity.as_bytes()[..];
    let removed = conn.execute(
        "DELETE FROM link WHERE src_entity = ?1 AND origin = 'projected'",
        params![src],
    )?;

    let mut inserted = 0;
    for edge in edges {
        debug_assert_eq!(edge.origin, LinkOrigin::Projected);
        if insert_link(conn, edge, now_ms, hlc, true)?.is_some() {
            inserted += 1;
        }
    }
    Ok(ReconcileStats { removed, inserted })
}

/// Soft-delete (tombstone) a live authored edge, OR-Set remove semantics.
///
/// # Errors
/// Propagates any SQLite failure.
pub fn tombstone_edge(conn: &Connection, edge: &NewLink, now_ms: i64) -> Result<bool> {
    let src = &edge.src_entity.as_bytes()[..];
    let dst = &edge.dst_entity.as_bytes()[..];
    let n = conn.execute(
        "UPDATE link SET deleted_at = ?5 \
         WHERE src_entity = ?1 AND dst_entity = ?2 AND rel = ?3 \
           AND src_block_id IS ?4 AND deleted_at IS NULL",
        params![src, dst, edge.rel.as_str(), edge.src_block_id, now_ms],
    )?;
    Ok(n > 0)
}

fn find_active(conn: &Connection, edge: &NewLink) -> Result<Option<Id>> {
    let src = &edge.src_entity.as_bytes()[..];
    let dst = &edge.dst_entity.as_bytes()[..];
    let id = conn
        .query_row(
            "SELECT id FROM link \
             WHERE src_entity = ?1 AND dst_entity = ?2 AND rel = ?3 \
               AND src_block_id IS ?4 AND deleted_at IS NULL LIMIT 1",
            params![src, dst, edge.rel.as_str(), edge.src_block_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    match id {
        Some(bytes) => Ok(Some(crate::edge::id_from_blob(&bytes)?)),
        None => Ok(None),
    }
}

fn revive_tombstone(conn: &Connection, edge: &NewLink, hlc: &str) -> Result<Option<Id>> {
    let src = &edge.src_entity.as_bytes()[..];
    let dst = &edge.dst_entity.as_bytes()[..];
    let existing = conn
        .query_row(
            "SELECT id FROM link \
             WHERE src_entity = ?1 AND dst_entity = ?2 AND rel = ?3 \
               AND src_block_id IS ?4 AND deleted_at IS NOT NULL \
             ORDER BY created_at DESC LIMIT 1",
            params![src, dst, edge.rel.as_str(), edge.src_block_id],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()?;
    let Some(bytes) = existing else {
        return Ok(None);
    };
    let id = crate::edge::id_from_blob(&bytes)?;
    conn.execute(
        "UPDATE link SET deleted_at = NULL, hlc = ?2 WHERE id = ?1",
        params![&id.as_bytes()[..], hlc],
    )?;
    Ok(Some(id))
}

/// Insert a new row. With `ignore`, a unique-key clash is silently skipped
/// (returns `Ok(None)`); otherwise it errors.
fn insert_link(
    conn: &Connection,
    edge: &NewLink,
    now_ms: i64,
    hlc: &str,
    ignore: bool,
) -> Result<Option<Id>> {
    let id = Id::new();
    let sql = if ignore {
        "INSERT OR IGNORE INTO link \
         (id, src_entity, dst_entity, rel, src_block_id, dst_block_id, \
          evidence_segment_ids, data_json, origin, created_at, hlc, deleted_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL)"
    } else {
        "INSERT INTO link \
         (id, src_entity, dst_entity, rel, src_block_id, dst_block_id, \
          evidence_segment_ids, data_json, origin, created_at, hlc, deleted_at) \
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,NULL)"
    };
    let n = conn.execute(
        sql,
        params![
            &id.as_bytes()[..],
            &edge.src_entity.as_bytes()[..],
            &edge.dst_entity.as_bytes()[..],
            edge.rel.as_str(),
            edge.src_block_id,
            edge.dst_block_id,
            edge.evidence_json(),
            edge.data_json,
            edge.origin.as_str(),
            now_ms,
            hlc,
        ],
    )?;
    Ok((n > 0).then_some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{fresh_db, link_count};
    use app_domain::LinkRel;

    #[test]
    fn upsert_is_idempotent_and_revives_tombstones() {
        let conn = fresh_db();
        let (a, b) = (Id::new(), Id::new());
        let edge = NewLink::new(a, b, LinkRel::Wikilink).with_src_block("p1");

        let id1 = upsert_edge(&conn, &edge, 100, "hlc1").unwrap();
        let id2 = upsert_edge(&conn, &edge, 200, "hlc2").unwrap();
        assert_eq!(id1, id2, "idempotent add returns the same row");
        assert_eq!(link_count(&conn), 1);

        assert!(tombstone_edge(&conn, &edge, 300).unwrap());
        // Re-adding revives the same tombstoned row (add-wins), not a new row.
        let id3 = upsert_edge(&conn, &edge, 400, "hlc3").unwrap();
        assert_eq!(id3, id1);
        assert_eq!(link_count(&conn), 1);
    }

    #[test]
    fn reconcile_projected_replaces_the_set() {
        let conn = fresh_db();
        let note = Id::new();
        let (t1, t2, t3) = (Id::new(), Id::new(), Id::new());

        let first = [
            NewLink::new(note, t1, LinkRel::Wikilink)
                .with_origin(LinkOrigin::Projected)
                .with_src_block("p1"),
            NewLink::new(note, t2, LinkRel::Tagged)
                .with_origin(LinkOrigin::Projected)
                .with_src_block("p1"),
        ];
        let s1 = reconcile_projected(&conn, note, &first, 1, "h").unwrap();
        assert_eq!(s1.inserted, 2);
        assert_eq!(link_count(&conn), 2);

        // Second save drops t2, adds t3.
        let second = [
            NewLink::new(note, t1, LinkRel::Wikilink)
                .with_origin(LinkOrigin::Projected)
                .with_src_block("p1"),
            NewLink::new(note, t3, LinkRel::Mention)
                .with_origin(LinkOrigin::Projected)
                .with_src_block("p2"),
        ];
        let s2 = reconcile_projected(&conn, note, &second, 2, "h").unwrap();
        assert_eq!(s2.removed, 2);
        assert_eq!(s2.inserted, 2);
        assert_eq!(link_count(&conn), 2);
    }

    #[test]
    fn projected_reconcile_leaves_user_edges_untouched() {
        let conn = fresh_db();
        let note = Id::new();
        let target = Id::new();
        // A durable user edge on the same source.
        upsert_edge(
            &conn,
            &NewLink::new(note, target, LinkRel::Wikilink).with_src_block("p9"),
            1,
            "h",
        )
        .unwrap();

        reconcile_projected(&conn, note, &[], 2, "h").unwrap();
        // The user edge survives the projected wipe.
        assert_eq!(link_count(&conn), 1);
    }
}

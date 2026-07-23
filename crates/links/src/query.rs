//! Read-side query builders: **backlinks** (derived-on-read, never materialized)
//! and **unlinked mentions** (an FTS match with no corresponding edge). Data Model
//! §5.1, Feature Specs §1.2 (backlinks panel).

use app_domain::{Id, LinkRel};
use rusqlite::{params, Connection};

use crate::edge::{id_from_blob, rel_from_str};
use crate::error::Result;

/// A reverse reference to a target entity, for the "Linked mentions" panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    /// The referencing edge (`link.id`).
    pub link_id: Id,
    /// The entity the reference originates from (`link.src_entity`).
    pub src_entity: Id,
    /// The precise origin block (`link.src_block_id`), for snippet rendering.
    pub src_block_id: Option<String>,
    /// The relationship (`wikilink` or `mention`).
    pub rel: LinkRel,
    /// Edge payload (`link.data_json`).
    pub data_json: Option<String>,
}

/// The SQL used by [`backlinks`]. Exposed so `storage` / `app-service` can compose
/// or explain it. Bind param `?1` = target entity id BLOB.
pub const BACKLINKS_SQL: &str = "SELECT id, src_entity, src_block_id, rel, data_json \
     FROM link \
     WHERE dst_entity = ?1 AND rel IN ('wikilink','mention') AND deleted_at IS NULL \
     ORDER BY created_at";

/// Read the backlinks for `dst` — every live `wikilink`/`mention` edge pointing at
/// it. This is the whole of "backlinks": no reverse row is ever written.
///
/// # Errors
/// Propagates any SQLite failure or a malformed stored `rel`/id.
pub fn backlinks(conn: &Connection, dst: Id) -> Result<Vec<Backlink>> {
    let mut stmt = conn.prepare(BACKLINKS_SQL)?;
    let rows = stmt.query_map(params![&dst.as_bytes()[..]], |row| {
        Ok((
            row.get::<_, Vec<u8>>(0)?,
            row.get::<_, Vec<u8>>(1)?,
            row.get::<_, Option<String>>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
        ))
    })?;

    let mut out = Vec::new();
    for row in rows {
        let (id, src, block, rel, data) = row?;
        out.push(Backlink {
            link_id: id_from_blob(&id)?,
            src_entity: id_from_blob(&src)?,
            src_block_id: block,
            rel: rel_from_str(&rel)?,
            data_json: data,
        });
    }
    Ok(out)
}

/// The SQL used by [`unlinked_mentions`]. Bind `?1` = an FTS5 `MATCH` query over the
/// target's title; `?2` = the target entity id BLOB (excluded from its own results
/// and from already-linked sources). Requires the `fts_note` external-content table
/// and its `fts_note_map(rowid, entity_id)` side table (Data Model §10).
pub const UNLINKED_MENTIONS_SQL: &str = "SELECT m.entity_id \
     FROM fts_note f JOIN fts_note_map m ON m.rowid = f.rowid \
     WHERE f MATCH ?1 \
       AND m.entity_id != ?2 \
       AND m.entity_id NOT IN ( \
         SELECT src_entity FROM link \
         WHERE dst_entity = ?2 AND rel IN ('wikilink','mention') AND deleted_at IS NULL \
       )";

/// Find "unlinked mentions": notes whose text matches the target's title via FTS
/// but that carry no `wikilink`/`mention` edge to it. These are *not* rows — they
/// are surfaced live in the backlinks panel (Data Model §5.1).
///
/// `fts_match` is a caller-built FTS5 query string (e.g. a quoted phrase of the
/// title). Kept a builder over a borrowed connection so `storage` owns the schema.
///
/// # Errors
/// Propagates any SQLite failure (including a missing FTS table).
pub fn unlinked_mentions(conn: &Connection, target: Id, fts_match: &str) -> Result<Vec<Id>> {
    let mut stmt = conn.prepare(UNLINKED_MENTIONS_SQL)?;
    let rows = stmt.query_map(params![fts_match, &target.as_bytes()[..]], |row| {
        row.get::<_, Vec<u8>>(0)
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(id_from_blob(&row?)?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::NewLink;
    use crate::reconcile::upsert_edge;
    use crate::test_support::fresh_db;

    #[test]
    fn backlinks_read_reverse_edges_without_writing() {
        let conn = fresh_db();
        let target = Id::new();
        let (a, b, c) = (Id::new(), Id::new(), Id::new());

        upsert_edge(
            &conn,
            &NewLink::new(a, target, LinkRel::Wikilink).with_src_block("p1"),
            1,
            "h",
        )
        .unwrap();
        upsert_edge(
            &conn,
            &NewLink::new(b, target, LinkRel::Mention).with_src_block("p2"),
            2,
            "h",
        )
        .unwrap();
        // A `tagged` edge is not a backlink; an edge to someone else is irrelevant.
        upsert_edge(&conn, &NewLink::new(c, target, LinkRel::Tagged), 3, "h").unwrap();
        upsert_edge(&conn, &NewLink::new(a, c, LinkRel::Wikilink), 4, "h").unwrap();

        let back = backlinks(&conn, target).unwrap();
        assert_eq!(back.len(), 2, "only wikilink + mention edges are backlinks");
        assert!(back
            .iter()
            .any(|bl| bl.src_entity == a && bl.rel == LinkRel::Wikilink));
        assert!(back
            .iter()
            .any(|bl| bl.src_entity == b && bl.rel == LinkRel::Mention));
        assert!(back.iter().all(|bl| bl.rel != LinkRel::Tagged));
    }

    #[test]
    fn unlinked_mentions_sql_is_stable() {
        assert!(UNLINKED_MENTIONS_SQL.contains("fts_note"));
        assert!(UNLINKED_MENTIONS_SQL.contains("NOT IN"));
        assert!(BACKLINKS_SQL.contains("rel IN ('wikilink','mention')"));
    }
}

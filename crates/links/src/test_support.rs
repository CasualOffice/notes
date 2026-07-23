//! Test-only helpers: an in-memory SQLite DB carrying the `link` table subset
//! (Data Model §5.1) so the write/read/graph paths can be exercised without the
//! full `storage` schema.

use rusqlite::Connection;

/// The `link` table + its unique/lookup indexes, verbatim from Data Model §5.1
/// (minus the FK `REFERENCES entity(id)` clauses, which need the spine tables).
const SCHEMA: &str = "
CREATE TABLE link (
  id                   BLOB PRIMARY KEY,
  src_entity           BLOB NOT NULL,
  dst_entity           BLOB NOT NULL,
  rel                  TEXT NOT NULL,
  src_block_id         TEXT,
  dst_block_id         TEXT,
  evidence_segment_ids TEXT,
  data_json            TEXT,
  origin               TEXT NOT NULL DEFAULT 'user',
  created_at           INTEGER NOT NULL,
  hlc                  TEXT NOT NULL,
  deleted_at           INTEGER,
  CHECK (rel IN ('wikilink','backlink','mention','tagged','spawned_from',
                 'about','attends','action_item_of','reminds','child_of'))
);
CREATE INDEX idx_link_src ON link(src_entity, rel) WHERE deleted_at IS NULL;
CREATE INDEX idx_link_dst ON link(dst_entity, rel) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX idx_link_uniq ON link(src_entity, dst_entity, rel, src_block_id)
  WHERE deleted_at IS NULL;
";

/// A fresh in-memory connection with the `link` schema applied.
pub fn fresh_db() -> Connection {
    let conn = Connection::open_in_memory().expect("open in-memory db");
    conn.execute_batch(SCHEMA).expect("apply link schema");
    conn
}

/// Count live (non-tombstoned) `link` rows.
pub fn link_count(conn: &Connection) -> i64 {
    conn.query_row(
        "SELECT COUNT(*) FROM link WHERE deleted_at IS NULL",
        [],
        |r| r.get(0),
    )
    .expect("count links")
}

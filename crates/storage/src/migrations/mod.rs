//! Embedded, versioned migration runner. Implements Data Model §13.2:
//! *"Migrations are ordered, forward-only Rust functions (`V001..VNNN`), each
//! idempotent and transactional; the app refuses to open a DB whose
//! `user_version` exceeds the binary's known max."*
//!
//! Each migration is an embedded `.sql` script applied in one transaction. The
//! applied version is recorded in both `PRAGMA user_version` and
//! `setting('schema_version')` (the mirror the rest of the app reads).

use rusqlite::Connection;

use crate::error::{StorageError, StorageResult};

/// One forward-only schema migration.
struct Migration {
    /// Monotonic version this migration brings the DB *to*.
    version: i64,
    /// Human name (diagnostics only).
    name: &'static str,
    /// The DDL/DML executed as a single `execute_batch`.
    sql: &'static str,
}

/// The ordered migration set. Append new entries; never edit a released one.
const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "initial",
        sql: include_str!("V001__initial.sql"),
    },
    Migration {
        version: 2,
        name: "meeting",
        sql: include_str!("V002__meeting.sql"),
    },
];

/// The highest schema version this binary understands.
#[must_use]
pub fn latest_version() -> i64 {
    MIGRATIONS.last().map_or(0, |m| m.version)
}

/// Apply every migration newer than the DB's current `user_version`.
///
/// Refuses (without mutating) a DB from a newer binary. Each migration runs in
/// its own transaction so a crash leaves the DB at a clean version boundary.
pub fn run(conn: &mut Connection) -> StorageResult<()> {
    let current: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let supported = latest_version();

    if current > supported {
        return Err(StorageError::SchemaTooNew {
            found: current,
            supported,
        });
    }

    for m in MIGRATIONS.iter().filter(|m| m.version > current) {
        let tx = conn.transaction()?;
        tx.execute_batch(m.sql)?;
        // user_version can't be parameterized; the value is a trusted constant.
        tx.execute_batch(&format!("PRAGMA user_version = {};", m.version))?;
        tx.execute(
            "INSERT INTO setting(key, value_json, updated_at)
             VALUES ('schema_version', ?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value_json = excluded.value_json,
                                            updated_at = excluded.updated_at",
            rusqlite::params![m.version.to_string(), now_ms()],
        )?;
        tx.commit()?;
        tracing::info!(version = m.version, name = m.name, "applied migration");
    }
    Ok(())
}

fn now_ms() -> i64 {
    app_domain::time::Timestamp::now().as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrates_from_zero_and_is_idempotent() {
        let mut conn = Connection::open_in_memory().unwrap();
        run(&mut conn).unwrap();
        let v: i64 = conn
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .unwrap();
        assert_eq!(v, latest_version());
        // Running again applies nothing and does not error.
        run(&mut conn).unwrap();

        let mirror: String = conn
            .query_row(
                "SELECT value_json FROM setting WHERE key='schema_version'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(mirror, latest_version().to_string());
    }

    #[test]
    fn refuses_newer_db() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("PRAGMA user_version = 9999;").unwrap();
        let err = run(&mut conn).unwrap_err();
        assert!(matches!(err, StorageError::SchemaTooNew { .. }));
    }
}

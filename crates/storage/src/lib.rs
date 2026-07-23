//! # storage
//!
//! Owns every database and filesystem access for Casual Note. Implements the
//! physical layout of **Data Model §12** (on-disk directory), **§10–§11**
//! (FTS5 + append-only NDJSON journals), **§13** (encryption/migrations), and the
//! single-logical-writer contract of **Architecture §7 / §6**.
//!
//! Invariants (CLAUDE.md): the WebView never sees SQL or raw FS; all access is
//! Rust-side via **direct `rusqlite` + SQLCipher** with the
//! `bundled-sqlcipher-vendored-openssl` feature (compiled from source, no system
//! libs). A single logical writer serializes mutating transactions (WAL); readers
//! use separate connections. Every entity mutation appends to `entity_op` + an
//! NDJSON journal so a `kill -9` loses no committed op. Derived tables rebuild
//! bit-identically from the op-log.
//!
//! ## Modules
//! - [`layout`]   — Data Model §12 directory paths.
//! - [`keystore`] — OS-keystore key custody (§13.1) + a dev fallback.
//! - [`db`]       — SQLCipher connection/keying + the single-writer [`Db`].
//! - [`migrations`] — the embedded versioned migration runner (§13.2).
//! - [`oplog`]    — the `entity_op` append-only log + apply/replay (§11.2).
//! - [`journal`]  — the NDJSON entity-op journal (§11.2) — crash-safe write-ahead.
//! - [`blobs`]    — content-addressed blob store (§4.5/§8.2/§12).
//! - [`rebuild`]  — reproject derived tables from the log (§11.2/§13.2).
//!
//! ## The [`Store`] facade
//! [`Store`] wires the [`Db`], the [`OpJournal`], the [`BlobStore`], and the
//! app-data [`Paths`] together. [`Store::commit`] is the one write path: it
//! journals the op (fsync) *then* atomically appends it to `entity_op`, applies
//! it to the truth tables, and reprojects FTS — so a crash between the two leaves
//! the op recoverable from the journal.

#![forbid(unsafe_code)]
#![warn(missing_debug_implementations)]

pub mod blobs;
pub mod db;
pub mod error;
pub mod journal;
pub mod keystore;
pub mod layout;
pub mod migrations;
pub mod oplog;
pub mod rebuild;

pub use blobs::{BlobStore, Sha256Hex};
pub use db::{Db, DbConfig, DbPath, KeyMaterial};
pub use error::{StorageError, StorageResult};
pub use journal::{OpJournal, OpRecord};
pub use keystore::{DevFileKeyStore, KeyStore};
pub use layout::Paths;
pub use oplog::{
    append_op, apply_op, BlockRow, DetailFields, DetailTable, EntityOp, LinkRow, OpBody, OpKind,
    SpineFields,
};
pub use rebuild::{rebuild_from_log, reproject_all_fts};

#[cfg(feature = "os-keystore")]
pub use keystore::KeyringKeyStore;

/// The top-level store facade: encrypted DB + op journal + blob store, rooted at
/// one app-data directory (Data Model §12).
#[derive(Debug)]
pub struct Store {
    db: Db,
    journal: OpJournal,
    blobs: BlobStore,
    paths: Paths,
}

impl Store {
    /// Open the store at `paths.root()` with the given master `key`, creating the
    /// directory skeleton, opening + keying the DB, and running migrations.
    pub fn open(paths: Paths, key: KeyMaterial) -> StorageResult<Self> {
        paths.ensure()?;
        let db = Db::open(DbConfig::file(paths.db_file(), key))?;
        let journal = OpJournal::new(paths.ops_journal_dir())?;
        let blobs = BlobStore::new(paths.files_dir())?;
        Ok(Self {
            db,
            journal,
            blobs,
            paths,
        })
    }

    /// Open an unencrypted in-memory store with journal + blobs under
    /// `scratch_root`. Intended for tests and tooling only.
    pub fn open_memory(scratch_root: Paths) -> StorageResult<Self> {
        scratch_root.ensure()?;
        let db = Db::open(DbConfig::memory())?;
        let journal = OpJournal::new(scratch_root.ops_journal_dir())?;
        let blobs = BlobStore::new(scratch_root.files_dir())?;
        Ok(Self {
            db,
            journal,
            blobs,
            paths: scratch_root,
        })
    }

    /// The single write path. Order matters for crash-safety:
    /// 1. append + `fsync` the op to the NDJSON journal (durable write-ahead);
    /// 2. in one DB transaction: append to `entity_op`, apply to truth tables,
    ///    reproject FTS.
    ///
    /// A crash after (1) but before (2) leaves the op in the journal for
    /// [`Store::recover`] to re-apply; a crash during (2) rolls the tx back.
    pub fn commit(&self, op: &EntityOp) -> StorageResult<()> {
        let rec = OpRecord::from_op(op)?;
        self.journal.append(&rec)?;
        self.db.with_write(|tx| {
            append_op(tx, op)?;
            apply_op(tx, op)?;
            reproject_all_fts(tx)?;
            Ok(())
        })
    }

    /// Replay the op journal, re-applying any op not already present in
    /// `entity_op` (idempotent recovery after an unclean shutdown). Returns the
    /// number of ops re-applied.
    pub fn recover(&self) -> StorageResult<usize> {
        let records = self.journal.replay()?;
        let mut reapplied = 0usize;
        for rec in records {
            let op = rec.into_entity_op()?;
            let op_id = op.op_id.to_string();
            let applied = self.db.with_write(|tx| {
                let exists: bool = tx
                    .query_row(
                        "SELECT 1 FROM entity_op WHERE op_id = ?1",
                        rusqlite::params![op_id],
                        |_| Ok(true),
                    )
                    .unwrap_or(false);
                if exists {
                    return Ok(false);
                }
                append_op(tx, &op)?;
                apply_op(tx, &op)?;
                reproject_all_fts(tx)?;
                Ok(true)
            })?;
            if applied {
                reapplied += 1;
            }
        }
        Ok(reapplied)
    }

    /// Full cold rebuild of the derived tables from `entity_op` (see
    /// [`rebuild_from_log`]).
    pub fn rebuild(&self) -> StorageResult<()> {
        rebuild_from_log(&self.db)
    }

    /// The single-writer database handle.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The content-addressed blob store.
    #[must_use]
    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// The entity-op journal.
    #[must_use]
    pub fn journal(&self) -> &OpJournal {
        &self.journal
    }

    /// The resolved app-data paths.
    #[must_use]
    pub fn paths(&self) -> &Paths {
        &self.paths
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use app_domain::{Hlc, Id};

    use super::*;

    fn scratch() -> Paths {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).unwrap();
        let name: String = b.iter().map(|x| format!("{x:02x}")).collect();
        Paths::new(std::env::temp_dir().join(format!("cn-store-{name}")))
    }

    fn area_op(id: Id) -> EntityOp {
        let mut columns = BTreeMap::new();
        columns.insert(
            "order_key".to_string(),
            serde_json::Value::String("a0".into()),
        );
        EntityOp::new(
            id,
            Hlc::new(1000, 0, "nodeA"),
            OpBody::EntitySet {
                spine: SpineFields {
                    kind: "area".into(),
                    title: Some("Home".into()),
                    daily_date: None,
                    created_at: 1000,
                    updated_at: 1000,
                    deleted_at: None,
                },
                detail: Some(DetailFields {
                    table: DetailTable::Area,
                    columns,
                }),
            },
        )
    }

    #[test]
    fn commit_writes_journal_db_and_recover_is_idempotent() {
        let paths = scratch();
        let store = Store::open_memory(paths.clone()).unwrap();

        let id = Id::new();
        store.commit(&area_op(id)).unwrap();

        // The entity is present, and the journal holds one record.
        let count: i64 = store
            .db()
            .with_writer_conn(|c| {
                c.query_row("SELECT count(*) FROM entity", [], |r| r.get(0))
                    .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(store.journal().replay().unwrap().len(), 1);

        // Recovery re-applies nothing (op already in entity_op).
        assert_eq!(store.recover().unwrap(), 0);

        std::fs::remove_dir_all(paths.root()).ok();
    }
}

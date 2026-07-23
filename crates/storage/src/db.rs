//! SQLCipher connection management + the single logical writer.
//!
//! Implements the persistence-writer contract of Architecture §6/§7:
//! *"a single logical writer serializes all mutating transactions (SQLite WAL,
//! `rusqlite`); readers use additional connections; `busy_timeout` + retry."*
//! DB access is **direct `rusqlite` + SQLCipher** with the
//! `bundled-sqlcipher-vendored-openssl` feature (compiled from source, no system
//! libs) — never `tauri-plugin-sql` (CLAUDE.md invariant).
//!
//! The master key never lives in the DB or logs; it is supplied by the
//! [`keystore`](crate::keystore) module and injected via `PRAGMA key`.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, TransactionBehavior};

use crate::error::{StorageError, StorageResult};
use crate::migrations;

/// A 32-byte (AES-256) SQLCipher master key.
pub type KeyMaterial = [u8; 32];

/// How many times a mutating transaction is retried on `SQLITE_BUSY`.
const BUSY_RETRIES: u32 = 5;
/// Backoff between busy retries (also covered by the connection `busy_timeout`).
const BUSY_BACKOFF: Duration = Duration::from_millis(25);
/// SQLite `busy_timeout` applied to every connection.
const BUSY_TIMEOUT_MS: u32 = 5_000;

/// Where the database lives.
#[derive(Clone, Debug)]
pub enum DbPath {
    /// A file on disk (production).
    File(PathBuf),
    /// A private in-memory database (tests only — no WAL, no encryption).
    Memory,
}

/// Configuration for opening the store.
#[derive(Clone)]
pub struct DbConfig {
    /// Database location.
    pub path: DbPath,
    /// SQLCipher master key. `None` opens an **unencrypted** DB (tests only).
    pub key: Option<KeyMaterial>,
}

impl std::fmt::Debug for DbConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never render the master key.
        f.debug_struct("DbConfig")
            .field("path", &self.path)
            .field("key", &self.key.map(|_| "<redacted>"))
            .finish()
    }
}

impl DbConfig {
    /// A keyed on-disk database — the production configuration.
    #[must_use]
    pub fn file(path: impl Into<PathBuf>, key: KeyMaterial) -> Self {
        Self {
            path: DbPath::File(path.into()),
            key: Some(key),
        }
    }

    /// An unencrypted in-memory database for tests.
    #[must_use]
    pub fn memory() -> Self {
        Self {
            path: DbPath::Memory,
            key: None,
        }
    }
}

/// The store's connection pool: one serialized writer + on-demand readers.
///
/// `Db` owns the sole writer connection behind a [`Mutex`], which is the single
/// logical writer. Read connections are opened fresh via [`Db::open_read`]; WAL
/// mode lets them run concurrently with the writer.
pub struct Db {
    config: DbConfig,
    writer: Mutex<Connection>,
}

impl std::fmt::Debug for Db {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Db").field("config", &self.config).finish()
    }
}

impl Db {
    /// Open (or create) the database, key it, set pragmas, and run all pending
    /// migrations. Refuses to open a DB whose `user_version` exceeds the binary's
    /// known maximum (Data Model §13.2).
    pub fn open(config: DbConfig) -> StorageResult<Self> {
        let mut conn = Self::open_conn(&config)?;
        migrations::run(&mut conn)?;
        Ok(Self {
            config,
            writer: Mutex::new(conn),
        })
    }

    /// Open a fresh **read** connection to the same database (keyed, WAL-aware).
    /// Readers never mutate; all writes funnel through [`Db::with_write`].
    pub fn open_read(&self) -> StorageResult<Connection> {
        // In-memory databases are private per-connection, so a second "read"
        // handle would see an empty DB. Callers of a memory DB must read through
        // the writer; guard against the footgun.
        if matches!(self.config.path, DbPath::Memory) {
            return Err(StorageError::Invariant(
                "open_read is unsupported for an in-memory DB (private per connection)".into(),
            ));
        }
        Self::open_conn(&self.config)
    }

    /// Run `f` inside a single serialized, immediate write transaction.
    ///
    /// Acquires the writer mutex (the single-writer seam), begins an `IMMEDIATE`
    /// transaction, invokes `f`, and commits. `SQLITE_BUSY` from another *process*
    /// is retried with bounded backoff; an error returned by `f` rolls back and is
    /// propagated (no retry — it is the caller's logic failing, not contention).
    pub fn with_write<T>(
        &self,
        f: impl Fn(&rusqlite::Transaction<'_>) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let mut attempt = 0;
        loop {
            // Scope the guard so the single-writer mutex is released *before* any
            // backoff sleep. Reaching the end of this block means a `SQLITE_BUSY`
            // from another process warrants a retry; every other outcome returns.
            {
                let mut guard = self.writer.lock().map_err(|_| StorageError::LockPoisoned)?;
                match guard.transaction_with_behavior(TransactionBehavior::Immediate) {
                    Ok(tx) => {
                        let value = f(&tx)?;
                        match tx.commit() {
                            Ok(()) => return Ok(value),
                            // busy on commit: fall through to backoff + retry
                            Err(e) if is_busy(&e) && attempt < BUSY_RETRIES => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                    // busy on begin: fall through to backoff + retry
                    Err(e) if is_busy(&e) && attempt < BUSY_RETRIES => {}
                    Err(e) => return Err(e.into()),
                };
            }
            attempt += 1;
            std::thread::sleep(BUSY_BACKOFF * attempt);
        }
    }

    /// Borrow the writer connection directly for a read-only query on a memory DB
    /// or a bulk maintenance pass. Prefer [`Db::open_read`] for concurrent reads.
    pub fn with_writer_conn<T>(
        &self,
        f: impl FnOnce(&Connection) -> StorageResult<T>,
    ) -> StorageResult<T> {
        let guard = self.writer.lock().map_err(|_| StorageError::LockPoisoned)?;
        f(&guard)
    }

    // -- internals -----------------------------------------------------------

    fn open_conn(config: &DbConfig) -> StorageResult<Connection> {
        let conn = match &config.path {
            DbPath::File(p) => Connection::open_with_flags(
                p,
                OpenFlags::SQLITE_OPEN_READ_WRITE
                    | OpenFlags::SQLITE_OPEN_CREATE
                    | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )?,
            DbPath::Memory => Connection::open_in_memory()?,
        };

        // §13.1: key MUST be applied before any other statement touches the DB.
        if let Some(key) = &config.key {
            let hex = to_hex(key);
            conn.execute_batch(&format!("PRAGMA key = \"x'{hex}'\";"))?;
        }

        // WAL only makes sense for a file DB; in-memory rejects it. `execute_batch`
        // is used (not `pragma_update`) because `journal_mode` returns a result row.
        if matches!(config.path, DbPath::File(_)) {
            conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL;")?;
        }
        conn.execute_batch("PRAGMA foreign_keys = ON;")?;
        conn.busy_timeout(Duration::from_millis(u64::from(BUSY_TIMEOUT_MS)))?;
        Ok(conn)
    }
}

/// True when a rusqlite error is a `SQLITE_BUSY` / `SQLITE_LOCKED` contention.
fn is_busy(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(err, _)
            if err.code == rusqlite::ErrorCode::DatabaseBusy
                || err.code == rusqlite::ErrorCode::DatabaseLocked
    )
}

/// Lowercase hex encoding for the `PRAGMA key` blob literal. No external dep.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opens_memory_and_migrates() {
        let db = Db::open(DbConfig::memory()).unwrap();
        // A table from V001 exists.
        db.with_writer_conn(|c| {
            let n: i64 = c.query_row("SELECT count(*) FROM entity", [], |r| r.get(0))?;
            assert_eq!(n, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn hex_roundtrips_known_vector() {
        assert_eq!(to_hex(&[0x00, 0x0f, 0xa0, 0xff]), "000fa0ff");
    }

    #[test]
    fn keyed_file_db_roundtrips() {
        let dir = std::env::temp_dir().join(format!("cn-db-{}", uuid_like()));
        std::fs::create_dir_all(&dir).unwrap();
        let key = [7u8; 32];
        let cfg = DbConfig::file(dir.join("casualnote.db"), key);
        {
            let db = Db::open(cfg.clone()).unwrap();
            db.with_write(|tx| {
                tx.execute(
                    "INSERT INTO setting(key, value_json, updated_at) VALUES('k','1',0)",
                    [],
                )?;
                Ok(())
            })
            .unwrap();
        }
        // Reopen with the same key: data survives; wrong key would fail to read.
        let db = Db::open(cfg).unwrap();
        let v: String = db
            .open_read()
            .unwrap()
            .query_row("SELECT value_json FROM setting WHERE key='k'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(v, "1");
        std::fs::remove_dir_all(&dir).ok();
    }

    fn uuid_like() -> String {
        let mut b = [0u8; 8];
        getrandom::getrandom(&mut b).unwrap();
        super::to_hex(&b)
    }
}

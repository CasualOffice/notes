//! # app-service
//!
//! The orchestration facade between `tauri-app` (the command router) and the feature
//! crates. Implements **HLD §4** (component view) and **§6** (command surface
//! semantics): owns transactions, cross-crate workflows, and **`AppEvent` emission**
//! (HLD §7). It is the only place cross-crate write workflows are composed; the
//! single DB writer funnels through `storage`.
//!
//! Each use-case method (`notes_save`, `capture_quick`, `tasks_bucket`, …) composes
//! the pure feature crates (`notes`, `links`, `tasks`, `reminders`, `search`,
//! `app-nlp`) over the one `storage::Store`. Every entity mutation is expressed as a
//! `storage::EntityOp` and committed through `Store::commit`, so derived tables stay
//! bit-reproducible from the op-log (CLAUDE.md op-log invariant). Reads go through
//! the single writer connection.
//!
//! Later-phase commands (meeting / AI / models / export) return a typed
//! [`AppError`] "not yet implemented in this phase" — see [`stubs`].

#![forbid(unsafe_code)]

pub mod capture;
pub mod dto;
pub mod notes;
pub mod reminders;
pub mod search;
pub mod stubs;
pub mod tasks;
pub mod util;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use app_domain::{AppError, AppEvent, AppResult, Hlc, SequencedEvent, Timestamp};
use rusqlite::Connection;
use storage::{DevFileKeyStore, EntityOp, KeyMaterial, Paths, StorageResult, Store};

/// Re-exported so `tauri-app` (which depends on `app-service`, not `app-nlp`) can
/// name the `nlp.parse` return type without a direct `app-nlp` dependency.
pub use app_nlp::ParsedEntry;

/// A sink the host installs to receive [`SequencedEvent`]s destined for the WebView
/// (`tauri::Window::emit`, HLD §7). Kept a boxed closure so `app-service` never
/// depends on Tauri.
pub type EventSink = Box<dyn Fn(SequencedEvent) + Send + Sync>;

/// The orchestration facade. Holds the single [`Store`], a monotonic event `seq`
/// source, the install's HLC, and the [`EventSink`].
pub struct Service {
    store: Store,
    hlc: Mutex<Hlc>,
    seq: AtomicU64,
    node: String,
    sink: EventSink,
}

impl std::fmt::Debug for Service {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Service")
            .field("node", &self.node)
            .field("seq", &self.seq.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Service {
    /// Open the encrypted store rooted at `paths` and build the service over it
    /// (the M0 boot path — HLD §4). The SQLCipher master key is provisioned from
    /// the OS keystore, with a `0600` dev-file fallback for headless boxes (Data
    /// Model §13.1). This is the single place `app-service` owns store custody, so
    /// `tauri-app` and headless tests open the DB identically.
    ///
    /// # Errors
    /// Returns an [`AppError`] if the key cannot be provisioned or the store fails
    /// to open / migrate.
    pub fn open(paths: Paths, node: impl Into<String>, sink: EventSink) -> AppResult<Self> {
        let key = provision_master_key(&paths)?;
        let store = Store::open(paths, key)?;
        Ok(Self::new(store, node, sink))
    }

    /// Build a service over `store`. `node` is the stable HLC node id for this
    /// install; `sink` receives every emitted event.
    #[must_use]
    pub fn new(store: Store, node: impl Into<String>, sink: EventSink) -> Self {
        let node = node.into();
        Self {
            store,
            hlc: Mutex::new(Hlc::now(node.clone())),
            seq: AtomicU64::new(0),
            node,
            sink,
        }
    }

    /// Run journal recovery on boot (idempotent). Returns re-applied op count.
    pub fn recover(&self) -> AppResult<usize> {
        Ok(self.store.recover()?)
    }

    /// The underlying store (read-only accessor for the host, e.g. scheduler boot).
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.store
    }

    // -- internal plumbing ---------------------------------------------------

    /// Next HLC value for an op (ticks the install clock).
    pub(crate) fn next_hlc(&self) -> Hlc {
        let mut guard = self
            .hlc
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        guard.tick_now();
        guard.clone()
    }

    /// Commit one op through the single writer, mapping the storage error class.
    pub(crate) fn commit(&self, op: &EntityOp) -> AppResult<()> {
        self.store.commit(op).map_err(AppError::from)
    }

    /// Read through the single writer connection (works for file and memory DBs).
    pub(crate) fn read<T>(&self, f: impl FnOnce(&Connection) -> StorageResult<T>) -> AppResult<T> {
        self.store.db().with_writer_conn(f).map_err(AppError::from)
    }

    /// Emit an [`AppEvent`] with the next monotonic `seq` (HLD §7).
    pub(crate) fn emit(&self, event: AppEvent) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        (self.sink)(SequencedEvent::new(seq, event));
    }

    /// Current wall instant in epoch-ms UTC.
    pub(crate) fn now_ms(&self) -> i64 {
        Timestamp::now().as_millis()
    }
}

/// Provision the SQLCipher master key for the store at `paths`: the OS keystore
/// first (Keychain / Credential Manager / Secret Service), then a `0600` dev-file
/// beside the DB as a headless fallback (Data Model §13.1; the fallback warns
/// loudly). `storage`'s `os-keystore` feature is on by default, so
/// [`storage::KeyringKeyStore`] exists here.
///
/// # Errors
/// Returns [`AppError::Storage`] if neither backend can yield a key.
pub fn provision_master_key(paths: &Paths) -> AppResult<KeyMaterial> {
    match storage::keystore::provision_db_key(&storage::KeyringKeyStore::new()) {
        Ok(k) => Ok(k),
        Err(e) => {
            tracing::warn!(error = %e, "OS keystore unavailable; falling back to dev key file");
            let dev = DevFileKeyStore::new(paths.root().join(".dev-db-key"));
            Ok(storage::keystore::provision_db_key(&dev)?)
        }
    }
}

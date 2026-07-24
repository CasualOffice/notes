//! The [`CalendarSyncAdapter`] trait, [`CalendarCapability`] tiers, the shared
//! sync value types, and a crash-safe [`LocalCalendarState`] reconciler.
//!
//! See `docs/casual-note-calendar.md` §3. One trait fronts all three tiers
//! (Native / CalDAV / ICS); every adapter reports its capability *honestly* so the
//! UI can tell the truth per platform and never silently downgrade (doc §3, §9).
//!
//! Sync-specific error type is [`SyncError`](crate::error::SyncError); the pure
//! ICS/model core keeps using [`CalendarError`](crate::error::CalendarError).

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::conflict::{self, Winner};
use crate::error::SyncResult;
use crate::model::CalendarEvent;

/// A reference to a remote calendar collection: a CalDAV collection URL, or a
/// native store's opaque calendar identifier. Opaque to this crate.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CalId(pub String);

impl CalId {
    /// Borrow the underlying string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<&str> for CalId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl From<String> for CalId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl fmt::Display for CalId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An opaque incremental-pull cursor: a CalDAV RFC 6578 `sync-token` or a native
/// change token. [`SyncToken::initial`] requests a full (idempotent) resync.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncToken(pub Option<String>);

impl SyncToken {
    /// The empty token — asks the adapter for a full initial sync.
    #[must_use]
    pub const fn initial() -> Self {
        Self(None)
    }

    /// Wrap a concrete token string.
    #[must_use]
    pub fn some(token: impl Into<String>) -> Self {
        Self(Some(token.into()))
    }

    /// The token string, if any (`None` = full resync requested).
    #[must_use]
    pub fn as_deref(&self) -> Option<&str> {
        self.0.as_deref()
    }
}

/// Honest capability report for one adapter (doc §3.1). The variant names the tier
/// and its read/write reach; `Unavailable` is the loud, non-downgrading answer a
/// deferred/unpermitted backend gives instead of pretending to be something else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalendarCapability {
    /// Tier A — a native OS calendar store (EventKit / EDS / AppointmentManager).
    Native {
        /// Can enumerate/pull events.
        read: bool,
        /// Can create/update/delete events.
        write: bool,
    },
    /// Tier B — a CalDAV collection (the universal two-way path).
    CalDav {
        /// Can enumerate/pull events.
        read: bool,
        /// Can create/update/delete events.
        write: bool,
    },
    /// Tier C — one-way ICS interchange only.
    IcsOnly,
    /// No sync possible right now (backend absent, unpermitted, or not implemented).
    Unavailable,
}

impl CalendarCapability {
    /// Whether the adapter can pull events.
    #[must_use]
    pub const fn can_read(self) -> bool {
        match self {
            Self::Native { read, .. } | Self::CalDav { read, .. } => read,
            Self::IcsOnly | Self::Unavailable => false,
        }
    }

    /// Whether the adapter can push (create/update/delete) events.
    #[must_use]
    pub const fn can_write(self) -> bool {
        match self {
            Self::Native { write, .. } | Self::CalDav { write, .. } => write,
            Self::IcsOnly | Self::Unavailable => false,
        }
    }

    /// A short, stable tier label for UI/logging (`"native"`, `"caldav"`,
    /// `"ics"`, `"unavailable"`).
    #[must_use]
    pub const fn tier(self) -> &'static str {
        match self {
            Self::Native { .. } => "native",
            Self::CalDav { .. } => "caldav",
            Self::IcsOnly => "ics",
            Self::Unavailable => "unavailable",
        }
    }
}

/// A calendar collection as discovered on the remote (doc §3.1 `RemoteCalendar`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteCalendar {
    /// The collection reference (CalDAV href/URL or native id) used for pull/push.
    pub id: CalId,
    /// Display name.
    pub name: String,
    /// Display color (`#rrggbb`), if the server advertises one.
    pub color: Option<String>,
    /// Whether the current principal may write to this collection.
    pub writable: bool,
    /// CalDAV collection ctag (cheap change detector), if any.
    pub ctag: Option<String>,
    /// Current sync-token, if the server returned one during discovery.
    pub sync_token: SyncToken,
    /// Default IANA timezone advertised by the collection, if any.
    pub tz: Option<String>,
}

/// One event as it lives on the remote: its resource path plus the parsed event
/// (whose [`etag`](CalendarEvent::etag) carries the CalDAV `getetag`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEvent {
    /// The CalDAV resource href (or native resource id) addressing this event.
    pub href: String,
    /// The parsed event, with `etag` populated.
    pub event: CalendarEvent,
}

/// The incremental delta returned by [`CalendarSyncAdapter::pull`] (doc §3.2). A
/// full resync (`since == SyncToken::initial`) returns every event as `changed`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChangeSet {
    /// Created or updated events since `since`.
    pub changed: Vec<RemoteEvent>,
    /// Resource hrefs removed since `since` (RFC 6578 `404` tombstones).
    pub deleted: Vec<String>,
    /// The cursor to pass to the next pull; persisted per-calendar.
    pub next_token: SyncToken,
}

/// A mutation to apply to a writable remote (doc §3.1 `EventOp`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventOp {
    /// Create a new event. Uses `If-None-Match: *` so a server-side duplicate
    /// (same href) is detected as a conflict rather than clobbered.
    Create(CalendarEvent),
    /// Update an existing event. `If-Match: <etag>` guards optimistic concurrency
    /// (a stale `etag` yields [`PushOutcome::Conflict`]).
    Update {
        /// The new event body.
        event: CalendarEvent,
        /// The href to `PUT` to (server-assigned; may differ from `uid.ics`).
        href: String,
        /// The last-known `ETag` for the `If-Match` precondition.
        etag: Option<String>,
    },
    /// Delete an event. `If-Match: <etag>` guards concurrency.
    Delete {
        /// The `UID` of the event (for result correlation and revision-keeping).
        uid: String,
        /// The href to `DELETE`.
        href: String,
        /// The last-known `ETag`.
        etag: Option<String>,
    },
}

impl EventOp {
    /// The `UID` this op targets (for correlating with [`PushResult`]).
    #[must_use]
    pub fn uid(&self) -> &str {
        match self {
            Self::Create(e) | Self::Update { event: e, .. } => &e.uid,
            Self::Delete { uid, .. } => uid,
        }
    }
}

/// What happened to one pushed [`EventOp`] (doc §3.1 `PushResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PushOutcome {
    /// Create/update succeeded; carries the new server `ETag` when returned.
    Written {
        /// The href the event now lives at.
        href: String,
        /// The new `ETag`, if the server echoed one.
        etag: Option<String>,
    },
    /// Delete succeeded.
    Deleted,
    /// The `If-Match`/`If-None-Match` precondition failed (HTTP 412): the remote
    /// changed under us. **Detected, never dropped** — the caller resolves and
    /// preserves the losing local edit as a revision (doc §3.2).
    Conflict {
        /// The stale `ETag` we sent, if any.
        local_etag: Option<String>,
    },
}

/// The result of one pushed op, correlated by `uid`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushResult {
    /// The `UID` of the pushed event.
    pub uid: String,
    /// The outcome for that op.
    pub outcome: PushOutcome,
}

/// The one trait fronting all three sync tiers (doc §3.1).
///
/// Implemented by [`CalDavClient`](crate::caldav::CalDavClient) (Tier B), the
/// native stubs in [`crate::adapters`] (Tier A), and
/// [`MockSyncAdapter`](crate::adapters::MockSyncAdapter) for tests.
// `async fn` in trait is intentional here (see `Transport`): adapters run on a
// single task; no `Send` bound is needed.
#[allow(async_fn_in_trait)]
pub trait CalendarSyncAdapter {
    /// Honest capability report for this adapter/platform (doc §3, §9).
    fn capability(&self) -> CalendarCapability;

    /// Enumerate the writable/readable calendars this adapter exposes.
    async fn list_calendars(&self) -> SyncResult<Vec<RemoteCalendar>>;

    /// Incremental pull since `since` (CalDAV sync-token / native change token).
    /// `SyncToken::initial` performs a full, idempotent resync.
    async fn pull(&self, cal: &CalId, since: &SyncToken) -> SyncResult<ChangeSet>;

    /// Apply create/update/delete ops to a writable calendar, returning a
    /// per-op [`PushResult`] (a 412 surfaces as [`PushOutcome::Conflict`]).
    async fn push(&self, cal: &CalId, ops: &[EventOp]) -> SyncResult<Vec<PushResult>>;
}

/// A stored event plus its remote resource href (server-assigned naming).
#[derive(Clone, Debug, PartialEq, Eq)]
struct StoredEvent {
    event: CalendarEvent,
    href: Option<String>,
}

/// A minimal, crash-safe-shaped local mirror of one calendar with the reconcile
/// logic that ties [`ChangeSet`] pulls, local edits, and [`PushResult`]s together
/// (doc §3.2). Keyed by iCalendar `UID`.
///
/// The invariant it upholds: **a losing local edit is never dropped** — when a
/// remote version wins a conflict, the superseded local copy is moved into
/// [`revisions`](Self::revisions) for review, exactly as doc §3.2 requires. Pull
/// is idempotent: re-applying the same [`ChangeSet`] leaves the state unchanged.
#[derive(Debug, Default)]
pub struct LocalCalendarState {
    events: HashMap<String, StoredEvent>,
    dirty: HashSet<String>,
    /// Losing local edits preserved for review (doc §3.2), never discarded.
    pub revisions: Vec<CalendarEvent>,
    /// The last sync cursor applied.
    pub sync_token: SyncToken,
}

impl LocalCalendarState {
    /// An empty state, before the first pull.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up an event by `UID`.
    #[must_use]
    pub fn get(&self, uid: &str) -> Option<&CalendarEvent> {
        self.events.get(uid).map(|s| &s.event)
    }

    /// Number of live events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the state holds no events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// UIDs of events with unpushed local edits.
    #[must_use]
    pub fn dirty_uids(&self) -> Vec<String> {
        let mut v: Vec<String> = self.dirty.iter().cloned().collect();
        v.sort();
        v
    }

    /// Record a local edit: upsert the event and mark it dirty (pending push).
    /// The caller is expected to have bumped `sequence`/`last_modified` already;
    /// this does not mutate those (projection/edit code owns them).
    pub fn local_edit(&mut self, event: CalendarEvent) {
        let uid = event.uid.clone();
        let href = self.events.get(&uid).and_then(|s| s.href.clone());
        self.events.insert(uid.clone(), StoredEvent { event, href });
        self.dirty.insert(uid);
    }

    /// Apply an incremental [`ChangeSet`] from [`CalendarSyncAdapter::pull`].
    ///
    /// - A remote change to a **clean** event upserts it (last-writer is the
    ///   remote; nothing to preserve).
    /// - A remote change to a **dirty** event is resolved by
    ///   [`conflict::resolve`]: if the remote wins, the local edit is moved to
    ///   [`revisions`](Self::revisions) and the remote installed (dirty cleared);
    ///   if the local wins, the local copy is kept and stays dirty for re-push.
    /// - A remote **delete** removes a clean event; a dirty event survives (its
    ///   pending local edit will re-create it on the next push) but the losing
    ///   remote state is noted by leaving it dirty.
    ///
    /// Idempotent: re-applying the same `ChangeSet` is a no-op on state and adds
    /// no duplicate revision.
    pub fn apply_pull(&mut self, changeset: ChangeSet) {
        for remote in changeset.changed {
            let uid = remote.event.uid.clone();
            let href = Some(remote.href);
            let is_dirty = self.dirty.contains(&uid);
            // Clone the existing local event (if any) so the `self.events` borrow
            // ends here, leaving the mutations below unencumbered.
            let existing = self.events.get(&uid).map(|s| s.event.clone());

            match existing {
                Some(local) if is_dirty => {
                    let outcome = conflict::resolve(&local, &remote.event, true);
                    match outcome.winner {
                        Winner::Remote => {
                            if let Some(preserved) = outcome.preserved_local {
                                self.revisions.push(preserved);
                            }
                            self.events.insert(
                                uid.clone(),
                                StoredEvent {
                                    event: outcome.merged,
                                    href,
                                },
                            );
                            self.dirty.remove(&uid);
                        }
                        Winner::Local => {
                            // Local edit is authoritative; keep it dirty so it
                            // re-pushes. Adopt the remote href for the next PUT.
                            if let Some(slot) = self.events.get_mut(&uid) {
                                slot.href = href;
                            }
                        }
                    }
                }
                _ => {
                    // Clean (or unknown) — remote is the latest writer.
                    self.events.insert(
                        uid,
                        StoredEvent {
                            event: remote.event,
                            href,
                        },
                    );
                }
            }
        }

        for href in changeset.deleted {
            // Deletions arrive by href; find the matching uid.
            let matched: Option<String> = self
                .events
                .iter()
                .find(|(_, s)| s.href.as_deref() == Some(href.as_str()))
                .map(|(uid, _)| uid.clone());
            if let Some(uid) = matched {
                if self.dirty.contains(&uid) {
                    // Remote deleted an event the user just edited: keep the local
                    // edit (it will re-create), do not drop it.
                    continue;
                }
                self.events.remove(&uid);
            }
        }

        self.sync_token = changeset.next_token;
    }

    /// Build the pending [`EventOp`]s for every dirty event, ready to hand to
    /// [`CalendarSyncAdapter::push`]. An event with a known href/etag becomes an
    /// `Update`; a never-synced one becomes a `Create`.
    #[must_use]
    pub fn pending_ops(&self) -> Vec<EventOp> {
        self.dirty_uids()
            .into_iter()
            .filter_map(|uid| self.events.get(&uid))
            .map(|s| match &s.href {
                Some(href) => EventOp::Update {
                    event: s.event.clone(),
                    href: href.clone(),
                    etag: s.event.etag.clone(),
                },
                None => EventOp::Create(s.event.clone()),
            })
            .collect()
    }

    /// Apply the [`PushResult`]s from a push: clear the dirty flag and record the
    /// new href/etag on success; a [`PushOutcome::Conflict`] leaves the event
    /// dirty so the next pull resolves it (and preserves the local edit if it
    /// loses). Returns the UIDs that conflicted (for the caller to re-pull).
    pub fn apply_push_results(&mut self, results: &[PushResult]) -> Vec<String> {
        let mut conflicts = Vec::new();
        for r in results {
            match &r.outcome {
                PushOutcome::Written { href, etag } => {
                    if let Some(slot) = self.events.get_mut(&r.uid) {
                        slot.href = Some(href.clone());
                        slot.event.etag = etag.clone();
                    }
                    self.dirty.remove(&r.uid);
                }
                PushOutcome::Deleted => {
                    self.events.remove(&r.uid);
                    self.dirty.remove(&r.uid);
                }
                PushOutcome::Conflict { .. } => {
                    conflicts.push(r.uid.clone());
                }
            }
        }
        conflicts
    }
}

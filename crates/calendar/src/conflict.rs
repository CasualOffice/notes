//! Two-way sync conflict resolution (doc §3.2).
//!
//! See `docs/casual-note-calendar.md` §3.2. The rules, verbatim:
//!
//! - **Identity** by iCalendar `UID` (the caller pairs `local`/`remote` by it).
//! - **Version** by `SEQUENCE`, then `LAST-MODIFIED` as the tiebreaker.
//! - CalDAV concurrency is enforced out-of-band by the `ETag` `If-Match`
//!   precondition (see [`crate::caldav`]); this module decides the *semantic*
//!   winner once both versions are in hand.
//! - **Last-writer-wins by `LAST-MODIFIED`**, but a losing **local** edit is
//!   never discarded silently — it is returned as
//!   [`ConflictOutcome::preserved_local`] for the caller to keep as a revision
//!   (consistent with the "user edit is authoritative" rule).
//!
//! This module is pure: no I/O, deterministic, and total.

use crate::model::CalendarEvent;

/// Which side won a conflict.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Winner {
    /// The local edit is authoritative and should be (re-)pushed.
    Local,
    /// The remote version is newer and should be installed locally.
    Remote,
}

/// The decision for one `UID` conflict.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictOutcome {
    /// Which side won.
    pub winner: Winner,
    /// The canonical version to keep going forward (the winner's event).
    pub merged: CalendarEvent,
    /// A losing **local** edit, preserved for review — `Some` only when the
    /// remote won *and* the local copy was a genuine, differing local edit
    /// (doc §3.2: "a losing local edit is never discarded silently").
    pub preserved_local: Option<CalendarEvent>,
}

/// `LAST-MODIFIED` as epoch-ms, treating an absent value as the epoch (oldest).
fn last_modified_ms(event: &CalendarEvent) -> i64 {
    event.last_modified.map_or(i64::MIN, |t| t.as_millis())
}

/// Resolve a `UID`-matched pair per doc §3.2.
///
/// `local_dirty` says whether the local copy carries an unpushed edit (only then
/// can there be something to preserve). Precedence:
/// 1. Higher `SEQUENCE` wins.
/// 2. On equal `SEQUENCE`, later `LAST-MODIFIED` wins.
/// 3. On a full tie, the **remote** wins (stable, converges both peers) — but a
///    differing dirty local copy is still preserved.
///
/// When the remote wins and `local_dirty` is set and the two events actually
/// differ, the local edit is returned in
/// [`preserved_local`](ConflictOutcome::preserved_local).
#[must_use]
pub fn resolve(
    local: &CalendarEvent,
    remote: &CalendarEvent,
    local_dirty: bool,
) -> ConflictOutcome {
    let local_wins = if local.sequence != remote.sequence {
        local.sequence > remote.sequence
    } else {
        // Equal SEQUENCE: newer LAST-MODIFIED wins; exact tie -> remote.
        last_modified_ms(local) > last_modified_ms(remote)
    };

    if local_wins {
        ConflictOutcome {
            winner: Winner::Local,
            merged: local.clone(),
            preserved_local: None,
        }
    } else {
        let preserved_local = if local_dirty && local != remote {
            Some(local.clone())
        } else {
            None
        };
        ConflictOutcome {
            winner: Winner::Remote,
            merged: remote.clone(),
            preserved_local,
        }
    }
}

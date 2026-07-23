//! The `reminder` entity (Data Model §7.1) and its mutating operations.
//!
//! `fire_at` is the authoritative instant (absolute UTC ms) and `tz` the IANA zone
//! so DST math is reconstructable. Every mutation that changes the schedule
//! **clears `os_handle`**: the stored Layer-B handle is invalidated at the model
//! level (the scheduler must `cancel()` it before re-registering — HLD §8.3
//! "any mutation cancels the stored `os_handle` first").

use app_domain::{EntityKind, EntityRef, Id, RecurrenceRuleId, ReminderId, Timestamp};
use serde::{Deserialize, Serialize};

use crate::error::ReminderError;
use crate::state::{OsLayer, ReminderState};

/// The kind of a reminder's polymorphic target (`reminder.target_kind` CHECK:
/// `task | note | session | NULL`). A `None` target is a *standalone* reminder.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderTargetKind {
    Task,
    Note,
    Session,
}

impl ReminderTargetKind {
    /// The exact string stored in `reminder.target_kind`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Note => "note",
            Self::Session => "session",
        }
    }

    /// Parse from the stored `reminder.target_kind` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "task" => Self::Task,
            "note" => Self::Note,
            "session" => Self::Session,
            _ => return None,
        })
    }

    /// The corresponding spine [`EntityKind`].
    #[must_use]
    pub const fn as_entity_kind(&self) -> EntityKind {
        match self {
            Self::Task => EntityKind::Task,
            Self::Note => EntityKind::Note,
            Self::Session => EntityKind::Session,
        }
    }

    /// Narrow a general [`EntityKind`] to a valid reminder target, if it is one.
    #[must_use]
    pub const fn from_entity_kind(kind: EntityKind) -> Option<Self> {
        match kind {
            EntityKind::Task => Some(Self::Task),
            EntityKind::Note => Some(Self::Note),
            EntityKind::Session => Some(Self::Session),
            _ => None,
        }
    }
}

/// A reminder's polymorphic pointer: the authoritative `target_kind`/`target_id`
/// pair (Data Model §7.1). The `link(rel='reminds')` edge mirrors this for graph
/// traversal; these columns are the fast-lookup source of truth.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ReminderTarget {
    pub kind: ReminderTargetKind,
    pub id: Id,
    /// Optional anchor to a specific block within the target (`target_block_id`).
    pub block_id: Option<BlockRef>,
}

/// A `target_block_id` anchor (short nanoid inside `doc_json`). Kept as a thin
/// newtype so the field is self-documenting on the wire.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockRef(pub String);

impl ReminderTarget {
    /// A whole-entity target (no block anchor).
    #[must_use]
    pub const fn entity(kind: ReminderTargetKind, id: Id) -> Self {
        Self {
            kind,
            id,
            block_id: None,
        }
    }

    /// This target as a spine [`EntityRef`] (drops the block anchor; used for the
    /// `target_ref` carried on [`app_domain::AppEvent::ReminderFired`]).
    #[must_use]
    pub const fn as_entity_ref(&self) -> EntityRef {
        EntityRef::new(self.kind.as_entity_kind(), self.id)
    }
}

/// A `reminder` row (Data Model §7.1). All scheduler layers derive from this.
///
/// This is the in-memory projection; `storage` maps it to/from the SQLite row.
/// The struct never touches the DB or the OS — it is pure domain logic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Reminder {
    /// `entity_id` (UUIDv7, `kind='reminder'`).
    pub entity_id: ReminderId,
    /// Polymorphic target; `None` == standalone reminder.
    pub target: Option<ReminderTarget>,
    /// Authoritative fire instant (absolute UTC ms).
    pub fire_at: Timestamp,
    /// IANA zone for DST-safe reconstruction (e.g. `"America/New_York"`).
    pub tz: String,
    /// Lifecycle state.
    pub state: ReminderState,
    /// Re-arm instant while `state == Snoozed`.
    pub snoozed_until: Option<Timestamp>,
    /// Layer-B OS notification identifier (`None` on Linux / when unscheduled).
    pub os_handle: Option<String>,
    /// Which OS layer owns `os_handle` (`None` on Linux).
    pub os_layer: Option<OsLayer>,
    /// Optional `recurrence_rule` this reminder is an instance of.
    pub recurrence_id: Option<RecurrenceRuleId>,
    /// Notification text override.
    pub body: Option<String>,
    /// Creation instant.
    pub created_at: Timestamp,
}

impl Reminder {
    /// Create a fresh standalone `pending` reminder.
    #[must_use]
    pub fn new(fire_at: Timestamp, tz: impl Into<String>) -> Self {
        Self {
            entity_id: Id::new(),
            target: None,
            fire_at,
            tz: tz.into(),
            state: ReminderState::Pending,
            snoozed_until: None,
            os_handle: None,
            os_layer: None,
            recurrence_id: None,
            body: None,
            created_at: Timestamp::now(),
        }
    }

    /// Attach a polymorphic target (builder style).
    #[must_use]
    pub fn with_target(mut self, target: ReminderTarget) -> Self {
        self.target = Some(target);
        self
    }

    /// The instant the scheduler should arm on: `snoozed_until` while snoozed,
    /// otherwise the authoritative `fire_at`. Snooze never destroys the original
    /// `fire_at` provenance (Architecture §6.1 invariant 4).
    #[must_use]
    pub fn effective_fire_at(&self) -> Timestamp {
        match (self.state, self.snoozed_until) {
            (ReminderState::Snoozed, Some(until)) => until,
            _ => self.fire_at,
        }
    }

    /// The spine `target_ref` for events (`None` for standalone reminders).
    #[must_use]
    pub fn target_ref(&self) -> Option<EntityRef> {
        self.target.as_ref().map(ReminderTarget::as_entity_ref)
    }

    /// Whether this reminder currently feeds the scheduler (active state).
    #[must_use]
    pub const fn is_schedulable(&self) -> bool {
        self.state.is_active()
    }

    // -- state-machine transitions -----------------------------------------

    /// Validate then apply a raw state transition. Prefer the named helpers below.
    fn transition(&mut self, next: ReminderState) -> Result<(), ReminderError> {
        if !self.state.can_transition_to(next) {
            return Err(ReminderError::IllegalTransition {
                from: self.state.as_str(),
                to: next.as_str(),
            });
        }
        self.state = next;
        Ok(())
    }

    /// Deliver the reminder (`pending|snoozed → fired`). Called by whichever
    /// layer wins the state-gated de-dup race (HLD §8.3).
    pub fn fire(&mut self) -> Result<(), ReminderError> {
        self.transition(ReminderState::Fired)
    }

    /// Snooze until `until` (`pending|snoozed|fired|missed → snoozed`). Clears the
    /// stale `os_handle`; the scheduler re-registers Layer B for the new instant.
    pub fn snooze(&mut self, until: Timestamp) -> Result<(), ReminderError> {
        self.transition(ReminderState::Snoozed)?;
        self.snoozed_until = Some(until);
        self.clear_os_handle();
        Ok(())
    }

    /// Re-arm a snoozed reminder once `snoozed_until` is reached
    /// (`snoozed → pending`). Keeps the original `fire_at`; clears the snooze.
    pub fn rearm(&mut self) -> Result<(), ReminderError> {
        self.transition(ReminderState::Pending)?;
        self.snoozed_until = None;
        self.clear_os_handle();
        Ok(())
    }

    /// Cancel the reminder (any non-terminal state → `canceled`).
    pub fn cancel(&mut self) -> Result<(), ReminderError> {
        self.transition(ReminderState::Canceled)?;
        self.clear_os_handle();
        Ok(())
    }

    /// Acknowledge a delivered/missed reminder (`fired|missed|pending|snoozed →
    /// dismissed`).
    pub fn dismiss(&mut self) -> Result<(), ReminderError> {
        self.transition(ReminderState::Dismissed)?;
        self.clear_os_handle();
        Ok(())
    }

    /// Mark as missed by the catch-up sweep (`pending|snoozed → missed`).
    pub fn mark_missed(&mut self) -> Result<(), ReminderError> {
        self.transition(ReminderState::Missed)
    }

    /// Record the Layer-B registration produced by a successful OS `schedule()`.
    pub fn set_os_registration(&mut self, handle: impl Into<String>, layer: OsLayer) {
        self.os_handle = Some(handle.into());
        self.os_layer = Some(layer);
    }

    /// Drop any stored Layer-B handle (invalidated by a schedule-changing mutation).
    pub fn clear_os_handle(&mut self) {
        self.os_handle = None;
        self.os_layer = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_millis(ms)
    }

    #[test]
    fn target_kind_roundtrip_and_entity_kind() {
        assert_eq!(
            ReminderTargetKind::from_db_str("session"),
            Some(ReminderTargetKind::Session)
        );
        assert_eq!(ReminderTargetKind::Task.as_entity_kind(), EntityKind::Task);
        assert_eq!(ReminderTargetKind::from_entity_kind(EntityKind::Area), None);
        assert_eq!(
            ReminderTargetKind::from_entity_kind(EntityKind::Note),
            Some(ReminderTargetKind::Note)
        );
    }

    #[test]
    fn effective_fire_at_prefers_snooze() {
        let mut r = Reminder::new(ts(1_000), "UTC");
        assert_eq!(r.effective_fire_at(), ts(1_000));
        r.snooze(ts(5_000)).unwrap();
        assert_eq!(r.effective_fire_at(), ts(5_000));
        // original fire_at is preserved as provenance
        assert_eq!(r.fire_at, ts(1_000));
    }

    #[test]
    fn snooze_clears_os_handle() {
        let mut r = Reminder::new(ts(1_000), "UTC");
        r.set_os_registration("os-123", OsLayer::Toast);
        assert!(r.os_handle.is_some());
        r.snooze(ts(2_000)).unwrap();
        assert!(r.os_handle.is_none());
        assert!(r.os_layer.is_none());
    }

    #[test]
    fn illegal_transition_is_rejected() {
        let mut r = Reminder::new(ts(1), "UTC");
        r.cancel().unwrap();
        let err = r.fire().unwrap_err();
        assert!(matches!(err, ReminderError::IllegalTransition { .. }));
        assert_eq!(r.state, ReminderState::Canceled);
    }

    #[test]
    fn rearm_restores_pending_and_keeps_fire_at() {
        let mut r = Reminder::new(ts(1_000), "UTC");
        r.snooze(ts(9_000)).unwrap();
        r.rearm().unwrap();
        assert_eq!(r.state, ReminderState::Pending);
        assert_eq!(r.snoozed_until, None);
        assert_eq!(r.effective_fire_at(), ts(1_000));
    }

    #[test]
    fn target_ref_maps_to_entity_ref() {
        let tid = Id::new();
        let r = Reminder::new(ts(1), "UTC")
            .with_target(ReminderTarget::entity(ReminderTargetKind::Task, tid));
        let er = r.target_ref().unwrap();
        assert_eq!(er.kind, EntityKind::Task);
        assert_eq!(er.id, tid);
    }
}

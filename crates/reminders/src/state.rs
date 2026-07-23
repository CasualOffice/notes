//! The `reminder.state` machine and the `os_layer` tag. Implements Data Model
//! §7.1 (`CHECK (state IN ('pending','fired','snoozed','missed','dismissed',
//! 'canceled'))` and `os_layer IN ('uncalendar','toast') OR NULL`).

use serde::{Deserialize, Serialize};

/// The lifecycle state of a `reminder` row (Data Model §7.1).
///
/// Legal transitions (this crate's authoritative state machine — the Data Model
/// fixes the *values*, this fixes the *edges*):
///
/// ```text
///   pending   → fired | snoozed | missed | dismissed | canceled
///   snoozed   → pending | fired | missed | dismissed | canceled
///   fired     → snoozed | dismissed | canceled
///   missed    → snoozed | dismissed | canceled
///   dismissed → ∅  (terminal)
///   canceled  → ∅  (terminal)
/// ```
///
/// Only `pending` and `snoozed` are *active* (schedulable); they match the
/// partial index `idx_reminder_fire ... WHERE state IN ('pending','snoozed')`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReminderState {
    /// Armed and waiting to fire. The default (`DEFAULT 'pending'`).
    Pending,
    /// Delivered (Layer A or Layer B won the state-gated de-dup race).
    Fired,
    /// User-deferred; `snoozed_until` holds the re-arm instant.
    Snoozed,
    /// The app was closed past `fire_at`; surfaced by the catch-up sweep.
    Missed,
    /// User acknowledged the notification.
    Dismissed,
    /// User (or a mutation) cancelled the reminder.
    Canceled,
}

impl ReminderState {
    /// The exact lowercase string stored in `reminder.state`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Fired => "fired",
            Self::Snoozed => "snoozed",
            Self::Missed => "missed",
            Self::Dismissed => "dismissed",
            Self::Canceled => "canceled",
        }
    }

    /// Parse from the stored `reminder.state` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "pending" => Self::Pending,
            "fired" => Self::Fired,
            "snoozed" => Self::Snoozed,
            "missed" => Self::Missed,
            "dismissed" => Self::Dismissed,
            "canceled" => Self::Canceled,
            _ => return None,
        })
    }

    /// Whether the reminder is schedulable (feeds Layer A / Layer B). Mirrors the
    /// `idx_reminder_fire` partial-index predicate.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        matches!(self, Self::Pending | Self::Snoozed)
    }

    /// Whether the reminder has reached a terminal state (no further transitions).
    #[must_use]
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Dismissed | Self::Canceled)
    }

    /// Whether the state machine permits `self → next`.
    #[must_use]
    pub const fn can_transition_to(&self, next: Self) -> bool {
        use ReminderState::*;
        matches!(
            (self, next),
            (Pending, Fired | Snoozed | Missed | Dismissed | Canceled)
                | (Snoozed, Pending | Fired | Missed | Dismissed | Canceled)
                | (Fired, Snoozed | Dismissed | Canceled)
                | (Missed, Snoozed | Dismissed | Canceled)
        )
    }
}

/// The OS one-shot layer (Layer B) that owns a scheduled reminder while the app
/// is closed. `None` on Linux — capability reported honestly (Data Model §7.1
/// `os_layer` TEXT, HLD §9.3).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OsLayer {
    /// macOS `UNCalendarNotificationTrigger`.
    Uncalendar,
    /// Windows `ScheduledToastNotification`.
    Toast,
}

impl OsLayer {
    /// The exact string stored in `reminder.os_layer`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Uncalendar => "uncalendar",
            Self::Toast => "toast",
        }
    }

    /// Parse from the stored `reminder.os_layer` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "uncalendar" => Self::Uncalendar,
            "toast" => Self::Toast,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_db_strings_roundtrip() {
        for s in [
            ReminderState::Pending,
            ReminderState::Fired,
            ReminderState::Snoozed,
            ReminderState::Missed,
            ReminderState::Dismissed,
            ReminderState::Canceled,
        ] {
            assert_eq!(ReminderState::from_db_str(s.as_str()), Some(s));
        }
        assert_eq!(ReminderState::from_db_str("bogus"), None);
    }

    #[test]
    fn state_serde_matches_db_str() {
        let j = serde_json::to_string(&ReminderState::Canceled).unwrap();
        assert_eq!(j, "\"canceled\"");
    }

    #[test]
    fn active_and_terminal_partition() {
        assert!(ReminderState::Pending.is_active());
        assert!(ReminderState::Snoozed.is_active());
        assert!(!ReminderState::Fired.is_active());
        assert!(ReminderState::Dismissed.is_terminal());
        assert!(ReminderState::Canceled.is_terminal());
        assert!(!ReminderState::Pending.is_terminal());
    }

    #[test]
    fn transitions_follow_matrix() {
        use ReminderState::*;
        assert!(Pending.can_transition_to(Fired));
        assert!(Pending.can_transition_to(Snoozed));
        assert!(Snoozed.can_transition_to(Pending));
        assert!(Fired.can_transition_to(Dismissed));
        assert!(Missed.can_transition_to(Snoozed));
        // terminal states go nowhere
        assert!(!Dismissed.can_transition_to(Pending));
        assert!(!Canceled.can_transition_to(Fired));
        // fired cannot go back to pending directly
        assert!(!Fired.can_transition_to(Pending));
    }

    #[test]
    fn os_layer_roundtrip() {
        assert_eq!(
            OsLayer::from_db_str("uncalendar"),
            Some(OsLayer::Uncalendar)
        );
        assert_eq!(OsLayer::from_db_str("toast"), Some(OsLayer::Toast));
        assert_eq!(OsLayer::from_db_str(""), None);
        assert_eq!(serde_json::to_string(&OsLayer::Toast).unwrap(), "\"toast\"");
    }
}

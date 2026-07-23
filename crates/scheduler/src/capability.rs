//! Layer-B capability + the value types crossing the OS-backend seam
//! (Architecture §6.1).

use app_domain::{EntityRef, ReminderId, Timestamp};
use reminders::{OsLayer, Reminder};
use serde::{Deserialize, Serialize};

/// The default Layer-B horizon: OS one-shots are registered only for reminders
/// firing within this many days (Architecture §6, HLD §9.3).
pub const DEFAULT_HORIZON_DAYS: u16 = 14;

/// What a platform's Layer B can do (Architecture §6.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SchedulerCapability {
    /// macOS / Windows: OS one-shots fire while the app is closed, within a
    /// rolling horizon.
    Full {
        /// Rolling horizon in days (typically [`DEFAULT_HORIZON_DAYS`]).
        horizon_days: u16,
    },
    /// Linux: no OS layer — honest downgrade to Layer A only (fires only while the
    /// app runs). Reported, never faked (CLAUDE.md capability-honesty invariant).
    RunningOnly,
}

impl SchedulerCapability {
    /// Whether an OS one-shot layer (Layer B) exists at all.
    #[must_use]
    pub const fn has_os_layer(&self) -> bool {
        matches!(self, Self::Full { .. })
    }

    /// Whether delivery is limited to while-running (Layer A only).
    #[must_use]
    pub const fn is_running_only(&self) -> bool {
        matches!(self, Self::RunningOnly)
    }

    /// The Layer-B horizon in days (`0` when there is no OS layer).
    #[must_use]
    pub const fn horizon_days(&self) -> u16 {
        match self {
            Self::Full { horizon_days } => *horizon_days,
            Self::RunningOnly => 0,
        }
    }
}

/// An opaque OS notification handle (`reminder.os_handle`) returned by a Layer-B
/// registration; the scheduler stores it and `cancel()`s it before any reschedule.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OsHandle(pub String);

impl OsHandle {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The minimal descriptor a Layer-B backend (and the Layer-A wheel) needs to arm a
/// reminder. Derived from a [`Reminder`] via [`ScheduledReminder::from_reminder`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ScheduledReminder {
    pub reminder_id: ReminderId,
    /// The instant to fire on (already snooze-resolved — [`Reminder::effective_fire_at`]).
    pub fire_at: Timestamp,
    /// IANA zone for DST-safe OS-side reconstruction.
    pub tz: String,
    /// Notification text override, if any.
    pub body: Option<String>,
    /// The reminder's target for deep-linking on click (`None` == standalone).
    pub target: Option<EntityRef>,
}

impl ScheduledReminder {
    /// Build the scheduler descriptor from a domain [`Reminder`].
    #[must_use]
    pub fn from_reminder(r: &Reminder) -> Self {
        Self {
            reminder_id: r.entity_id,
            fire_at: r.effective_fire_at(),
            tz: r.tz.clone(),
            body: r.body.clone(),
            target: r.target_ref(),
        }
    }

    /// The [`OsLayer`] a backend should stamp when it registers this reminder, for
    /// the given platform capability. `None` when there is no OS layer.
    #[must_use]
    pub const fn os_layer_for(
        cap: &SchedulerCapability,
        platform_layer: OsLayer,
    ) -> Option<OsLayer> {
        match cap {
            SchedulerCapability::Full { .. } => Some(platform_layer),
            SchedulerCapability::RunningOnly => None,
        }
    }
}

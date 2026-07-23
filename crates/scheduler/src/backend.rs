//! **Layer B** — the per-OS one-shot handoff (Architecture §6.1). One trait,
//! three platform backends. macOS/Windows are Phase-1 **stubs** (the real
//! `UNCalendarNotificationTrigger` / `ScheduledToastNotification` bindings land in
//! a later phase); Linux is complete: it reports [`SchedulerCapability::RunningOnly`]
//! and refuses to schedule, honestly (no freedesktop persistence — HLD §9.3).
//!
//! The trait mirrors the Architecture §6.1 sketch and is re-exported as
//! `SchedulerAdapter` (its role name in the task/HLD command surface).

use app_domain::Platform;
use reminders::OsLayer;

use crate::capability::{OsHandle, ScheduledReminder, SchedulerCapability, DEFAULT_HORIZON_DAYS};
use crate::error::SchedulerError;

/// The OS notification backend seam (Architecture §6.1 `OsNotificationBackend`).
///
/// Invariant: any reminder mutation writes SQLite first, patches Layer A, then
/// [`reconcile`](OsNotificationBackend::reconcile)s this backend's 14-day horizon
/// (cancel stale `os_handle`, register new). A backend never invents delivery it
/// cannot provide — [`capability`](OsNotificationBackend::capability) is the truth.
pub trait OsNotificationBackend: Send + Sync {
    /// What this platform's Layer B can do.
    fn capability(&self) -> SchedulerCapability;

    /// Register a one-shot OS notification, returning its handle. Errors with
    /// [`SchedulerError::Unsupported`] on a `RunningOnly` platform.
    fn schedule(&self, r: &ScheduledReminder) -> Result<OsHandle, SchedulerError>;

    /// Cancel a previously-registered handle (before any reschedule).
    fn cancel(&self, handle: &OsHandle) -> Result<(), SchedulerError>;

    /// Re-sync the whole horizon: cancel everything not in `active`, (re)register
    /// everything that is. Called on every launch and wake so Layer B never drifts.
    fn reconcile(&self, active: &[ScheduledReminder]) -> Result<(), SchedulerError>;

    /// The [`OsLayer`] tag this backend stamps on `reminder.os_layer`, if any.
    fn os_layer(&self) -> Option<OsLayer>;

    /// The platform this backend serves.
    fn platform(&self) -> Platform;
}

/// Linux Layer B: **none**. Capability is [`SchedulerCapability::RunningOnly`];
/// only Layer A fires, and only while the app runs. Complete and honest.
#[derive(Debug, Clone, Copy, Default)]
pub struct LinuxBackend;

impl OsNotificationBackend for LinuxBackend {
    fn capability(&self) -> SchedulerCapability {
        SchedulerCapability::RunningOnly
    }

    fn schedule(&self, _r: &ScheduledReminder) -> Result<OsHandle, SchedulerError> {
        Err(SchedulerError::Unsupported(
            "linux has no OS one-shot layer; reminders fire only while Casual Note is open".into(),
        ))
    }

    fn cancel(&self, _handle: &OsHandle) -> Result<(), SchedulerError> {
        // Nothing is ever registered, so cancellation is a no-op (not an error).
        Ok(())
    }

    fn reconcile(&self, _active: &[ScheduledReminder]) -> Result<(), SchedulerError> {
        Ok(())
    }

    fn os_layer(&self) -> Option<OsLayer> {
        None
    }

    fn platform(&self) -> Platform {
        Platform::Linux
    }
}

/// macOS Layer B (**Phase-1 stub**): `UNCalendarNotificationTrigger`. Reports
/// [`SchedulerCapability::Full`]; the scheduling calls are wired in a later phase.
#[derive(Debug, Clone, Copy)]
pub struct MacosBackend {
    horizon_days: u16,
}

impl Default for MacosBackend {
    fn default() -> Self {
        Self {
            horizon_days: DEFAULT_HORIZON_DAYS,
        }
    }
}

impl OsNotificationBackend for MacosBackend {
    fn capability(&self) -> SchedulerCapability {
        SchedulerCapability::Full {
            horizon_days: self.horizon_days,
        }
    }

    fn schedule(&self, _r: &ScheduledReminder) -> Result<OsHandle, SchedulerError> {
        Err(SchedulerError::Unimplemented(
            "macOS UNCalendarNotificationTrigger backend is a Phase-1 stub".into(),
        ))
    }

    fn cancel(&self, _handle: &OsHandle) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unimplemented(
            "macOS backend is a Phase-1 stub".into(),
        ))
    }

    fn reconcile(&self, _active: &[ScheduledReminder]) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unimplemented(
            "macOS backend is a Phase-1 stub".into(),
        ))
    }

    fn os_layer(&self) -> Option<OsLayer> {
        Some(OsLayer::Uncalendar)
    }

    fn platform(&self) -> Platform {
        Platform::Macos
    }
}

/// Windows Layer B (**Phase-1 stub**): `ScheduledToastNotification`. Reports
/// [`SchedulerCapability::Full`]; the scheduling calls are wired in a later phase.
#[derive(Debug, Clone, Copy)]
pub struct WindowsBackend {
    horizon_days: u16,
}

impl Default for WindowsBackend {
    fn default() -> Self {
        Self {
            horizon_days: DEFAULT_HORIZON_DAYS,
        }
    }
}

impl OsNotificationBackend for WindowsBackend {
    fn capability(&self) -> SchedulerCapability {
        SchedulerCapability::Full {
            horizon_days: self.horizon_days,
        }
    }

    fn schedule(&self, _r: &ScheduledReminder) -> Result<OsHandle, SchedulerError> {
        Err(SchedulerError::Unimplemented(
            "Windows ScheduledToastNotification backend is a Phase-1 stub".into(),
        ))
    }

    fn cancel(&self, _handle: &OsHandle) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unimplemented(
            "Windows backend is a Phase-1 stub".into(),
        ))
    }

    fn reconcile(&self, _active: &[ScheduledReminder]) -> Result<(), SchedulerError> {
        Err(SchedulerError::Unimplemented(
            "Windows backend is a Phase-1 stub".into(),
        ))
    }

    fn os_layer(&self) -> Option<OsLayer> {
        Some(OsLayer::Toast)
    }

    fn platform(&self) -> Platform {
        Platform::Windows
    }
}

/// The Layer-B backend for the platform this binary was compiled for. Linux is
/// fully functional (honest `RunningOnly`); macOS/Windows are Phase-1 stubs.
#[must_use]
pub fn platform_backend() -> Box<dyn OsNotificationBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(MacosBackend::default())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(WindowsBackend::default())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        // Linux and any other Unix: no OS one-shot layer.
        Box::new(LinuxBackend)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::{Id, Timestamp};

    fn descriptor() -> ScheduledReminder {
        ScheduledReminder {
            reminder_id: Id::new(),
            fire_at: Timestamp::from_millis(1_000),
            tz: "UTC".into(),
            body: None,
            target: None,
        }
    }

    #[test]
    fn linux_reports_running_only_and_refuses_to_schedule() {
        let b = LinuxBackend;
        assert_eq!(b.capability(), SchedulerCapability::RunningOnly);
        assert!(b.capability().is_running_only());
        assert!(!b.capability().has_os_layer());
        assert!(b.os_layer().is_none());

        let err = b.schedule(&descriptor()).unwrap_err();
        assert!(matches!(err, SchedulerError::Unsupported(_)));
        // cancel/reconcile are no-ops, never errors
        assert!(b.cancel(&OsHandle::new("x")).is_ok());
        assert!(b.reconcile(&[]).is_ok());
    }

    #[test]
    fn full_backends_report_horizon_and_layer() {
        let m = MacosBackend::default();
        assert_eq!(
            m.capability(),
            SchedulerCapability::Full { horizon_days: 14 }
        );
        assert_eq!(m.os_layer(), Some(OsLayer::Uncalendar));
        assert!(m.capability().has_os_layer());

        let w = WindowsBackend::default();
        assert_eq!(w.os_layer(), Some(OsLayer::Toast));
        assert_eq!(w.capability().horizon_days(), 14);
    }

    #[test]
    fn platform_backend_is_constructible() {
        let b = platform_backend();
        // On the Linux CI/build host this is RunningOnly; the point is it builds.
        let _ = b.capability();
    }
}

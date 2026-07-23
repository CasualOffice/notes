//! Scheduler error taxonomy (thiserror; maps into `app_domain::AppError`).

use app_domain::AppError;

/// A fallible scheduler / OS-backend outcome.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// The OS notification backend rejected an operation.
    #[error("os notification backend error: {0}")]
    Backend(String),

    /// The requested operation is not supported on this platform's Layer B
    /// (e.g. `schedule()` on Linux, which is `RunningOnly`). Reported, never faked.
    #[error("os scheduling unsupported on this platform: {0}")]
    Unsupported(String),

    /// A Phase-1 backend stub that a later phase will implement (macOS / Windows).
    #[error("os backend not yet implemented: {0}")]
    Unimplemented(String),

    /// The scheduler's command channel is closed (the driver task has stopped).
    #[error("scheduler task is not running")]
    NotRunning,
}

impl From<SchedulerError> for AppError {
    fn from(e: SchedulerError) -> Self {
        match e {
            // A genuinely absent platform capability is the `Capability` class
            // (HLD §9 — reported, never faked).
            SchedulerError::Unsupported(_) => AppError::Capability(e.to_string()),
            SchedulerError::Backend(_) => AppError::Storage(e.to_string()),
            SchedulerError::Unimplemented(_) => AppError::Internal(e.to_string()),
            SchedulerError::NotRunning => AppError::Internal(e.to_string()),
        }
    }
}

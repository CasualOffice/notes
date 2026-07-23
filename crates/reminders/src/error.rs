//! Error taxonomy for the reminders crate (thiserror; maps into the shared
//! [`AppError`] taxonomy of app-domain — HLD §10).

use app_domain::AppError;

/// A fallible reminder / recurrence operation outcome.
#[derive(Debug, thiserror::Error)]
pub enum ReminderError {
    /// A [`crate::ReminderState`] transition that the state machine forbids.
    #[error("illegal reminder state transition: {from} -> {to}")]
    IllegalTransition {
        /// The current state (`reminder.state`).
        from: &'static str,
        /// The rejected target state.
        to: &'static str,
    },

    /// The stored `recurrence_rule.rrule` string failed to parse / expand
    /// (delegated to the `rrule` crate).
    #[error("recurrence rule error: {0}")]
    Recurrence(String),

    /// A field carried an out-of-contract value (Data Model §7 CHECK).
    #[error("invalid reminder field: {0}")]
    Invalid(String),
}

impl From<ReminderError> for AppError {
    fn from(e: ReminderError) -> Self {
        // Every reminder-crate error is a command-boundary validation failure:
        // an illegal transition, a malformed rule, or an out-of-contract field.
        AppError::Validation(e.to_string())
    }
}

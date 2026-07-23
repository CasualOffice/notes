//! Crate-local error type. Bridges into the workspace [`AppError`] taxonomy
//! (app-domain `error`, HLD §10) so command paths get a stable, typed, retryable
//! surface. Libraries use `thiserror` (CLAUDE.md conventions); the `From` bridge
//! maps each failure onto the correct [`ErrorClass`].

use app_domain::AppError;

/// A failure inside the planning pillar (bucket compilation, order-key math,
/// field validation, or a status transition).
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    /// An `order_key` string is not a canonical fractional index (empty, a
    /// non-alphabet byte, or a trailing zero digit that breaks the fraction
    /// invariant).
    #[error("invalid order key: {0}")]
    InvalidOrderKey(String),

    /// The two neighbours handed to [`crate::order_key::key_between`] are not
    /// strictly increasing (`lo >= hi`), so no key can be placed between them.
    #[error("order-key neighbours out of order: {lo:?} !< {hi:?}")]
    OrderKeyOutOfOrder {
        /// The (would-be) lower neighbour.
        lo: String,
        /// The (would-be) upper neighbour.
        hi: String,
    },

    /// A structured task/project field failed validation (e.g. priority > 3).
    #[error("invalid task field: {0}")]
    InvalidField(String),

    /// A requested status transition is not permitted for the current state.
    #[error("illegal status transition: {0}")]
    IllegalTransition(String),
}

impl From<TaskError> for AppError {
    fn from(e: TaskError) -> Self {
        match e {
            // A rejected transition is a concurrency/consistency conflict.
            TaskError::IllegalTransition(_) => Self::Conflict(e.to_string()),
            // Everything else is a boundary-validation failure.
            TaskError::InvalidOrderKey(_)
            | TaskError::OrderKeyOutOfOrder { .. }
            | TaskError::InvalidField(_) => Self::Validation(e.to_string()),
        }
    }
}

/// Convenience alias for fallible planning-pillar functions.
pub type TaskResult<T> = Result<T, TaskError>;

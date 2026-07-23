//! Later-phase command stubs. Meeting intelligence, the grounded-AI workspace,
//! model management, and export land in Phases 2–3 (HLD §8.4/§8.5). Until then the
//! router returns a typed, honest "not implemented in this phase" error rather than
//! pretending — consistent with the capability-honesty invariant (CLAUDE.md).

use app_domain::AppError;

/// The typed error a not-yet-implemented command returns. Terminal (non-retryable);
/// the WebView surfaces it as an actionable "coming in a later phase" notice.
#[must_use]
pub fn not_implemented(command: &str) -> AppError {
    AppError::Internal(format!(
        "{command} is not yet implemented in this phase (Phase-1 core notebook)"
    ))
}

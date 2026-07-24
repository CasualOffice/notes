//! Error taxonomy for the calendar engine.
//!
//! See `docs/casual-note-calendar.md`. Every fallible path returns
//! [`CalendarError`] (via [`CalendarResult`]); no `unwrap()` is used on a
//! fallible runtime path (CLAUDE.md conventions). A [`From`] conversion into the
//! workspace [`AppError`](app_domain::AppError) is provided so the later sync /
//! app-service layers can bubble these into the unified command error surface.

use app_domain::AppError;

/// Convenience alias used throughout the crate.
pub type CalendarResult<T> = Result<T, CalendarError>;

/// Failures raised by ICS parsing/serialization, recurrence expansion, and
/// projection. Modeled with `thiserror` per the Architecture error taxonomy.
#[derive(Debug, thiserror::Error)]
pub enum CalendarError {
    /// A VCALENDAR / VEVENT stream was malformed. `line` is the 1-based unfolded
    /// content-line index when known.
    #[error("ics parse error{}: {message}", match .line { Some(n) => format!(" at line {n}"), None => String::new() })]
    IcsParse {
        /// 1-based unfolded content-line index, when known.
        line: Option<usize>,
        /// Human-readable description of what failed to parse.
        message: String,
    },

    /// Serializing the model back to RFC 5545 text failed (should be rare — most
    /// serialization is infallible; reserved for invariant violations).
    #[error("ics serialize error: {0}")]
    IcsSerialize(String),

    /// A DATE / DATE-TIME value could not be interpreted.
    #[error("invalid date-time value: {0}")]
    InvalidDateTime(String),

    /// A `TZID` referenced an IANA zone not present in the bundled tz database.
    #[error("unknown timezone: {0}")]
    UnknownTimezone(String),

    /// Recurrence-rule parsing or expansion (via the `rrule` engine) failed.
    #[error("recurrence error: {0}")]
    Recurrence(String),

    /// A projection input could not be mapped to a valid event.
    #[error("projection error: {0}")]
    Projection(String),
}

impl From<CalendarError> for AppError {
    fn from(e: CalendarError) -> Self {
        match e {
            CalendarError::IcsParse { .. } | CalendarError::IcsSerialize(_) => {
                Self::Serialization(e.to_string())
            }
            CalendarError::InvalidDateTime(_)
            | CalendarError::UnknownTimezone(_)
            | CalendarError::Recurrence(_)
            | CalendarError::Projection(_) => Self::Validation(e.to_string()),
        }
    }
}

/// Convenience alias for the sync/transport layer.
pub type SyncResult<T> = Result<T, SyncError>;

/// Failures raised by the calendar **sync** layer (the `CalendarSyncAdapter`
/// implementations, the CalDAV protocol logic, and the [`Transport`] seam).
///
/// See `docs/casual-note-calendar.md` §3. Kept separate from [`CalendarError`]
/// (which covers the pure ICS/model/projection core) so the network-touching
/// surface has its own honest taxonomy — the offline core never produces these.
///
/// [`Transport`]: crate::transport::Transport
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// The underlying [`Transport`](crate::transport::Transport) failed before an
    /// HTTP status was obtained (DNS, TLS, socket, timeout). The real transport is
    /// a documented seam; this crate never opens a socket itself.
    #[error("transport error: {0}")]
    Transport(String),

    /// The server returned an unexpected HTTP status for the request.
    #[error("http {status}: {message}")]
    Http {
        /// The HTTP status code.
        status: u16,
        /// Server-provided or synthesized description.
        message: String,
    },

    /// A CalDAV response violated RFC 4791 / RFC 6578 expectations (e.g. a
    /// `Multi-Status` was missing a required element).
    #[error("caldav protocol error: {0}")]
    Protocol(String),

    /// The DAV `Multi-Status` XML (or a request body) could not be parsed.
    #[error("xml error: {0}")]
    Xml(String),

    /// An `ETag`/`If-Match` precondition failed (HTTP 412) — an optimistic-
    /// concurrency conflict on `uid`. Surfaced per-op as
    /// [`PushOutcome::Conflict`](crate::sync::PushOutcome::Conflict); this variant
    /// is for the whole-request failure case.
    #[error("etag precondition failed (conflict) for uid {uid}")]
    Conflict {
        /// The `UID` of the event whose precondition failed.
        uid: String,
    },

    /// The adapter cannot perform the requested operation (e.g. a read-only
    /// native tier asked to write, or a not-yet-implemented FFI backend). This is
    /// the *honest* failure that replaces a silent downgrade (doc §3).
    #[error("operation not supported by this adapter: {0}")]
    Unsupported(String),

    /// An ICS (de)serialization error while building a `PUT` body or parsing
    /// `calendar-data` from a response.
    #[error(transparent)]
    Ics(#[from] CalendarError),
}

impl From<SyncError> for AppError {
    fn from(e: SyncError) -> Self {
        match e {
            SyncError::Transport(_) | SyncError::Http { .. } => Self::Network(e.to_string()),
            SyncError::Conflict { .. } => Self::Conflict(e.to_string()),
            SyncError::Protocol(_) | SyncError::Xml(_) => Self::Serialization(e.to_string()),
            SyncError::Unsupported(_) => Self::Capability(e.to_string()),
            SyncError::Ics(inner) => inner.into(),
        }
    }
}

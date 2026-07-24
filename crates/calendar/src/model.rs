//! Calendar domain model (doc §4).
//!
//! See `docs/casual-note-calendar.md` §4 ("Data model additions"). These are the
//! pure in-memory types the ICS layer and projection produce/consume; persistence
//! (the `calendar` / `event` / `calendar_account` tables) is the storage crate's
//! job. All timestamps are absolute UTC epoch-milliseconds ([`Timestamp`]); an
//! event additionally carries an IANA `tz` so DATE-TIME values round-trip through
//! RFC 5545 losslessly (all-day DATE, floating, `Z`, and `TZID` forms).

use app_domain::{Id, Timestamp};
use serde::{Deserialize, Serialize};

use crate::error::{CalendarError, CalendarResult};

/// Where a [`Calendar`] comes from (doc §4 `calendar.source`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CalendarSource {
    /// A native OS calendar store (Tier A — EventKit / EDS / AppointmentManager).
    System,
    /// A CalDAV collection on the user's own server (Tier B, RFC 4791).
    CalDav,
    /// A purely local Casual Note calendar (Tier C / offline).
    Local,
}

/// A calendar collection (doc §4 `calendar`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Calendar {
    /// Entity id (UUIDv7).
    pub id: Id,
    /// Human-readable name.
    pub name: String,
    /// Origin tier.
    pub source: CalendarSource,
    /// Optional owning account (`calendar_account.id`); `None` for local calendars.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_ref: Option<Id>,
    /// Display color, typically a `#rrggbb` hex string.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Whether Casual Note may create/update/delete events on this calendar.
    pub writable: bool,
    /// IANA timezone the calendar defaults to (e.g. `"America/New_York"`).
    pub tz: String,
    /// CalDAV sync-token (RFC 6578) for incremental pull; `None` before first sync.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sync_token: Option<String>,
    /// CalDAV collection ctag; a cheap change-detector complementing `sync_token`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ctag: Option<String>,
    /// Whether sync is opt-in-enabled for this calendar (doc §1: opt-in per calendar).
    pub enabled: bool,
}

/// iCalendar `STATUS` of an event (RFC 5545 §3.8.1.11).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventStatus {
    /// `CONFIRMED` — the default.
    #[default]
    Confirmed,
    /// `TENTATIVE`.
    Tentative,
    /// `CANCELLED`.
    Cancelled,
}

impl EventStatus {
    /// The RFC 5545 token (`CONFIRMED` / `TENTATIVE` / `CANCELLED`).
    #[must_use]
    pub const fn as_ical(self) -> &'static str {
        match self {
            Self::Confirmed => "CONFIRMED",
            Self::Tentative => "TENTATIVE",
            Self::Cancelled => "CANCELLED",
        }
    }

    /// Parse from the RFC 5545 token (case-insensitive). Unknown tokens map to
    /// [`EventStatus::Confirmed`].
    #[must_use]
    pub fn from_ical(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "TENTATIVE" => Self::Tentative,
            "CANCELLED" => Self::Cancelled,
            _ => Self::Confirmed,
        }
    }
}

/// iCalendar `TRANSP` — whether the event consumes busy time (RFC 5545 §3.8.2.7).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transparency {
    /// `OPAQUE` — busy — the default.
    #[default]
    Opaque,
    /// `TRANSPARENT` — free.
    Transparent,
}

impl Transparency {
    /// The RFC 5545 token (`OPAQUE` / `TRANSPARENT`).
    #[must_use]
    pub const fn as_ical(self) -> &'static str {
        match self {
            Self::Opaque => "OPAQUE",
            Self::Transparent => "TRANSPARENT",
        }
    }

    /// Parse from the RFC 5545 token (case-insensitive). Unknown tokens map to
    /// [`Transparency::Opaque`].
    #[must_use]
    pub fn from_ical(s: &str) -> Self {
        if s.trim().eq_ignore_ascii_case("TRANSPARENT") {
            Self::Transparent
        } else {
            Self::Opaque
        }
    }
}

/// The Casual Note pillar an event was projected from (doc §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A scheduled task (`kind='task'`).
    Task,
    /// A reminder (`kind='reminder'`).
    Reminder,
    /// A meeting session (`kind='session'`).
    Meeting,
}

impl SourceKind {
    /// The lowercase marker token used in the ICS `X-CASUAL-NOTE-SOURCE` property.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Reminder => "reminder",
            Self::Meeting => "meeting",
        }
    }

    /// Parse a marker token; returns `None` for anything unrecognized.
    #[must_use]
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s {
            "task" => Some(Self::Task),
            "reminder" => Some(Self::Reminder),
            "meeting" => Some(Self::Meeting),
            _ => None,
        }
    }
}

/// Back-link from a projected event to the Casual Note item it came from
/// (doc §4 `event.source_ref`, doc §5). Serialized into ICS as
/// `X-CASUAL-NOTE-SOURCE:<kind>:<uuid>` so the reverse detect helper can recover it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SourceRef {
    /// Which pillar produced the event.
    pub kind: SourceKind,
    /// The source entity's id.
    pub entity_id: Id,
}

impl SourceRef {
    /// The `X-CASUAL-NOTE-SOURCE` property value, e.g. `task:018f...`.
    #[must_use]
    pub fn marker_value(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.entity_id)
    }

    /// Parse a `<kind>:<uuid>` marker value. Returns `None` if malformed.
    #[must_use]
    pub fn parse_marker(value: &str) -> Option<Self> {
        let (kind, id) = value.split_once(':')?;
        Some(Self {
            kind: SourceKind::from_str_opt(kind.trim())?,
            entity_id: id.trim().parse().ok()?,
        })
    }
}

/// A `RECURRENCE-ID` override target (RFC 5545 §3.8.4.4): identifies which
/// instance of a recurring series a VEVENT overrides.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecurrenceId {
    /// The original instance start, as absolute UTC.
    pub instant: Timestamp,
    /// Whether the original instance was an all-day (DATE) value.
    pub all_day: bool,
    /// IANA tz of the original value; `None` for floating / all-day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// `RANGE=THISANDFUTURE` — the override applies to this and all later instances.
    #[serde(default)]
    pub this_and_future: bool,
}

/// The action of a `VALARM` (RFC 5545 §3.8.6.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlarmAction {
    /// `DISPLAY` — show text.
    Display,
    /// `AUDIO` — play a sound.
    Audio,
    /// `EMAIL` — send mail (transport is out of scope; retained for round-trip).
    Email,
    /// Any other / vendor action, retained verbatim for lossless round-trip.
    Other(String),
}

impl AlarmAction {
    /// The RFC 5545 token.
    #[must_use]
    pub fn as_ical(&self) -> String {
        match self {
            Self::Display => "DISPLAY".to_string(),
            Self::Audio => "AUDIO".to_string(),
            Self::Email => "EMAIL".to_string(),
            Self::Other(s) => s.clone(),
        }
    }

    /// Parse from the RFC 5545 token (case-insensitive for the known set).
    #[must_use]
    pub fn from_ical(s: &str) -> Self {
        match s.trim().to_ascii_uppercase().as_str() {
            "DISPLAY" => Self::Display,
            "AUDIO" => Self::Audio,
            "EMAIL" => Self::Email,
            other => Self::Other(other.to_string()),
        }
    }
}

/// A `VALARM` `TRIGGER` (RFC 5545 §3.8.6.3): either relative to the event
/// start/end, or an absolute UTC instant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlarmTrigger {
    /// Offset in seconds from the related edge; negative = before. `related_end`
    /// selects `RELATED=END` (default is the event start).
    Relative {
        /// Signed offset in seconds (negative fires before the edge).
        offset_secs: i64,
        /// Whether the offset is relative to the event end rather than its start.
        related_end: bool,
    },
    /// An absolute UTC trigger instant (`TRIGGER;VALUE=DATE-TIME:...Z`).
    Absolute(Timestamp),
}

/// A `VALARM` sub-component of an event (doc §4 / doc §5 reminder projection).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventAlarm {
    /// The alarm action.
    pub action: AlarmAction,
    /// When the alarm fires.
    pub trigger: AlarmTrigger,
    /// `DESCRIPTION` — required for `DISPLAY`/`EMAIL`; the alert text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `SUMMARY` — the subject line for `EMAIL` alarms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    /// `REPEAT` — number of additional times to re-fire.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<u32>,
    /// `DURATION` between repeats, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat_interval_secs: Option<i64>,
}

/// A calendar event (doc §4 `event`).
///
/// `start_utc`/`end_utc` are always absolute UTC. Interpretation of the original
/// RFC 5545 form is recovered from [`all_day`](Self::all_day) and
/// [`tz`](Self::tz): all-day = a DATE value at UTC-midnight; `tz == Some("UTC")` =
/// a `Z` value; `tz == Some(iana)` = a `TZID` value; `tz == None` (timed) = a
/// floating local value.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarEvent {
    /// Entity id (UUIDv7).
    pub id: Id,
    /// Owning [`Calendar::id`].
    pub calendar_id: Id,
    /// iCalendar `UID` — the stable cross-system identity (doc §3.2).
    pub uid: String,
    /// `SUMMARY`.
    pub title: String,
    /// Event start, absolute UTC.
    pub start_utc: Timestamp,
    /// Event end, absolute UTC (exclusive; for all-day this is the day after the
    /// last covered date, per RFC 5545).
    pub end_utc: Timestamp,
    /// Whether this is an all-day (DATE-valued) event.
    pub all_day: bool,
    /// IANA timezone for the DATE-TIME form; see the type-level note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tz: Option<String>,
    /// Raw `RRULE` property value (e.g. `FREQ=WEEKLY;BYDAY=MO`), without the
    /// `RRULE:` prefix; expanded via the `rrule` engine ([`Self::occurrences`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rrule: Option<String>,
    /// `EXDATE` exceptions (excluded instances), absolute UTC.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exdates: Vec<Timestamp>,
    /// `LOCATION`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// `DESCRIPTION`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `STATUS`.
    #[serde(default)]
    pub status: EventStatus,
    /// `TRANSP`.
    #[serde(default)]
    pub transparency: Transparency,
    /// `SEQUENCE` — the revision counter used for conflict resolution (doc §3.2).
    #[serde(default)]
    pub sequence: u32,
    /// `CREATED`, absolute UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created: Option<Timestamp>,
    /// `LAST-MODIFIED`, absolute UTC — the tiebreaker for last-writer-wins (doc §3.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<Timestamp>,
    /// CalDAV `ETag` for `If-Match` optimistic concurrency (doc §3.2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
    /// `RECURRENCE-ID` — set when this VEVENT overrides one instance of a series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recurrence_id: Option<RecurrenceId>,
    /// Back-link to the projected-from Casual Note item (doc §5), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<SourceRef>,
    /// `VALARM` sub-components.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alarms: Vec<EventAlarm>,
}

impl CalendarEvent {
    /// Construct a minimal timed event; optional fields default to empty/`None`.
    /// Callers typically build via the projection helpers or the ICS parser.
    #[must_use]
    pub fn new(
        calendar_id: Id,
        uid: impl Into<String>,
        title: impl Into<String>,
        start_utc: Timestamp,
        end_utc: Timestamp,
    ) -> Self {
        Self {
            id: Id::new(),
            calendar_id,
            uid: uid.into(),
            title: title.into(),
            start_utc,
            end_utc,
            all_day: false,
            tz: None,
            rrule: None,
            exdates: Vec::new(),
            location: None,
            description: None,
            status: EventStatus::default(),
            transparency: Transparency::default(),
            sequence: 0,
            created: None,
            last_modified: None,
            etag: None,
            recurrence_id: None,
            source_ref: None,
            alarms: Vec::new(),
        }
    }

    /// Expand the recurrence into concrete instance start-instants, using the
    /// shared `rrule` engine (doc §3.2). Non-recurring events yield the single
    /// start. `EXDATE` exclusions are honored. `limit` bounds the number of
    /// instances returned (guards against unbounded rules).
    pub fn occurrences(&self, limit: u16) -> CalendarResult<Vec<Timestamp>> {
        let Some(rule) = &self.rrule else {
            return Ok(vec![self.start_utc]);
        };
        // Build a minimal RRuleSet string DTSTART (UTC) + RRULE for the engine.
        let dtstart = crate::ical::format_utc_basic(self.start_utc)?;
        let spec = format!("DTSTART:{dtstart}\nRRULE:{rule}");
        let set: rrule::RRuleSet = spec
            .parse()
            .map_err(|e| CalendarError::Recurrence(format!("{e}")))?;
        let result = set.all(limit);
        let excluded: std::collections::HashSet<i64> =
            self.exdates.iter().map(Timestamp::as_millis).collect();
        Ok(result
            .dates
            .into_iter()
            .map(|d| Timestamp::from_millis(d.timestamp_millis()))
            .filter(|t| !excluded.contains(&t.as_millis()))
            .collect())
    }
}

/// The kind of account backing one or more calendars (doc §4 `calendar_account`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountKind {
    /// A native OS calendar account (Tier A).
    System,
    /// A CalDAV account on the user's own server (Tier B).
    CalDav,
}

/// Connection metadata for a calendar account.
///
/// **Invariant (doc §4 / §7): NO secrets live here.** Passwords, app-specific
/// passwords, and OAuth tokens are held by the OS keystore (Keychain / Credential
/// Manager / Secret Service) and passed to the sync layer as parameters — never
/// stored on this struct or persisted by this crate. `username` and `server_url`
/// are identity/endpoint metadata, not credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CalendarAccount {
    /// Entity id (UUIDv7).
    pub id: Id,
    /// Account tier.
    pub kind: AccountKind,
    /// User-facing label.
    pub display_name: String,
    /// CalDAV base / discovery URL (`None` for native accounts).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_url: Option<String>,
    /// Login/identity string (not a secret); the keystore key is derived from it
    /// by the app-service, never stored here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// Discovered CalDAV principal URL, cached after first connect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_url: Option<String>,
    /// Whether the account is currently connected/enabled.
    pub enabled: bool,
}

//! Entity/edge/state enumerations. Implements Data Model §3.1 (`entity.kind`
//! CHECK), §5.1 (`link.rel` CHECK), §6.3 (task buckets), §8.1 (`session.state`
//! CHECK), plus a [`Platform`] tag for honest capability reporting (HLD §9).

use serde::{Deserialize, Serialize};

use crate::id::Id;

/// The kind of a spine [`entity`](Data Model §3.1) row.
///
/// **Authoritative note (Data Model §3.1):** `block` and `link` are *not* entities.
/// A block is a projected sub-node addressed by `block_id` under a note; a link is
/// an edge row. The task brief listed them loosely — the Data Model CHECK wins, so
/// this enum contains exactly the twelve first-class kinds and nothing else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Note,
    Notebook,
    Tag,
    Task,
    Project,
    Area,
    Reminder,
    Session,
    Artifact,
    ActionItem,
    Person,
    RecurrenceRule,
}

impl EntityKind {
    /// The exact lowercase string stored in `entity.kind`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Notebook => "notebook",
            Self::Tag => "tag",
            Self::Task => "task",
            Self::Project => "project",
            Self::Area => "area",
            Self::Reminder => "reminder",
            Self::Session => "session",
            Self::Artifact => "artifact",
            Self::ActionItem => "action_item",
            Self::Person => "person",
            Self::RecurrenceRule => "recurrence_rule",
        }
    }

    /// Parse from the stored `entity.kind` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "note" => Self::Note,
            "notebook" => Self::Notebook,
            "tag" => Self::Tag,
            "task" => Self::Task,
            "project" => Self::Project,
            "area" => Self::Area,
            "reminder" => Self::Reminder,
            "session" => Self::Session,
            "artifact" => Self::Artifact,
            "action_item" => Self::ActionItem,
            "person" => Self::Person,
            "recurrence_rule" => Self::RecurrenceRule,
            _ => return None,
        })
    }
}

/// The relationship of a [`link`](Data Model §5.1) edge (`link.rel` CHECK).
///
/// `backlink` exists only for explicit user-authored reverse edges; ordinary
/// backlink rendering is a read over `wikilink`/`mention`, never a dual-write.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LinkRel {
    Wikilink,
    Backlink,
    Mention,
    Tagged,
    SpawnedFrom,
    About,
    Attends,
    ActionItemOf,
    Reminds,
    ChildOf,
}

impl LinkRel {
    /// The exact string stored in `link.rel`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::Wikilink => "wikilink",
            Self::Backlink => "backlink",
            Self::Mention => "mention",
            Self::Tagged => "tagged",
            Self::SpawnedFrom => "spawned_from",
            Self::About => "about",
            Self::Attends => "attends",
            Self::ActionItemOf => "action_item_of",
            Self::Reminds => "reminds",
            Self::ChildOf => "child_of",
        }
    }

    /// Parse from the stored `link.rel` string.
    #[must_use]
    pub fn from_db_str(s: &str) -> Option<Self> {
        Some(match s {
            "wikilink" => Self::Wikilink,
            "backlink" => Self::Backlink,
            "mention" => Self::Mention,
            "tagged" => Self::Tagged,
            "spawned_from" => Self::SpawnedFrom,
            "about" => Self::About,
            "attends" => Self::Attends,
            "action_item_of" => Self::ActionItemOf,
            "reminds" => Self::Reminds,
            "child_of" => Self::ChildOf,
            _ => return None,
        })
    }
}

/// The meeting [`session`](Data Model §8.1) state machine (`session.state` CHECK).
/// The LLM never owns recording state; a failed generate/transcribe stage falls
/// back without losing captured audio (HLD §8.4, Architecture §10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SessionState {
    New,
    Preflight,
    Ready,
    Recording,
    Paused,
    Stopping,
    Captured,
    FinalTranscribing,
    Generating,
    Indexing,
    Complete,
    Degraded,
    Failed,
    Recovering,
}

impl SessionState {
    /// The exact `SCREAMING_SNAKE_CASE` string stored in `session.state`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Preflight => "PREFLIGHT",
            Self::Ready => "READY",
            Self::Recording => "RECORDING",
            Self::Paused => "PAUSED",
            Self::Stopping => "STOPPING",
            Self::Captured => "CAPTURED",
            Self::FinalTranscribing => "FINAL_TRANSCRIBING",
            Self::Generating => "GENERATING",
            Self::Indexing => "INDEXING",
            Self::Complete => "COMPLETE",
            Self::Degraded => "DEGRADED",
            Self::Failed => "FAILED",
            Self::Recovering => "RECOVERING",
        }
    }
}

/// A derived task bucket (Data Model §6.3). **Never stored** — computed by query.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Bucket {
    Today,
    Upcoming,
    Anytime,
    Someday,
}

/// Host platform, for honest per-platform capability reporting (HLD §9).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Macos,
    Windows,
    Linux,
}

impl Platform {
    /// The platform this binary was compiled for; `None` on unsupported targets.
    #[must_use]
    pub const fn current() -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            Some(Self::Macos)
        }
        #[cfg(target_os = "windows")]
        {
            Some(Self::Windows)
        }
        #[cfg(target_os = "linux")]
        {
            Some(Self::Linux)
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            None
        }
    }
}

/// A typed reference to a spine entity: the `{kind, id}` pair carried on events
/// (`entity_ref` / `target_ref`, HLD §7) and in polymorphic pointers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: EntityKind,
    pub id: Id,
}

impl EntityRef {
    #[must_use]
    pub const fn new(kind: EntityKind, id: Id) -> Self {
        Self { kind, id }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entity_kind_db_strings_roundtrip() {
        for k in [
            EntityKind::Note,
            EntityKind::ActionItem,
            EntityKind::RecurrenceRule,
            EntityKind::Person,
        ] {
            assert_eq!(EntityKind::from_db_str(k.as_str()), Some(k));
        }
        assert_eq!(EntityKind::from_db_str("block"), None);
        assert_eq!(EntityKind::from_db_str("link"), None);
    }

    #[test]
    fn entity_kind_serde_matches_db_str() {
        let json = serde_json::to_string(&EntityKind::ActionItem).unwrap();
        assert_eq!(json, "\"action_item\"");
    }

    #[test]
    fn session_state_serde_is_screaming() {
        let json = serde_json::to_string(&SessionState::FinalTranscribing).unwrap();
        assert_eq!(json, "\"FINAL_TRANSCRIBING\"");
        assert_eq!(
            SessionState::FinalTranscribing.as_str(),
            "FINAL_TRANSCRIBING"
        );
    }

    #[test]
    fn link_rel_roundtrip() {
        assert_eq!(
            LinkRel::from_db_str("spawned_from"),
            Some(LinkRel::SpawnedFrom)
        );
        assert_eq!(LinkRel::SpawnedFrom.as_str(), "spawned_from");
    }
}

//! Identifier newtypes. Implements Data Model §1 (IDs) and §11.2 (op-log).
//!
//! - [`Id`]      — UUIDv7 for every `entity.id` (time-ordered, 16-byte BLOB at rest,
//!   hyphenated string on the wire).
//! - [`OpId`]    — ULID for `entity_op.op_id` (time-sortable).
//! - [`BlockId`] — short nanoid string living inside `doc_json`, mirrored to `block.block_id`.
//! - [`ModelId`] — opaque registry model identifier (e.g. `"qwen3-8b-q4_k_m"`).

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;
use ulid::Ulid;
use uuid::Uuid;

/// A UUIDv7 entity identifier.
///
/// Stored as a 16-byte `BLOB` in SQLite (see [`Id::as_bytes`]) and serialized as a
/// hyphenated string across the IPC boundary. UUIDv7 is time-ordered, so the natural
/// `Ord` is creation-order.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(Uuid);

impl Id {
    /// Generate a fresh time-ordered UUIDv7.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7())
    }

    /// Wrap an existing [`Uuid`] (used when reading rows).
    #[must_use]
    pub const fn from_uuid(u: Uuid) -> Self {
        Self(u)
    }

    /// The underlying [`Uuid`].
    #[must_use]
    pub const fn as_uuid(&self) -> Uuid {
        self.0
    }

    /// The 16-byte big-endian representation persisted as a SQLite `BLOB`.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8; 16] {
        self.0.as_bytes()
    }

    /// Reconstruct from the 16-byte `BLOB` form.
    #[must_use]
    pub fn from_bytes(b: [u8; 16]) -> Self {
        Self(Uuid::from_bytes(b))
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.as_hyphenated())
    }
}

impl fmt::Debug for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Id({})", self.0.as_hyphenated())
    }
}

impl FromStr for Id {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

/// A ULID op-log identifier (`entity_op.op_id`). Time-sortable, lexicographic.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OpId(Ulid);

impl OpId {
    /// Generate a fresh ULID for a new op-log entry.
    #[must_use]
    pub fn new() -> Self {
        Self(Ulid::new())
    }

    /// Wrap an existing [`Ulid`].
    #[must_use]
    pub const fn from_ulid(u: Ulid) -> Self {
        Self(u)
    }

    /// The underlying [`Ulid`].
    #[must_use]
    pub const fn as_ulid(&self) -> Ulid {
        self.0
    }
}

impl Default for OpId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Debug for OpId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "OpId({})", self.0)
    }
}

impl FromStr for OpId {
    type Err = ulid::DecodeError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
    }
}

/// A block identifier: a short nanoid string carried inside `doc_json` and mirrored
/// into `block.block_id`. Not a spine entity — it addresses a projected sub-node.
#[derive(Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BlockId(pub String);

impl BlockId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Debug for BlockId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlockId({})", self.0)
    }
}

/// An opaque model-registry identifier (`model-manager` / `models.*` commands).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ModelId(pub String);

impl ModelId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ModelId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// ---------------------------------------------------------------------------
// Semantic aliases. Every spine id is a UUIDv7 `Id`; these aliases document
// intent at command/event boundaries (HLD §6/§7) without new wrapper types.
// ---------------------------------------------------------------------------

/// `kind='note'` entity id.
pub type NoteId = Id;
/// `kind='notebook'` entity id.
pub type NotebookId = Id;
/// `kind='tag'` entity id.
pub type TagId = Id;
/// `kind='task'` entity id.
pub type TaskId = Id;
/// `kind='project'` entity id.
pub type ProjectId = Id;
/// `kind='area'` entity id.
pub type AreaId = Id;
/// `kind='reminder'` entity id.
pub type ReminderId = Id;
/// `kind='session'` entity id.
pub type SessionId = Id;
/// `kind='artifact'` entity id.
pub type ArtifactId = Id;
/// `kind='action_item'` entity id.
pub type ActionItemId = Id;
/// `kind='person'` entity id.
pub type PersonId = Id;
/// `kind='recurrence_rule'` entity id.
pub type RecurrenceRuleId = Id;
/// A `link` row id (UUIDv7).
pub type LinkId = Id;
/// A `transcript_segment.id` (UUIDv7), referenced by `evidence_segment_ids`.
pub type SegmentId = Id;
/// A `chunk.id` (UUIDv7).
pub type ChunkId = Id;
/// An ephemeral correlation id for a streamed search query (HLD §7 `SearchPartial`).
pub type QueryId = Id;
/// An ephemeral correlation id for a batch of AI suggestions (HLD §7 `SuggestionsReady`).
pub type BatchId = Id;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrips_string_and_bytes() {
        let id = Id::new();
        let s = id.to_string();
        assert_eq!(Id::from_str(&s).unwrap(), id);
        assert_eq!(Id::from_bytes(*id.as_bytes()), id);
    }

    #[test]
    fn id_is_uuidv7() {
        let id = Id::new();
        assert_eq!(id.as_uuid().get_version_num(), 7);
    }

    #[test]
    fn opid_roundtrips_string() {
        let op = OpId::new();
        assert_eq!(OpId::from_str(&op.to_string()).unwrap(), op);
    }

    #[test]
    fn id_serde_is_transparent_string() {
        let id = Id::new();
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{id}\""));
    }
}

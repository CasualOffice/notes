//! The polymorphic edge model (Data Model §5.1 `link`). One table holds every
//! edge kind — wikilinks, mentions, tags, meeting provenance, reminder targets,
//! parentage. These structs mirror the columns; [`LinkOrigin`] distinguishes
//! rebuilt-on-save `projected` edges from durable `user`/`meeting`/`ai_suggested`
//! edges (§5.1 "Projected vs authored").

use app_domain::{Id, LinkRel};

use crate::error::{LinkError, Result};

/// The provenance of a `link` row (`link.origin`, Data Model §5.1). `projected`
/// edges are deleted-and-reinserted on every note save; the others are never
/// touched by projection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LinkOrigin {
    /// Authored directly by the user.
    User,
    /// Parsed from `doc_json` on save (rebuildable).
    Projected,
    /// Proposed by AI, pending accept/reject (never silent).
    AiSuggested,
    /// Emitted by the meeting pipeline (provenance / action-item bridge).
    Meeting,
}

impl LinkOrigin {
    /// The exact string stored in `link.origin`.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Projected => "projected",
            Self::AiSuggested => "ai_suggested",
            Self::Meeting => "meeting",
        }
    }

    /// Parse from the stored `link.origin` string.
    ///
    /// # Errors
    /// Returns [`LinkError::UnknownOrigin`] for an unrecognized value.
    pub fn from_db_str(s: &str) -> Result<Self> {
        Ok(match s {
            "user" => Self::User,
            "projected" => Self::Projected,
            "ai_suggested" => Self::AiSuggested,
            "meeting" => Self::Meeting,
            other => return Err(LinkError::UnknownOrigin(other.to_string())),
        })
    }
}

/// An edge to insert / upsert. `id`, `created_at`, and `hlc` are assigned by the
/// writer, not the caller.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewLink {
    /// Origin entity (`link.src_entity`).
    pub src_entity: Id,
    /// Target entity (`link.dst_entity`).
    pub dst_entity: Id,
    /// Relationship (`link.rel`).
    pub rel: LinkRel,
    /// Precise origin block inside `src` (`link.src_block_id`).
    pub src_block_id: Option<String>,
    /// Precise target block inside `dst` (`link.dst_block_id`).
    pub dst_block_id: Option<String>,
    /// Meeting provenance segment ids (`link.evidence_segment_ids`, JSON array).
    pub evidence_segment_ids: Option<Vec<Id>>,
    /// Edge payload (supertag field values, mention offsets…) (`link.data_json`).
    pub data_json: Option<String>,
    /// Provenance (`link.origin`).
    pub origin: LinkOrigin,
}

impl NewLink {
    /// Build a minimal edge with default (`user`) origin and no anchors.
    #[must_use]
    pub fn new(src_entity: Id, dst_entity: Id, rel: LinkRel) -> Self {
        Self {
            src_entity,
            dst_entity,
            rel,
            src_block_id: None,
            dst_block_id: None,
            evidence_segment_ids: None,
            data_json: None,
            origin: LinkOrigin::User,
        }
    }

    /// Builder: set the origin.
    #[must_use]
    pub fn with_origin(mut self, origin: LinkOrigin) -> Self {
        self.origin = origin;
        self
    }

    /// Builder: set the source block anchor.
    #[must_use]
    pub fn with_src_block(mut self, block_id: impl Into<String>) -> Self {
        self.src_block_id = Some(block_id.into());
        self
    }

    /// Serialize `evidence_segment_ids` to the stored JSON-array string form.
    #[must_use]
    pub fn evidence_json(&self) -> Option<String> {
        self.evidence_segment_ids.as_ref().map(|ids| {
            let strs: Vec<String> = ids.iter().map(ToString::to_string).collect();
            serde_json::to_string(&strs).unwrap_or_else(|_| "[]".to_string())
        })
    }
}

/// A fully-materialized `link` row as read back from the DB.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkEdge {
    /// Row id (`link.id`, UUIDv7).
    pub id: Id,
    /// `link.src_entity`.
    pub src_entity: Id,
    /// `link.dst_entity`.
    pub dst_entity: Id,
    /// `link.rel`.
    pub rel: LinkRel,
    /// `link.src_block_id`.
    pub src_block_id: Option<String>,
    /// `link.dst_block_id`.
    pub dst_block_id: Option<String>,
    /// `link.evidence_segment_ids` (raw JSON-array string).
    pub evidence_segment_ids: Option<String>,
    /// `link.data_json`.
    pub data_json: Option<String>,
    /// `link.origin`.
    pub origin: LinkOrigin,
    /// `link.created_at` (epoch-ms UTC).
    pub created_at: i64,
    /// `link.hlc`.
    pub hlc: String,
    /// `link.deleted_at` (tombstone; `None` = live).
    pub deleted_at: Option<i64>,
}

// ---------------------------------------------------------------------------
// BLOB <-> Id helpers (entity ids persist as 16-byte BLOBs, Data Model §1).
// ---------------------------------------------------------------------------

/// Decode a 16-byte id BLOB read from a row.
///
/// # Errors
/// Returns [`LinkError::BadIdBlob`] if the slice is not 16 bytes.
pub fn id_from_blob(bytes: &[u8]) -> Result<Id> {
    let arr: [u8; 16] = bytes
        .try_into()
        .map_err(|_| LinkError::BadIdBlob(bytes.len()))?;
    Ok(Id::from_bytes(arr))
}

/// Parse a stored `link.rel` string into a [`LinkRel`].
///
/// # Errors
/// Returns [`LinkError::UnknownRel`] for a value outside the CHECK set.
pub fn rel_from_str(s: &str) -> Result<LinkRel> {
    LinkRel::from_db_str(s).ok_or_else(|| LinkError::UnknownRel(s.to_string()))
}

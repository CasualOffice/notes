//! Cited, reversible AI suggestions (**Data Model §9.6** `suggestion`; **Foundation
//! §4** "never silent edits").
//!
//! Auto-link / auto-tag are proposed as [`Suggestion`] rows a user later approves or
//! dismisses (`ai.suggestions.list/.apply/.dismiss`, HLD §6) — never applied here.
//! This module is **data only**: it produces the proposed mutation and the evidence
//! backing it; storage owns persistence, and application/reversal is a separate,
//! op-log-appending step. Every suggestion carries **non-empty** citations
//! (constructors return `None` otherwise) — a suggestion with no evidence is never
//! produced, mirroring the answer-grounding gate.

use serde::{Deserialize, Serialize};

use app_domain::{EntityKind, EntityRef, Id, LinkRel, Timestamp};
use llm_api::{Citation, SourceKind};

use crate::verify::VerifiedAnswer;

/// The kind of proposed edit (`suggestion.kind` CHECK, Data Model §9.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionKind {
    /// Propose a `link` edge (e.g. a wikilink) to a related entity.
    AutoLink,
    /// Propose tagging the target with an existing tag entity.
    AutoTag,
    /// Propose promoting an extracted action item to a task.
    ActionItem,
    /// Propose merging two `person` entities.
    MergePerson,
}

/// Review state of a suggestion (`suggestion.state`, Data Model §9.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionState {
    Pending,
    Accepted,
    Rejected,
    Expired,
}

/// A reversible, evidence-backed proposed edit (Data Model §9.6). In-memory form;
/// [`Suggestion::proposed_json_str`] / [`Suggestion::citations_json_str`] render the
/// `proposed_json` / `citations_json` columns for storage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Suggestion {
    pub id: Id,
    pub kind: SuggestionKind,
    /// `suggestion.target_id` — the entity the edit applies to.
    pub target: EntityRef,
    /// The concrete proposed mutation (`suggestion.proposed_json`).
    pub proposed: serde_json::Value,
    /// Evidence backing the suggestion (`suggestion.citations_json`); non-empty.
    pub citations: Vec<Citation>,
    pub state: SuggestionState,
    pub created_at: Timestamp,
    pub resolved_at: Option<Timestamp>,
}

impl Suggestion {
    /// Propose a `wikilink` from `target` to `dst`, backed by `citations`. Returns
    /// `None` if `citations` is empty (no evidence ⇒ no suggestion) or `dst == target`
    /// (a self-link is never proposed).
    #[must_use]
    pub fn auto_link(target: EntityRef, dst: EntityRef, citations: Vec<Citation>) -> Option<Self> {
        if citations.is_empty() || dst == target {
            return None;
        }
        let proposed = serde_json::json!({
            "rel": LinkRel::Wikilink.as_str(),
            "src_kind": target.kind.as_str(),
            "src_id": target.id.to_string(),
            "dst_kind": dst.kind.as_str(),
            "dst_id": dst.id.to_string(),
        });
        Some(Self::new(
            SuggestionKind::AutoLink,
            target,
            proposed,
            citations,
        ))
    }

    /// Propose tagging `target` with the tag entity `tag`, backed by `citations`.
    /// Returns `None` if `citations` is empty or `tag` is not a `tag` entity.
    #[must_use]
    pub fn auto_tag(target: EntityRef, tag: EntityRef, citations: Vec<Citation>) -> Option<Self> {
        if citations.is_empty() || tag.kind != EntityKind::Tag {
            return None;
        }
        let proposed = serde_json::json!({
            "rel": LinkRel::Tagged.as_str(),
            "target_kind": target.kind.as_str(),
            "target_id": target.id.to_string(),
            "tag_id": tag.id.to_string(),
        });
        Some(Self::new(
            SuggestionKind::AutoTag,
            target,
            proposed,
            citations,
        ))
    }

    fn new(
        kind: SuggestionKind,
        target: EntityRef,
        proposed: serde_json::Value,
        citations: Vec<Citation>,
    ) -> Self {
        Self {
            id: Id::new(),
            kind,
            target,
            proposed,
            citations,
            state: SuggestionState::Pending,
            created_at: Timestamp::now(),
            resolved_at: None,
        }
    }

    /// The `suggestion.proposed_json` column value.
    ///
    /// # Errors
    /// [`serde_json::Error`] if the proposed value cannot serialize (it always can).
    pub fn proposed_json_str(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.proposed)
    }

    /// The `suggestion.citations_json` column value.
    ///
    /// # Errors
    /// [`serde_json::Error`] if the citations cannot serialize.
    pub fn citations_json_str(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(&self.citations)
    }
}

/// Map a citation's [`SourceKind`] to the spine [`EntityKind`] it points at, so a
/// citation can be turned into an auto-link target.
#[must_use]
pub fn cited_entity(citation: &Citation) -> EntityRef {
    let kind = match citation.source_kind {
        SourceKind::NoteBlock => EntityKind::Note,
        SourceKind::TranscriptWindow => EntityKind::Session,
        SourceKind::Task => EntityKind::Task,
        SourceKind::Reminder => EntityKind::Reminder,
    };
    EntityRef::new(kind, citation.source_id)
}

/// Generate cited auto-link suggestions from a verified answer: for each distinct
/// entity the answer cites (other than `target`), propose a wikilink from `target`
/// to it, carrying the citations that support the link.
///
/// Reversible and data-only: nothing is applied. Entities are emitted in first-
/// citation order for determinism.
#[must_use]
pub fn auto_links_from_answer(target: EntityRef, verified: &VerifiedAnswer) -> Vec<Suggestion> {
    // Group verified citations by the entity they point at, preserving first-seen
    // order (deterministic output — the op-log oracle expects stability).
    let mut order: Vec<EntityRef> = Vec::new();
    let mut grouped: Vec<(EntityRef, Vec<Citation>)> = Vec::new();
    for c in &verified.answer.citations {
        let ent = cited_entity(c);
        if ent == target {
            continue;
        }
        if let Some(slot) = grouped.iter_mut().find(|(e, _)| *e == ent) {
            slot.1.push(c.clone());
        } else {
            order.push(ent);
            grouped.push((ent, vec![c.clone()]));
        }
    }

    grouped
        .into_iter()
        .filter_map(|(ent, cites)| Suggestion::auto_link(target, ent, cites))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::Chunk;
    use crate::retrieve::fuse_channels;
    use crate::verify::verify;
    use app_domain::ChunkId;
    use llm_api::AnswerV1;
    use search::RrfConfig;

    fn chunk_id(i: u8) -> ChunkId {
        let mut b = [0u8; 16];
        b[15] = i;
        Id::from_bytes(b)
    }

    #[test]
    fn auto_link_requires_evidence() {
        let a = EntityRef::new(EntityKind::Note, Id::new());
        let b = EntityRef::new(EntityKind::Note, Id::new());
        assert!(Suggestion::auto_link(a, b, vec![]).is_none());
    }

    #[test]
    fn auto_link_rejects_self_link() {
        let a = EntityRef::new(EntityKind::Note, Id::new());
        let cite = Chunk::note_block(chunk_id(1), a.id, "x").to_citation();
        assert!(Suggestion::auto_link(a, a, vec![cite]).is_none());
    }

    #[test]
    fn auto_tag_requires_tag_entity() {
        let target = EntityRef::new(EntityKind::Note, Id::new());
        let not_tag = EntityRef::new(EntityKind::Note, Id::new());
        let cite = Chunk::note_block(chunk_id(1), target.id, "x").to_citation();
        assert!(Suggestion::auto_tag(target, not_tag, vec![cite.clone()]).is_none());
        let tag = EntityRef::new(EntityKind::Tag, Id::new());
        assert!(Suggestion::auto_tag(target, tag, vec![cite]).is_some());
    }

    #[test]
    fn suggestions_from_answer_link_to_cited_entities() {
        // A note being composed (target) with an answer citing a DIFFERENT note.
        let target = EntityRef::new(EntityKind::Note, Id::new());
        let other_note = Id::new();
        let chunk = Chunk::note_block(chunk_id(1), other_note, "pricing decision recorded here");
        let retrieval = fuse_channels(std::slice::from_ref(&chunk), &[], RrfConfig::default(), 8);
        let answer = AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "pricing stays at $12".into(),
            citations: vec![chunk.to_citation()],
            confidence: 0.9,
            unanswered: false,
        };
        let verified = verify(&answer, &retrieval, 0.0).unwrap();
        let suggestions = auto_links_from_answer(target, &verified);
        assert_eq!(suggestions.len(), 1);
        assert_eq!(suggestions[0].kind, SuggestionKind::AutoLink);
        assert_eq!(suggestions[0].state, SuggestionState::Pending);
        assert!(!suggestions[0].citations.is_empty());
        // The proposed edge points at the cited note.
        assert_eq!(suggestions[0].proposed["dst_id"], other_note.to_string());
    }

    #[test]
    fn suggestions_skip_self_citations() {
        // The answer cites the SAME note being composed — no self-link suggestion.
        let note = Id::new();
        let target = EntityRef::new(EntityKind::Note, note);
        let chunk = Chunk::note_block(chunk_id(1), note, "body");
        let retrieval = fuse_channels(std::slice::from_ref(&chunk), &[], RrfConfig::default(), 8);
        let answer = AnswerV1 {
            schema: AnswerV1::SCHEMA.to_string(),
            answer: "x".into(),
            citations: vec![chunk.to_citation()],
            confidence: 0.9,
            unanswered: false,
        };
        let verified = verify(&answer, &retrieval, 0.0).unwrap();
        assert!(auto_links_from_answer(target, &verified).is_empty());
    }

    #[test]
    fn columns_serialize() {
        let target = EntityRef::new(EntityKind::Note, Id::new());
        let dst = EntityRef::new(EntityKind::Note, Id::new());
        let cite = Chunk::note_block(chunk_id(1), dst.id, "x").to_citation();
        let s = Suggestion::auto_link(target, dst, vec![cite]).unwrap();
        assert!(s.proposed_json_str().unwrap().contains("wikilink"));
        assert!(s.citations_json_str().unwrap().contains("chunk_id"));
    }
}

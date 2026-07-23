//! # notes
//!
//! The knowledge pillar. Implements **Data Model §4** (`note`/`block`/`notebook`/
//! `tag`/`attachment`), the editor↔Rust projection of **Architecture §3.1**, and the
//! note-save + backlink-resolution sequence of **HLD §8.1**.
//!
//! `doc_json` (Tiptap/ProseMirror) is the source of truth; `block`, projected
//! `link` rows, FTS, and chunks are derived on save and rebuilt only when
//! `content_hash` changes. Never dual-write backlinks — they are a read over `link`.
//!
//! ## Modules & pipeline
//! The save path is a pure pipeline over the parsed [`model::Node`] tree (`storage`
//! supplies the connection and persistence; this crate supplies the logic):
//! 1. [`validate`] — schema-validate `doc_json` before persist (Data Model §4.1).
//! 2. [`projection::ensure_block_ids`] — stamp stable `blockId`s on new blocks.
//! 3. [`projection::project_blocks`] — flatten to ordered `block` rows.
//! 4. [`extract::extract_links`] — yield `[[wiki]]`/`#tag`/`@mention` edges with
//!    their source `block_id` (resolved to entities by the `links` crate).
//! 5. [`markdown`] — CommonMark+GFM import/export round-trip (Data Model §15.1).

#![forbid(unsafe_code)]

pub mod error;
pub mod extract;
pub mod lexo;
pub mod markdown;
pub mod model;
pub mod projection;
pub mod validate;

pub use error::{NotesError, Result};
pub use extract::{extract_links, ExtractedLink};
pub use markdown::{from_markdown, to_markdown, to_markdown_with, MarkdownOptions};
pub use model::{Mark, Node};
pub use projection::{ensure_block_ids, project_blocks, ProjectedBlock};
pub use validate::validate;

/// Convenience: validate a document then project both its block rows and link
/// references in one call. `gen` mints `blockId`s for new blocks (mutating `doc`
/// so the id-stamped tree can be persisted as the new `doc_json`).
///
/// # Errors
/// Returns [`NotesError::Invalid`] if `doc` fails schema validation.
pub fn validate_and_project(
    note_id: app_domain::NoteId,
    doc: &mut Node,
    gen: &mut dyn FnMut() -> app_domain::BlockId,
) -> Result<(Vec<ProjectedBlock>, Vec<ExtractedLink>)> {
    validate(doc)?;
    ensure_block_ids(doc, gen);
    let blocks = project_blocks(note_id, doc);
    let links = extract_links(doc);
    Ok((blocks, links))
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_domain::{BlockId, LinkRel, NoteId};

    #[test]
    fn full_pipeline_from_markdown_to_projection() {
        let mut doc = from_markdown("# Notes\n\nSee [[Foo]] about #Work with @sam");
        let note = NoteId::new();
        let mut n = 0;
        let mut gen = || {
            n += 1;
            BlockId::new(format!("blk{n}"))
        };
        let (blocks, links) = validate_and_project(note, &mut doc, &mut gen).unwrap();

        // Heading + paragraph projected in order, both stamped with ids.
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].node_type, "heading");
        assert_eq!(blocks[1].node_type, "paragraph");
        assert!(blocks.iter().all(|b| !b.block_id.as_str().is_empty()));

        // Every reference carries the paragraph's block id as source.
        let para_block = blocks[1].block_id.clone();
        assert!(links.iter().any(|l| l.rel == LinkRel::Wikilink
            && l.target_title.as_deref() == Some("Foo")
            && l.src_block_id.as_ref() == Some(&para_block)));
        assert!(links
            .iter()
            .any(|l| l.rel == LinkRel::Tagged && l.target_title.as_deref() == Some("Work")));
        assert!(links
            .iter()
            .any(|l| l.rel == LinkRel::Mention && l.target_title.as_deref() == Some("sam")));
    }
}

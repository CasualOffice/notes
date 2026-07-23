//! Block-index projection: flatten `doc_json` → `block` rows (Data Model §4.2,
//! Architecture §3.1). This is a *derived* projection — the block table is rebuilt
//! bit-identically from `doc_json` on every save where `content_hash` changed, so
//! this function is pure (no IO). `storage` performs the delete-and-reinsert.
//!
//! Two steps, kept separate so the doc can be persisted with its stamped ids:
//! 1. [`ensure_block_ids`] mutates the doc, minting a stable `blockId` for any leaf
//!    block that lacks one (Architecture §3.1 "block IDs are Rust-assigned when
//!    missing"). Existing ids are never reused or changed.
//! 2. [`project_blocks`] reads the (id-stamped) doc into ordered [`ProjectedBlock`]
//!    rows with `seq`, `depth`, `text_content`, `attrs_json`, and `order_key`.

use app_domain::{BlockId, NoteId};

use crate::lexo;
use crate::model::{is_container, is_leaf_block, Node};

/// One projected `block` table row (Data Model §4.2). Field names mirror the columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectedBlock {
    /// Stable nanoid from the node's `attrs.blockId` (`block.block_id`).
    pub block_id: BlockId,
    /// Owning note (`block.note_id`).
    pub note_id: NoteId,
    /// ProseMirror node type (`block.node_type`).
    pub node_type: String,
    /// Document order within the note (`block.seq`).
    pub seq: i64,
    /// Nesting depth; incremented per `listItem` ancestor (`block.depth`).
    pub depth: i64,
    /// Flattened plain text for FTS / backlink targeting (`block.text_content`).
    pub text_content: String,
    /// Node-specific attrs as a JSON object string, or `None` (`block.attrs_json`).
    pub attrs_json: Option<String>,
    /// Fractional index for reorder (`block.order_key`).
    pub order_key: String,
}

/// Walk `doc` and stamp a fresh `blockId` on every leaf block that lacks one.
///
/// `gen` is injected so callers control id generation (real callers pass a nanoid
/// source; tests pass a deterministic counter). Existing ids are preserved so
/// backlinks, block-anchored reminders, and evidence anchors survive re-edits.
pub fn ensure_block_ids(doc: &mut Node, gen: &mut dyn FnMut() -> BlockId) {
    for child in &mut doc.content {
        if is_leaf_block(&child.node_type) {
            if child.block_id().is_none() {
                let id = gen();
                child.set_block_id(&id);
            }
        } else {
            ensure_block_ids(child, gen);
        }
    }
}

/// Flatten an id-stamped document into ordered `block` rows.
///
/// Call [`ensure_block_ids`] first; any leaf block still missing a `blockId` is
/// given a deterministic `seq`-derived fallback so the projection never drops a row.
#[must_use]
pub fn project_blocks(note_id: NoteId, doc: &Node) -> Vec<ProjectedBlock> {
    let mut raw: Vec<RawBlock> = Vec::new();
    collect(doc, 0, &mut raw);

    let keys = lexo::order_keys(raw.len());
    raw.into_iter()
        .zip(keys)
        .enumerate()
        .map(|(i, (rb, order_key))| ProjectedBlock {
            block_id: rb
                .block_id
                .unwrap_or_else(|| BlockId::new(format!("auto-{i}"))),
            note_id,
            node_type: rb.node_type,
            seq: i as i64,
            depth: rb.depth,
            text_content: rb.text_content,
            attrs_json: rb.attrs_json,
            order_key,
        })
        .collect()
}

struct RawBlock {
    block_id: Option<BlockId>,
    node_type: String,
    depth: i64,
    text_content: String,
    attrs_json: Option<String>,
}

/// Depth-first walk in document order. Descends through containers; each leaf block
/// emits one row (its inner text flattened, no further block recursion).
fn collect(node: &Node, depth: i64, out: &mut Vec<RawBlock>) {
    for child in &node.content {
        if is_leaf_block(&child.node_type) {
            out.push(RawBlock {
                block_id: child.block_id(),
                node_type: child.node_type.clone(),
                depth,
                text_content: child.flatten_text(),
                attrs_json: child.attrs_json(),
            });
        } else if is_container(&child.node_type) {
            // Only list items add nesting depth; other containers are structural.
            let child_depth = if child.node_type == "listItem" {
                depth + 1
            } else {
                depth
            };
            collect(child, child_depth, out);
        }
        // Unknown/non-block, non-container nodes at block level are skipped for the
        // index but remain in doc_json (source of truth).
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn counter() -> impl FnMut() -> BlockId {
        let mut n = 0;
        move || {
            n += 1;
            BlockId::new(format!("b{n}"))
        }
    }

    fn doc() -> Node {
        serde_json::from_value(json!({
            "type": "doc",
            "content": [
                { "type": "heading", "attrs": { "level": 1, "blockId": "h1" },
                  "content": [ { "type": "text", "text": "Title" } ] },
                { "type": "paragraph",
                  "content": [ { "type": "text", "text": "Hello world" } ] },
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [
                        { "type": "paragraph",
                          "content": [ { "type": "text", "text": "one" } ] }
                    ] },
                    { "type": "listItem", "content": [
                        { "type": "paragraph",
                          "content": [ { "type": "text", "text": "two" } ] }
                    ] }
                ] },
                { "type": "todo", "attrs": { "checked": true },
                  "content": [ { "type": "text", "text": "done item" } ] }
            ]
        }))
        .unwrap()
    }

    #[test]
    fn ensure_block_ids_mints_only_missing() {
        let mut d = doc();
        let mut g = counter();
        ensure_block_ids(&mut d, &mut g);
        // Heading kept its explicit id.
        assert_eq!(d.content[0].block_id().unwrap().as_str(), "h1");
        // Paragraph, both list-item paragraphs, and the todo were minted.
        assert!(d.content[1].block_id().is_some());
        assert!(d.content[3].block_id().is_some());
    }

    #[test]
    fn projection_flattens_in_order_with_depth() {
        let mut d = doc();
        let mut g = counter();
        ensure_block_ids(&mut d, &mut g);
        let note = NoteId::new();
        let blocks = project_blocks(note, &d);

        let types: Vec<&str> = blocks.iter().map(|b| b.node_type.as_str()).collect();
        assert_eq!(
            types,
            ["heading", "paragraph", "paragraph", "paragraph", "todo"]
        );

        // seq is dense and ordered.
        assert_eq!(
            blocks.iter().map(|b| b.seq).collect::<Vec<_>>(),
            [0, 1, 2, 3, 4]
        );
        // List-item paragraphs are one level deep.
        assert_eq!(blocks[2].depth, 1);
        assert_eq!(blocks[3].depth, 1);
        assert_eq!(blocks[0].depth, 0);
        // order_key sorts in seq order.
        for w in blocks.windows(2) {
            assert!(w[0].order_key < w[1].order_key);
        }
        // Flattened text + attrs preserved.
        assert_eq!(blocks[0].text_content, "Title");
        assert!(blocks[4].attrs_json.as_deref().unwrap().contains("checked"));
    }

    #[test]
    fn projection_is_deterministic() {
        let mut d = doc();
        let mut g = counter();
        ensure_block_ids(&mut d, &mut g);
        let note = NoteId::new();
        assert_eq!(project_blocks(note, &d), project_blocks(note, &d));
    }
}

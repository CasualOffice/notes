//! Tiptap / ProseMirror `doc_json` serde model. Implements the editor document
//! shape of **Data Model §4.1** (`note.doc_json` = source of truth) and the block
//! node vocabulary of **Feature Specs §1.1** (paragraph, heading, todo, callout,
//! code, table, …) plus the inline `[[wikilink]]` / `#tag` / `@mention` marks of
//! **§1.2–§1.4**.
//!
//! ProseMirror JSON is uniform: every node is `{ "type", "attrs"?, "content"?,
//! "marks"?, "text"? }`. Rather than a lossy per-type enum we keep one flexible
//! [`Node`] struct so unknown/future node types round-trip byte-for-byte; typed
//! meaning is layered on top by [`crate::validate`], [`crate::projection`], and
//! [`crate::extract`]. `block_id` is carried in `attrs.blockId` (Architecture §3.1).

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use app_domain::BlockId;

/// The `attrs` key holding a block node's stable id inside `doc_json`.
pub const BLOCK_ID_ATTR: &str = "blockId";

/// A single ProseMirror node. The root document is a [`Node`] with
/// `node_type == "doc"`; see [`Node::is_doc_root`].
///
/// `PartialEq` (not `Eq`) because `serde_json::Value` attrs may hold floats.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Node {
    /// The ProseMirror node type (`"paragraph"`, `"heading"`, `"doc"`, `"text"`…).
    #[serde(rename = "type")]
    pub node_type: String,
    /// Node-specific attributes (heading `level`, todo `checked`, code `language`,
    /// `blockId`, …). Empty maps are omitted on serialize to stay diff-clean.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
    /// Inline marks applied to a text node (bold/italic/code plus the semantic
    /// `wikilink`/`tag`/`mention` marks).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub marks: Vec<Mark>,
    /// Child nodes (block children, list items, table rows, inline runs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub content: Vec<Node>,
    /// The literal text of a `type == "text"` leaf node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// An inline mark: `{ "type", "attrs"? }`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Mark {
    /// The mark type (`"bold"`, `"wikilink"`, `"tag"`, `"mention"`, …).
    #[serde(rename = "type")]
    pub mark_type: String,
    /// Mark attributes (wikilink `target`/`alias`, tag `name`, mention `label`…).
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub attrs: Map<String, Value>,
}

// ---------------------------------------------------------------------------
// Node-type classification (Data Model §4.2 `block.node_type`, Feature Specs §1.1)
// ---------------------------------------------------------------------------

/// Structural container nodes that never become a `block` row on their own; the
/// projection descends through them. `listItem` is the only one that adds depth.
pub const CONTAINER_TYPES: &[&str] = &["doc", "bulletList", "orderedList", "listItem"];

/// Leaf block node types that each project to exactly one `block` row. Their inner
/// text (including table cells / list-item prose) is flattened into `text_content`.
pub const LEAF_BLOCK_TYPES: &[&str] = &[
    "paragraph",
    "heading",
    "todo",
    "taskItem",
    "callout",
    "blockquote",
    "code",
    "codeBlock",
    "table",
    "horizontalRule",
    "divider",
    "embed",
    "image",
    "transcript_segment",
];

/// The semantic inline marks that carry link edges (Feature Specs §1.2–§1.4).
pub const SEMANTIC_MARKS: &[&str] = &["wikilink", "tag", "mention"];

/// Standard formatting marks accepted by [`crate::validate`].
pub const FORMATTING_MARKS: &[&str] = &["bold", "italic", "code", "strike", "underline", "link"];

/// True for a structural container node (`doc`, lists, list items).
#[must_use]
pub fn is_container(node_type: &str) -> bool {
    CONTAINER_TYPES.contains(&node_type)
}

/// True for a node type that projects to a single `block` row.
#[must_use]
pub fn is_leaf_block(node_type: &str) -> bool {
    LEAF_BLOCK_TYPES.contains(&node_type)
}

impl Node {
    /// Construct an empty node of the given type.
    #[must_use]
    pub fn new(node_type: impl Into<String>) -> Self {
        Self {
            node_type: node_type.into(),
            ..Self::default()
        }
    }

    /// Construct a `text` leaf node.
    #[must_use]
    pub fn text_node(text: impl Into<String>) -> Self {
        Self {
            node_type: "text".to_string(),
            text: Some(text.into()),
            ..Self::default()
        }
    }

    /// Parse a document root from a `doc_json` string.
    ///
    /// # Errors
    /// Returns [`crate::NotesError::Serde`] if the JSON is malformed.
    pub fn from_json(s: &str) -> Result<Self, crate::NotesError> {
        Ok(serde_json::from_str(s)?)
    }

    /// Serialize back to canonical `doc_json`.
    ///
    /// # Errors
    /// Returns [`crate::NotesError::Serde`] on an unserializable value.
    pub fn to_json(&self) -> Result<String, crate::NotesError> {
        Ok(serde_json::to_string(self)?)
    }

    /// True when this is the `type == "doc"` root.
    #[must_use]
    pub fn is_doc_root(&self) -> bool {
        self.node_type == "doc"
    }

    /// True for a `type == "text"` leaf.
    #[must_use]
    pub fn is_text(&self) -> bool {
        self.node_type == "text"
    }

    /// Read a string attribute by key.
    #[must_use]
    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).and_then(Value::as_str)
    }

    /// Read a boolean attribute by key.
    #[must_use]
    pub fn attr_bool(&self, key: &str) -> Option<bool> {
        self.attrs.get(key).and_then(Value::as_bool)
    }

    /// Read an integer attribute by key.
    #[must_use]
    pub fn attr_i64(&self, key: &str) -> Option<i64> {
        self.attrs.get(key).and_then(Value::as_i64)
    }

    /// The stable `blockId` carried in `attrs`, if present.
    #[must_use]
    pub fn block_id(&self) -> Option<BlockId> {
        self.attr_str(BLOCK_ID_ATTR).map(BlockId::new)
    }

    /// Stamp `attrs.blockId`.
    pub fn set_block_id(&mut self, id: &BlockId) {
        self.attrs.insert(
            BLOCK_ID_ATTR.to_string(),
            Value::String(id.as_str().to_string()),
        );
    }

    /// `attrs` serialized to a JSON object string, or `None` when empty. This is the
    /// `block.attrs_json` column value (Data Model §4.2).
    #[must_use]
    pub fn attrs_json(&self) -> Option<String> {
        if self.attrs.is_empty() {
            None
        } else {
            serde_json::to_string(&self.attrs).ok()
        }
    }

    /// Flatten all descendant text into a single plain string — the
    /// `block.text_content` value used for FTS and backlink targeting
    /// (Data Model §4.2). Block-ish descendants are space-separated.
    #[must_use]
    pub fn flatten_text(&self) -> String {
        let mut buf = String::new();
        flatten_into(self, &mut buf);
        buf.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}

fn flatten_into(node: &Node, buf: &mut String) {
    if let Some(t) = &node.text {
        buf.push_str(t);
        return;
    }
    for child in &node.content {
        flatten_into(child, buf);
        // Separate block-ish descendants so words don't run together.
        if child.text.is_none() && !buf.ends_with(' ') {
            buf.push(' ');
        }
    }
}

impl Mark {
    /// Construct a mark of the given type with no attributes.
    #[must_use]
    pub fn new(mark_type: impl Into<String>) -> Self {
        Self {
            mark_type: mark_type.into(),
            ..Self::default()
        }
    }

    /// Read a string attribute by key.
    #[must_use]
    pub fn attr_str(&self, key: &str) -> Option<&str> {
        self.attrs.get(key).and_then(Value::as_str)
    }

    /// True for one of the semantic link marks (`wikilink`/`tag`/`mention`).
    #[must_use]
    pub fn is_semantic(&self) -> bool {
        SEMANTIC_MARKS.contains(&self.mark_type.as_str())
    }
}

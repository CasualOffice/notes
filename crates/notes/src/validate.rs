//! `doc_json` schema validation, run before any persist (Data Model §4.1:
//! "`doc_json` is schema-validated against the active Tiptap schema before
//! persist"; Architecture §3.1: "validate against PM schema, reject if invalid").
//!
//! This is a structural validator, deliberately lenient about *unknown* node/mark
//! types (the editor schema evolves via `doc_schema_version`) but strict about the
//! invariants the Rust projection relies on: a `doc` root, well-formed text leaves,
//! sane heading levels, and no obviously malformed semantic marks.

use crate::error::NotesError;
use crate::model::{Mark, Node, SEMANTIC_MARKS};

/// Maximum heading level accepted (`heading.attrs.level`). The editor exposes 1–3
/// (Feature Specs §1.1) but we accept the full ProseMirror range for import safety.
const MAX_HEADING_LEVEL: i64 = 6;

/// Validate a `doc_json` document tree.
///
/// # Errors
/// Returns the first [`NotesError::Invalid`] encountered, with a JSON-path-ish
/// location. Passing validation is a precondition of persist / projection.
pub fn validate(doc: &Node) -> Result<(), NotesError> {
    if !doc.is_doc_root() {
        return Err(NotesError::invalid(
            "$",
            format!(
                "document root must be type \"doc\", found \"{}\"",
                doc.node_type
            ),
        ));
    }
    validate_node(doc, "$")
}

fn validate_node(node: &Node, path: &str) -> Result<(), NotesError> {
    if node.node_type.is_empty() {
        return Err(NotesError::invalid(path, "node is missing a \"type\""));
    }

    // Text leaves must carry text and nothing structural; non-text nodes must not.
    if node.is_text() {
        if node.text.is_none() {
            return Err(NotesError::invalid(path, "text node has no \"text\""));
        }
        if !node.content.is_empty() {
            return Err(NotesError::invalid(
                path,
                "text node must not have child content",
            ));
        }
    } else if node.text.is_some() {
        return Err(NotesError::invalid(
            path,
            format!(
                "non-text node \"{}\" must not carry \"text\"",
                node.node_type
            ),
        ));
    }

    // Heading level sanity.
    if node.node_type == "heading" {
        match node.attr_i64("level") {
            None => {
                return Err(NotesError::invalid(path, "heading is missing attrs.level"));
            }
            Some(l) if !(1..=MAX_HEADING_LEVEL).contains(&l) => {
                return Err(NotesError::invalid(
                    path,
                    format!("heading level {l} out of range 1..={MAX_HEADING_LEVEL}"),
                ));
            }
            _ => {}
        }
    }

    // Unknown node types pass (forward-compat via `doc_schema_version`); only the
    // invariant shape asserted above is enforced.
    for (i, mark) in node.marks.iter().enumerate() {
        validate_mark(mark, &format!("{path}.marks[{i}]"))?;
    }
    for (i, child) in node.content.iter().enumerate() {
        validate_node(child, &format!("{path}.content[{i}]"))?;
    }
    Ok(())
}

fn validate_mark(mark: &Mark, path: &str) -> Result<(), NotesError> {
    if mark.mark_type.is_empty() {
        return Err(NotesError::invalid(path, "mark is missing a \"type\""));
    }
    if SEMANTIC_MARKS.contains(&mark.mark_type.as_str()) {
        // A semantic mark must resolve to *something* to project a link edge.
        let ok = match mark.mark_type.as_str() {
            "wikilink" => {
                mark.attr_str("target").is_some()
                    || mark.attr_str("targetId").is_some()
                    || mark.attr_str("href").is_some()
            }
            "tag" => mark.attr_str("name").is_some() || mark.attr_str("label").is_some(),
            "mention" => {
                mark.attr_str("label").is_some()
                    || mark.attr_str("personId").is_some()
                    || mark.attr_str("id").is_some()
            }
            _ => true,
        };
        if !ok {
            return Err(NotesError::invalid(
                path,
                format!("{} mark has no resolvable target attribute", mark.mark_type),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn from(v: serde_json::Value) -> Node {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn accepts_a_well_formed_doc() {
        let d = from(json!({
            "type": "doc",
            "content": [
                { "type": "heading", "attrs": { "level": 2, "blockId": "h" },
                  "content": [ { "type": "text", "text": "Hi" } ] },
                { "type": "paragraph", "content": [
                    { "type": "text", "text": "see ",
                      "marks": [] },
                    { "type": "text", "text": "Foo",
                      "marks": [ { "type": "wikilink", "attrs": { "target": "Foo" } } ] }
                ] }
            ]
        }));
        assert!(validate(&d).is_ok());
    }

    #[test]
    fn rejects_non_doc_root() {
        let d = from(json!({ "type": "paragraph" }));
        assert!(validate(&d).is_err());
    }

    #[test]
    fn rejects_bad_heading_level() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "heading", "attrs": { "level": 9 } } ]
        }));
        assert!(validate(&d).is_err());
    }

    #[test]
    fn rejects_text_node_with_children() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "text", "text": "x",
                           "content": [ { "type": "text", "text": "y" } ] } ]
        }));
        assert!(validate(&d).is_err());
    }

    #[test]
    fn rejects_targetless_wikilink() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "paragraph", "content": [
                { "type": "text", "text": "x",
                  "marks": [ { "type": "wikilink", "attrs": {} } ] }
            ] } ]
        }));
        assert!(validate(&d).is_err());
    }
}

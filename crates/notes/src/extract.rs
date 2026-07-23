//! Link extraction: walk `doc_json` → `[[wikilink]]` / `#tag` / `@mention` edges
//! tagged with their source block id (Data Model §5.1 `link`, Architecture §3.1
//! "EXTRACT links", Feature Specs §1.2–§1.4).
//!
//! These become `origin='projected'` rows in the `link` table (rebuilt on every
//! save). Both structured **marks** (what the Tiptap editor emits) and **plain
//! text** tokens (what Markdown import produces) are recognized; a text node that
//! already carries a semantic mark is not re-scanned as plain text, so a single
//! reference never yields a duplicate edge.
//!
//! Resolution (title → `entity_id`, create-on-miss stubs) is *not* done here — that
//! is the `links` crate's job on save (HLD §8.1). Extraction only reports intent.

use app_domain::{BlockId, Id, LinkRel};
use once_cell::sync::Lazy;
use regex::Regex;

use crate::model::{is_leaf_block, Mark, Node};

/// One extracted, unresolved link reference from `doc_json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExtractedLink {
    /// The block the reference lives in (`link.src_block_id`), if inside a block.
    pub src_block_id: Option<BlockId>,
    /// Edge relationship: `Wikilink`, `Tagged`, or `Mention`.
    pub rel: LinkRel,
    /// The referenced title / tag name / mention label (used for title resolution).
    pub target_title: Option<String>,
    /// A block anchor within the target (`[[Title#^blockId]]` → `link.dst_block_id`).
    pub target_block_id: Option<BlockId>,
    /// Display alias (`[[Title|alias]]`).
    pub alias: Option<String>,
    /// A pre-resolved target `entity_id` when the editor already bound the mark.
    pub target_id: Option<Id>,
    /// Whether this reference is a transclusion embed (`![[note]]`); recorded in
    /// `link.data_json` since `LinkRel` has no dedicated `embed` variant (see gaps).
    pub is_embed: bool,
}

impl ExtractedLink {
    fn wikilink(src: Option<BlockId>) -> Self {
        Self {
            src_block_id: src,
            rel: LinkRel::Wikilink,
            target_title: None,
            target_block_id: None,
            alias: None,
            target_id: None,
            is_embed: false,
        }
    }
}

/// Extract every projected link reference from a document, in document order.
#[must_use]
pub fn extract_links(doc: &Node) -> Vec<ExtractedLink> {
    let mut out = Vec::new();
    walk(doc, None, &mut out);
    out
}

fn walk(node: &Node, current_block: Option<BlockId>, out: &mut Vec<ExtractedLink>) {
    for child in &node.content {
        // The nearest enclosing leaf block owns references beneath it.
        let block = if is_leaf_block(&child.node_type) {
            child.block_id().or_else(|| current_block.clone())
        } else {
            current_block.clone()
        };

        // Structured marks on this (text) node.
        let mut has_semantic_mark = false;
        for mark in &child.marks {
            if let Some(link) = mark_to_link(mark, block.clone()) {
                has_semantic_mark = true;
                out.push(link);
            }
        }

        // `embed` block node: attrs.target is a transclusion wikilink.
        if child.node_type == "embed" {
            if let Some(target) = child.attr_str("target").or_else(|| child.attr_str("note")) {
                let mut l = ExtractedLink::wikilink(block.clone());
                l.target_title = Some(target.to_string());
                l.target_id = child.attr_str("targetId").and_then(|s| s.parse().ok());
                l.is_embed = true;
                out.push(l);
            }
        }

        // Plain-text tokens (Markdown-imported prose) only when not already marked.
        if !has_semantic_mark {
            if let Some(text) = &child.text {
                scan_plain_text(text, block.clone(), out);
            }
        }

        walk(child, block, out);
    }
}

fn mark_to_link(mark: &Mark, src: Option<BlockId>) -> Option<ExtractedLink> {
    match mark.mark_type.as_str() {
        "wikilink" => {
            let mut l = ExtractedLink::wikilink(src);
            l.target_title = mark
                .attr_str("target")
                .or_else(|| mark.attr_str("href"))
                .map(str::to_string);
            l.target_id = mark
                .attr_str("targetId")
                .or_else(|| mark.attr_str("entityId"))
                .and_then(|s| s.parse().ok());
            l.target_block_id = mark.attr_str("blockId").map(BlockId::new);
            l.alias = mark.attr_str("alias").map(str::to_string);
            Some(l)
        }
        "tag" => Some(ExtractedLink {
            src_block_id: src,
            rel: LinkRel::Tagged,
            target_title: mark
                .attr_str("name")
                .or_else(|| mark.attr_str("label"))
                .map(str::to_string),
            target_block_id: None,
            alias: None,
            target_id: mark.attr_str("tagId").and_then(|s| s.parse().ok()),
            is_embed: false,
        }),
        "mention" => Some(ExtractedLink {
            src_block_id: src,
            rel: LinkRel::Mention,
            target_title: mark
                .attr_str("label")
                .or_else(|| mark.attr_str("id"))
                .map(str::to_string),
            target_block_id: None,
            alias: None,
            target_id: mark
                .attr_str("personId")
                .or_else(|| mark.attr_str("entityId"))
                .and_then(|s| s.parse().ok()),
            is_embed: false,
        }),
        _ => None,
    }
}

// `[[Title]]`, `[[Title#^block]]`, `[[Title|alias]]`, and `![[embed]]`.
static WIKILINK_RE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(!?)\[\[([^\]|#]+?)(?:#\^?([^\]|]+))?(?:\|([^\]]+))?\]\]")
        .expect("valid wikilink regex")
});
// `#tag`, `#area/subarea` — not preceded by a word char (avoids `a#b`).
static TAG_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:^|[^\w#])#([A-Za-z0-9_][\w/\-]*)").expect("valid tag regex"));
// `@mention` — not preceded by a word char (avoids emails `a@b`).
static MENTION_RE: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"(?:^|[^\w@])@([A-Za-z0-9_][\w.\-]*)").expect("valid mention regex"));

fn scan_plain_text(text: &str, src: Option<BlockId>, out: &mut Vec<ExtractedLink>) {
    for caps in WIKILINK_RE.captures_iter(text) {
        let mut l = ExtractedLink::wikilink(src.clone());
        l.is_embed = &caps[1] == "!";
        l.target_title = Some(caps[2].trim().to_string());
        l.target_block_id = caps.get(3).map(|m| BlockId::new(m.as_str().trim()));
        l.alias = caps.get(4).map(|m| m.as_str().trim().to_string());
        out.push(l);
    }
    for caps in TAG_RE.captures_iter(text) {
        out.push(ExtractedLink {
            src_block_id: src.clone(),
            rel: LinkRel::Tagged,
            target_title: Some(caps[1].to_string()),
            target_block_id: None,
            alias: None,
            target_id: None,
            is_embed: false,
        });
    }
    for caps in MENTION_RE.captures_iter(text) {
        out.push(ExtractedLink {
            src_block_id: src.clone(),
            rel: LinkRel::Mention,
            target_title: Some(caps[1].to_string()),
            target_block_id: None,
            alias: None,
            target_id: None,
            is_embed: false,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn from(v: serde_json::Value) -> Node {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn extracts_structured_marks_with_block_id() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "paragraph", "attrs": { "blockId": "p1" }, "content": [
                { "type": "text", "text": "See " },
                { "type": "text", "text": "Foo",
                  "marks": [ { "type": "wikilink",
                               "attrs": { "target": "Foo", "alias": "the foo" } } ] },
                { "type": "text", "text": " ", },
                { "type": "text", "text": "Work",
                  "marks": [ { "type": "tag", "attrs": { "name": "Work" } } ] }
            ] } ]
        }));
        let links = extract_links(&d);
        assert_eq!(links.len(), 2);
        let wl = &links[0];
        assert_eq!(wl.rel, LinkRel::Wikilink);
        assert_eq!(wl.src_block_id.as_ref().unwrap().as_str(), "p1");
        assert_eq!(wl.target_title.as_deref(), Some("Foo"));
        assert_eq!(wl.alias.as_deref(), Some("the foo"));
        assert_eq!(links[1].rel, LinkRel::Tagged);
        assert_eq!(links[1].target_title.as_deref(), Some("Work"));
    }

    #[test]
    fn extracts_plain_text_tokens() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "paragraph", "attrs": { "blockId": "p1" }, "content": [
                { "type": "text", "text": "link [[Bar#^b3|alias]] and #Todo and @sam here" }
            ] } ]
        }));
        let links = extract_links(&d);
        let wl = links.iter().find(|l| l.rel == LinkRel::Wikilink).unwrap();
        assert_eq!(wl.target_title.as_deref(), Some("Bar"));
        assert_eq!(wl.target_block_id.as_ref().unwrap().as_str(), "b3");
        assert_eq!(wl.alias.as_deref(), Some("alias"));
        assert!(links
            .iter()
            .any(|l| l.rel == LinkRel::Tagged && l.target_title.as_deref() == Some("Todo")));
        assert!(links
            .iter()
            .any(|l| l.rel == LinkRel::Mention && l.target_title.as_deref() == Some("sam")));
    }

    #[test]
    fn marked_text_is_not_double_counted() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "paragraph", "attrs": { "blockId": "p1" }, "content": [
                { "type": "text", "text": "#Work",
                  "marks": [ { "type": "tag", "attrs": { "name": "Work" } } ] }
            ] } ]
        }));
        // Exactly one edge, from the mark — the "#Work" literal is not re-scanned.
        let links = extract_links(&d);
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn embed_node_is_a_wikilink() {
        let d = from(json!({
            "type": "doc",
            "content": [ { "type": "embed", "attrs": { "blockId": "e1", "target": "Other" } } ]
        }));
        let links = extract_links(&d);
        assert_eq!(links.len(), 1);
        assert!(links[0].is_embed);
        assert_eq!(links[0].target_title.as_deref(), Some("Other"));
    }
}

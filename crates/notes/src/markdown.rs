//! Markdown import / export for notes (Data Model §15.1, Feature Specs §8.1).
//!
//! `doc_json` → CommonMark + GFM and back. Markdown is an interop *feature*, never
//! the storage format, so this is a pragmatic converter over the block/mark subset
//! the editor emits: headings, paragraphs, `todo` (`- [ ]` / `- [x]`), `callout`
//! (`> [!type]` admonitions), `blockquote`, fenced `code`, GFM `table`, thematic
//! break, bullet lists, and inline `[[wikilink]]` / `#tag` / `@mention` / bold /
//! italic / code marks. Import produces id-less nodes; run [`crate::projection::
//! ensure_block_ids`] afterward so re-import yields fresh blockIds (AC-8.1).

use serde_json::Value;

use crate::model::{Mark, Node};

/// Options controlling Markdown export.
#[derive(Clone, Copy, Debug, Default)]
pub struct MarkdownOptions {
    /// When `true`, wikilinks render as portable `[Title](note://<uuid>)` links;
    /// otherwise the Obsidian-style `[[Title]]` form is preserved (Data Model §15.1).
    pub portable_links: bool,
}

// ===========================================================================
// Export: doc_json -> Markdown
// ===========================================================================

/// Render a document to Markdown with default options.
#[must_use]
pub fn to_markdown(doc: &Node) -> String {
    to_markdown_with(doc, &MarkdownOptions::default())
}

/// Render a document to Markdown.
#[must_use]
pub fn to_markdown_with(doc: &Node, opts: &MarkdownOptions) -> String {
    let mut parts: Vec<(&str, String)> = Vec::new();
    for node in &doc.content {
        if let Some(md) = block_to_md(node, opts) {
            parts.push((node.node_type.as_str(), md));
        }
    }
    let mut out = String::new();
    for (i, (ty, md)) in parts.iter().enumerate() {
        if i > 0 {
            // Adjacent to-dos stay "tight" (single newline); everything else is
            // separated by a blank line so it round-trips through `from_markdown`.
            let tight = is_tight(parts[i - 1].0) && is_tight(ty);
            out.push_str(if tight { "\n" } else { "\n\n" });
        }
        out.push_str(md);
    }
    out
}

fn is_tight(node_type: &str) -> bool {
    matches!(node_type, "todo" | "taskItem")
}

fn block_to_md(node: &Node, opts: &MarkdownOptions) -> Option<String> {
    Some(match node.node_type.as_str() {
        "paragraph" => inline_to_md(&node.content, opts),
        "heading" => {
            let level = node.attr_i64("level").unwrap_or(1).clamp(1, 6) as usize;
            format!(
                "{} {}",
                "#".repeat(level),
                inline_to_md(&node.content, opts)
            )
        }
        "todo" | "taskItem" => {
            let mark = if node.attr_bool("checked").unwrap_or(false) {
                "x"
            } else {
                " "
            };
            format!("- [{}] {}", mark, inline_to_md(&node.content, opts))
        }
        "blockquote" => prefix_lines(&inner_paragraphs(node, opts), "> "),
        "callout" => {
            let kind = node.attr_str("type").unwrap_or("note");
            let body = prefix_lines(&inner_paragraphs(node, opts), "> ");
            format!("> [!{kind}]\n{body}")
        }
        "code" | "codeBlock" => {
            let lang = node
                .attr_str("language")
                .or_else(|| node.attr_str("lang"))
                .unwrap_or("");
            let body = node.flatten_text();
            format!("```{lang}\n{body}\n```")
        }
        "horizontalRule" | "divider" => "---".to_string(),
        "bulletList" => list_to_md(node, opts, false),
        "orderedList" => list_to_md(node, opts, true),
        "table" => table_to_md(node, opts),
        "embed" => {
            let target = node
                .attr_str("target")
                .or_else(|| node.attr_str("note"))
                .unwrap_or("");
            format!("![[{target}]]")
        }
        "image" => {
            let src = node.attr_str("src").unwrap_or("");
            let alt = node.attr_str("alt").unwrap_or("");
            format!("![{alt}]({src})")
        }
        // Unknown block: fall back to flattened text so no content is lost.
        _ => {
            let t = node.flatten_text();
            if t.is_empty() {
                return None;
            }
            t
        }
    })
}

/// Render each paragraph child of a container to one line.
fn inner_paragraphs(node: &Node, opts: &MarkdownOptions) -> String {
    node.content
        .iter()
        .map(|c| inline_to_md(&c.content, opts))
        .collect::<Vec<_>>()
        .join("\n")
}

fn prefix_lines(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|l| format!("{prefix}{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn list_to_md(node: &Node, opts: &MarkdownOptions, ordered: bool) -> String {
    let mut out = Vec::new();
    for (i, item) in node.content.iter().enumerate() {
        let marker = if ordered {
            format!("{}. ", i + 1)
        } else {
            "- ".to_string()
        };
        let text = inner_paragraphs(item, opts);
        out.push(format!("{marker}{text}"));
    }
    out.join("\n")
}

fn table_to_md(node: &Node, opts: &MarkdownOptions) -> String {
    let mut lines = Vec::new();
    let mut col_count = 0;
    for (r, row) in node.content.iter().enumerate() {
        let cells: Vec<String> = row
            .content
            .iter()
            .map(|cell| inline_to_md(&flatten_cell(cell), opts))
            .collect();
        col_count = col_count.max(cells.len());
        lines.push(format!("| {} |", cells.join(" | ")));
        if r == 0 {
            let sep = vec!["---"; col_count.max(1)].join(" | ");
            lines.push(format!("| {sep} |"));
        }
    }
    lines.join("\n")
}

/// A table cell may be `cell > paragraph > text` or `cell > text`; return its inline
/// run either way.
fn flatten_cell(cell: &Node) -> Vec<Node> {
    if cell.content.iter().any(|c| c.is_text()) {
        cell.content.clone()
    } else if let Some(p) = cell.content.first() {
        p.content.clone()
    } else {
        Vec::new()
    }
}

fn inline_to_md(nodes: &[Node], opts: &MarkdownOptions) -> String {
    let mut s = String::new();
    for node in nodes {
        match &node.text {
            Some(text) => s.push_str(&wrap_marks(text, &node.marks, opts)),
            None if node.node_type == "hardBreak" => s.push_str("  \n"),
            None => s.push_str(&inline_to_md(&node.content, opts)),
        }
    }
    s
}

fn wrap_marks(text: &str, marks: &[Mark], opts: &MarkdownOptions) -> String {
    // Semantic marks replace the whole token.
    for mark in marks {
        match mark.mark_type.as_str() {
            "wikilink" => return render_wikilink(text, mark, opts),
            "tag" => {
                let name = mark
                    .attr_str("name")
                    .or_else(|| mark.attr_str("label"))
                    .unwrap_or(text);
                return format!("#{name}");
            }
            "mention" => {
                let label = mark.attr_str("label").unwrap_or(text);
                return format!("@{label}");
            }
            _ => {}
        }
    }

    let has = |t: &str| marks.iter().any(|m| m.mark_type == t);
    let mut s = text.to_string();
    if has("code") {
        s = format!("`{s}`");
    }
    if has("strike") {
        s = format!("~~{s}~~");
    }
    if has("italic") {
        s = format!("*{s}*");
    }
    if has("bold") {
        s = format!("**{s}**");
    }
    // A plain formatting-only link mark: [text](href).
    if let Some(link) = marks.iter().find(|m| m.mark_type == "link") {
        if let Some(href) = link.attr_str("href") {
            s = format!("[{s}]({href})");
        }
    }
    s
}

fn render_wikilink(text: &str, mark: &Mark, opts: &MarkdownOptions) -> String {
    let target = mark
        .attr_str("target")
        .or_else(|| mark.attr_str("href"))
        .unwrap_or(text);
    if opts.portable_links {
        if let Some(id) = mark
            .attr_str("targetId")
            .or_else(|| mark.attr_str("entityId"))
        {
            return format!("[{text}](note://{id})");
        }
    }
    let mut inner = target.to_string();
    if let Some(block) = mark.attr_str("blockId") {
        inner.push_str(&format!("#^{block}"));
    }
    if let Some(alias) = mark.attr_str("alias") {
        inner.push_str(&format!("|{alias}"));
    } else if text != target {
        inner.push_str(&format!("|{text}"));
    }
    format!("[[{inner}]]")
}

// ===========================================================================
// Import: Markdown -> doc_json
// ===========================================================================

/// Parse Markdown into a `doc_json` document. Nodes carry no `blockId`; call
/// [`crate::projection::ensure_block_ids`] before persist.
#[must_use]
pub fn from_markdown(md: &str) -> Node {
    let body = strip_front_matter(md);
    let lines: Vec<&str> = body.lines().collect();
    let mut doc = Node::new("doc");
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }

        // Fenced code block.
        if let Some(lang) = fence_lang(line) {
            let mut body_lines = Vec::new();
            i += 1;
            while i < lines.len() && fence_lang(lines[i]).is_none() {
                body_lines.push(lines[i]);
                i += 1;
            }
            i += 1; // consume closing fence
            doc.content.push(code_node(&lang, &body_lines.join("\n")));
            continue;
        }

        // Thematic break.
        if is_thematic_break(line) {
            doc.content.push(Node::new("divider"));
            i += 1;
            continue;
        }

        // Heading.
        if let Some((level, text)) = heading_parts(line) {
            let mut h = Node::new("heading");
            h.attrs.insert("level".into(), Value::from(level as i64));
            h.content = parse_inline(text);
            doc.content.push(h);
            i += 1;
            continue;
        }

        // To-do.
        if let Some((checked, text)) = todo_parts(line) {
            let mut t = Node::new("todo");
            t.attrs.insert("checked".into(), Value::Bool(checked));
            t.content = parse_inline(text);
            doc.content.push(t);
            i += 1;
            continue;
        }

        // Callout / blockquote (both use `> `).
        if line.trim_start().starts_with('>') {
            let (node, next) = parse_quote(&lines, i);
            doc.content.push(node);
            i = next;
            continue;
        }

        // GFM table (`| ... |` with a separator row beneath).
        if is_table_row(line) && i + 1 < lines.len() && is_table_separator(lines[i + 1]) {
            let (node, next) = parse_table(&lines, i);
            doc.content.push(node);
            i = next;
            continue;
        }

        // Bullet list (grouped).
        if is_bullet(line) {
            let (node, next) = parse_list(&lines, i);
            doc.content.push(node);
            i = next;
            continue;
        }

        // Paragraph (single line).
        let mut p = Node::new("paragraph");
        p.content = parse_inline(line);
        doc.content.push(p);
        i += 1;
    }

    doc
}

fn code_node(lang: &str, body: &str) -> Node {
    let mut n = Node::new("code");
    if !lang.is_empty() {
        n.attrs
            .insert("language".into(), Value::from(lang.to_string()));
    }
    n.content.push(Node::text_node(body));
    n
}

fn strip_front_matter(md: &str) -> &str {
    if let Some(rest) = md.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let after = &rest[end + 4..];
            return after.strip_prefix('\n').unwrap_or(after);
        }
    }
    md
}

fn fence_lang(line: &str) -> Option<String> {
    let t = line.trim_start();
    t.strip_prefix("```").map(|rest| rest.trim().to_string())
}

fn is_thematic_break(line: &str) -> bool {
    let t = line.trim();
    t == "---" || t == "***" || t == "___"
}

fn heading_parts(line: &str) -> Option<(usize, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=6).contains(&hashes) && line.as_bytes().get(hashes) == Some(&b' ') {
        Some((hashes, line[hashes + 1..].trim_end()))
    } else {
        None
    }
}

fn todo_parts(line: &str) -> Option<(bool, &str)> {
    let t = line.trim_start();
    if let Some(rest) = t.strip_prefix("- [ ] ") {
        Some((false, rest))
    } else if let Some(rest) = t
        .strip_prefix("- [x] ")
        .or_else(|| t.strip_prefix("- [X] "))
    {
        Some((true, rest))
    } else {
        None
    }
}

fn is_bullet(line: &str) -> bool {
    let t = line.trim_start();
    (t.starts_with("- ") || t.starts_with("* ")) && todo_parts(line).is_none()
}

fn is_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn is_table_separator(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn parse_quote(lines: &[&str], start: usize) -> (Node, usize) {
    let mut i = start;
    let mut body = Vec::new();
    let first = lines[i].trim_start();
    // Callout admonition marker `> [!type]`.
    let callout_kind = first
        .strip_prefix("> [!")
        .or_else(|| first.strip_prefix(">[!"))
        .and_then(|r| r.split(']').next())
        .map(str::to_string);
    if callout_kind.is_some() {
        i += 1;
    }
    while i < lines.len() && lines[i].trim_start().starts_with('>') {
        let content = lines[i]
            .trim_start()
            .trim_start_matches('>')
            .strip_prefix(' ')
            .unwrap_or_else(|| lines[i].trim_start().trim_start_matches('>'));
        body.push(content.to_string());
        i += 1;
    }
    let node_type = if callout_kind.is_some() {
        "callout"
    } else {
        "blockquote"
    };
    let mut node = Node::new(node_type);
    if let Some(kind) = callout_kind {
        node.attrs.insert("type".into(), Value::from(kind));
    }
    for line in body {
        let mut p = Node::new("paragraph");
        p.content = parse_inline(&line);
        node.content.push(p);
    }
    (node, i)
}

fn parse_table(lines: &[&str], start: usize) -> (Node, usize) {
    let mut i = start;
    let mut table = Node::new("table");
    let mut row_idx = 0;
    while i < lines.len() && is_table_row(lines[i]) {
        if is_table_separator(lines[i]) {
            i += 1;
            continue;
        }
        let cells = split_table_cells(lines[i]);
        let mut row = Node::new("tableRow");
        for c in cells {
            let cell_type = if row_idx == 0 {
                "tableHeader"
            } else {
                "tableCell"
            };
            let mut cell = Node::new(cell_type);
            let mut p = Node::new("paragraph");
            p.content = parse_inline(&c);
            cell.content.push(p);
            row.content.push(cell);
        }
        table.content.push(row);
        row_idx += 1;
        i += 1;
    }
    (table, i)
}

fn split_table_cells(line: &str) -> Vec<String> {
    let t = line.trim().trim_matches('|');
    t.split('|').map(|c| c.trim().to_string()).collect()
}

fn parse_list(lines: &[&str], start: usize) -> (Node, usize) {
    let mut i = start;
    let mut list = Node::new("bulletList");
    while i < lines.len() && is_bullet(lines[i]) {
        let t = lines[i].trim_start();
        let text = t[2..].trim_start();
        let mut item = Node::new("listItem");
        let mut p = Node::new("paragraph");
        p.content = parse_inline(text);
        item.content.push(p);
        list.content.push(item);
        i += 1;
    }
    (list, i)
}

// ---------------------------------------------------------------------------
// Inline tokenizer: [[wiki]], #tag, @mention, **bold**, *italic*, `code`.
// ---------------------------------------------------------------------------

fn parse_inline(s: &str) -> Vec<Node> {
    let bytes = s.as_bytes();
    let mut nodes = Vec::new();
    let mut buf = String::new();
    let mut i = 0;

    let flush = |buf: &mut String, nodes: &mut Vec<Node>| {
        if !buf.is_empty() {
            nodes.push(Node::text_node(std::mem::take(buf)));
        }
    };

    while i < s.len() {
        let rest = &s[i..];

        if let Some((node, len)) = take_wikilink(rest) {
            flush(&mut buf, &mut nodes);
            nodes.push(node);
            i += len;
            continue;
        }
        if let Some((node, len)) = take_wrapped(rest, "`", "code") {
            flush(&mut buf, &mut nodes);
            nodes.push(node);
            i += len;
            continue;
        }
        if let Some((node, len)) = take_wrapped(rest, "**", "bold") {
            flush(&mut buf, &mut nodes);
            nodes.push(node);
            i += len;
            continue;
        }
        if let Some((node, len)) = take_wrapped(rest, "*", "italic") {
            flush(&mut buf, &mut nodes);
            nodes.push(node);
            i += len;
            continue;
        }
        let prev_is_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if prev_is_boundary {
            if let Some((node, len)) = take_prefixed(rest, '#', "tag", "name") {
                flush(&mut buf, &mut nodes);
                nodes.push(node);
                i += len;
                continue;
            }
            if let Some((node, len)) = take_prefixed(rest, '@', "mention", "label") {
                flush(&mut buf, &mut nodes);
                nodes.push(node);
                i += len;
                continue;
            }
        }

        // Default: consume one char.
        let ch = rest.chars().next().unwrap();
        buf.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut buf, &mut nodes);
    nodes
}

fn take_wikilink(rest: &str) -> Option<(Node, usize)> {
    let (embed, open_len) = if rest.starts_with("![[") {
        (true, 3)
    } else if rest.starts_with("[[") {
        (false, 2)
    } else {
        return None;
    };
    let inner_end = rest[open_len..].find("]]")?;
    let inner = &rest[open_len..open_len + inner_end];
    let total = open_len + inner_end + 2;

    // inner = Title[#^block][|alias]
    let (head, alias) = match inner.split_once('|') {
        Some((h, a)) => (h, Some(a.to_string())),
        None => (inner, None),
    };
    let (target, block) = match head.split_once('#') {
        Some((t, b)) => (t.trim(), Some(b.trim_start_matches('^').trim().to_string())),
        None => (head.trim(), None),
    };

    let display = alias.clone().unwrap_or_else(|| target.to_string());
    let mut mark = Mark::new("wikilink");
    mark.attrs
        .insert("target".into(), Value::from(target.to_string()));
    if let Some(b) = block {
        mark.attrs.insert("blockId".into(), Value::from(b));
    }
    if let Some(a) = alias {
        mark.attrs.insert("alias".into(), Value::from(a));
    }
    if embed {
        mark.attrs.insert("embed".into(), Value::Bool(true));
    }
    let mut node = Node::text_node(display);
    node.marks.push(mark);
    Some((node, total))
}

fn take_wrapped(rest: &str, delim: &str, mark_type: &str) -> Option<(Node, usize)> {
    let inner_start = delim.len();
    if !rest.starts_with(delim) {
        return None;
    }
    let close = rest[inner_start..].find(delim)?;
    if close == 0 {
        return None; // empty
    }
    let inner = &rest[inner_start..inner_start + close];
    let total = inner_start + close + delim.len();
    let mut node = Node::text_node(inner.to_string());
    node.marks.push(Mark::new(mark_type));
    Some((node, total))
}

fn take_prefixed(rest: &str, sigil: char, mark_type: &str, attr: &str) -> Option<(Node, usize)> {
    if rest.chars().next()? != sigil {
        return None;
    }
    let first = rest[sigil.len_utf8()..].chars().next()?;
    if !(first.is_ascii_alphanumeric() || first == '_') {
        return None;
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '/' | '-' | '.');
    let mut end = sigil.len_utf8();
    for c in rest[sigil.len_utf8()..].chars() {
        if allowed(c) {
            end += c.len_utf8();
        } else {
            break;
        }
    }
    let name = &rest[sigil.len_utf8()..end];
    // Disallow a trailing '.'/'-' so "@sam." keeps the period as prose.
    let name = name.trim_end_matches(['.', '-']);
    if name.is_empty() {
        return None;
    }
    let mut mark = Mark::new(mark_type);
    mark.attrs
        .insert(attr.into(), Value::from(name.to_string()));
    let mut node = Node::text_node(name.to_string());
    node.marks.push(mark);
    Some((node, sigil.len_utf8() + name.len()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_round_trips() {
        let md = "\
# Title

A paragraph with a [[Foo]] link, an [[Bar|alias]], a #Work tag, an @sam mention, and **bold** text.

- [ ] open task
- [x] done task

> [!info]
> a callout body

> just a quote

- first
- second

```rust
let x = 1;
```

| a | b |
| --- | --- |
| c | d |

---";
        let doc = from_markdown(md);
        let out = to_markdown(&doc);
        assert_eq!(out, md, "\n--- got ---\n{out}\n--- want ---\n{md}");
    }

    #[test]
    fn import_projects_link_marks() {
        let doc = from_markdown("see [[Foo#^b1|the foo]] and #Bar");
        let para = &doc.content[0];
        let wl = para
            .content
            .iter()
            .find(|n| n.marks.iter().any(|m| m.mark_type == "wikilink"))
            .unwrap();
        let mark = &wl.marks[0];
        assert_eq!(mark.attr_str("target"), Some("Foo"));
        assert_eq!(mark.attr_str("blockId"), Some("b1"));
        assert_eq!(mark.attr_str("alias"), Some("the foo"));
        assert_eq!(wl.text.as_deref(), Some("the foo"));
    }

    #[test]
    fn portable_links_use_note_uri() {
        let mut para = Node::new("paragraph");
        let mut mark = Mark::new("wikilink");
        mark.attrs.insert("target".into(), Value::from("Foo"));
        mark.attrs
            .insert("targetId".into(), Value::from("018f-uuid"));
        let mut t = Node::text_node("Foo");
        t.marks.push(mark);
        para.content.push(t);
        let mut doc = Node::new("doc");
        doc.content.push(para);

        let opts = MarkdownOptions {
            portable_links: true,
        };
        assert_eq!(to_markdown_with(&doc, &opts), "[Foo](note://018f-uuid)");
    }
}

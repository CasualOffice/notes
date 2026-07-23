//! M1 NFR gate — **save → projection < 50 ms p95** (roadmap §5 / PRD NFR).
//!
//! On every note save the Rust side runs a pure, IO-free pipeline over the editor
//! `doc_json` before the single DB write. This benchmark-style test exercises that
//! exact pipeline and asserts its p95 wall-clock stays well under the 50 ms budget.
//!
//! ## The measured path (one "save")
//! Per iteration, over a freshly-parsed document (so parsing cost is included):
//! 1. `Node::from_json` — deserialize `doc_json` (ProseMirror JSON) into the tree.
//! 2. [`notes::validate`] — schema-validate the document (via `validate_and_project`).
//! 3. [`notes::ensure_block_ids`] — stamp `blockId`s on new leaf blocks.
//! 4. [`notes::project_blocks`] — flatten to ordered `block` rows.
//! 5. [`notes::extract_links`] — scan marks + plain text for `[[wiki]]`/`#tag`/`@mention`.
//!
//! Steps 2–5 are `notes::validate_and_project`. Everything downstream (the SQL
//! upserts / FTS reproject) is `storage`'s single-writer transaction and is not the
//! CPU work this gate governs — this measures the projection compute that stands
//! between a keystroke-triggered save and the committed derived state.
//!
//! The bound is deliberately generous relative to the 50 ms gate so the test is
//! stable on loaded CI while still proving a comfortable margin: locally the p95 is
//! sub-2 ms for ~320 blocks. If this ever regresses past `P95_BUDGET_MS` the gate
//! fails loudly.

use std::time::Instant;

use app_domain::{BlockId, LinkRel, NoteId};
use notes::model::Node;
use notes::validate_and_project;
use serde_json::{json, Value};

/// Iterations timed for the p95 estimate.
const ITERS: usize = 200;
/// Warm-up iterations (prime the regex `Lazy` statics, allocator, and caches)
/// excluded from the measurement.
const WARMUP: usize = 20;
/// CI-safe p95 ceiling — well under the 50 ms M1 gate (2x margin) so a loaded
/// runner does not flake while a real regression still trips it.
const P95_BUDGET_MS: f64 = 25.0;

fn heading(i: usize) -> Value {
    json!({
        "type": "heading",
        "attrs": { "level": (i % 3) as i64 + 1 },
        "content": [ { "type": "text", "text": format!("Section {i}") } ]
    })
}

fn marked_paragraph(i: usize) -> Value {
    json!({
        "type": "paragraph",
        "content": [
            { "type": "text", "text": "See " },
            { "type": "text", "text": format!("Note{i}"),
              "marks": [ { "type": "wikilink", "attrs": { "target": format!("Note{i}") } } ] },
            { "type": "text", "text": " about " },
            { "type": "text", "text": "work",
              "marks": [ { "type": "tag", "attrs": { "name": "work" } } ] },
            { "type": "text", "text": " with " },
            { "type": "text", "text": "sam",
              "marks": [ { "type": "mention", "attrs": { "label": "sam" } } ] },
            { "type": "text", "text": " — some trailing prose to give the block body length." }
        ]
    })
}

fn plain_paragraph(i: usize) -> Value {
    json!({
        "type": "paragraph",
        "content": [ { "type": "text",
            "text": format!("Imported line {i} links [[Topic {i}]] and #area/sub and @jane here.") } ]
    })
}

fn todo(i: usize) -> Value {
    json!({
        "type": "todo",
        "attrs": { "checked": i.is_multiple_of(2) },
        "content": [ { "type": "text", "text": format!("action item {i}") } ]
    })
}

fn bullet_list(i: usize) -> Value {
    json!({
        "type": "bulletList",
        "content": [
            { "type": "listItem", "content": [
                { "type": "paragraph", "content": [
                    { "type": "text", "text": format!("bullet {i} a with [[Ref{i}]]") } ] } ] },
            { "type": "listItem", "content": [
                { "type": "paragraph", "content": [
                    { "type": "text", "text": format!("bullet {i} b #tag{i}") } ] } ] }
        ]
    })
}

fn code_block(i: usize) -> Value {
    json!({
        "type": "codeBlock",
        "attrs": { "language": "rust" },
        "content": [ { "type": "text", "text": format!("let x = {i}; // not a #tag inside code") } ]
    })
}

fn callout(i: usize) -> Value {
    json!({
        "type": "callout",
        "attrs": { "kind": "note" },
        "content": [ { "type": "text", "text": format!("callout {i} referencing [[Handbook]]") } ]
    })
}

fn blockquote(i: usize) -> Value {
    json!({
        "type": "blockquote",
        "content": [ { "type": "text", "text": format!("quote {i} — @alex said something") } ]
    })
}

/// Build a realistic multi-hundred-block note: headings, prose with structured
/// `wikilink`/`tag`/`mention` marks, Markdown-style `[[wiki]]`/`#tag`/`@mention`
/// plain-text tokens, todos, nested lists, code, callouts, and blockquotes.
fn realistic_doc_json(sections: usize) -> String {
    let mut content: Vec<Value> = Vec::with_capacity(sections);
    for i in 0..sections {
        let block = match i % 8 {
            0 => heading(i),
            1 => marked_paragraph(i),
            2 => plain_paragraph(i),
            3 => todo(i),
            4 => bullet_list(i),
            5 => code_block(i),
            6 => callout(i),
            _ => blockquote(i),
        };
        content.push(block);
    }
    serde_json::to_string(&json!({ "type": "doc", "content": content })).expect("serialize doc")
}

/// Run the full save→projection pipeline once over a fresh parse of `doc_json`,
/// returning `(block_count, link_count)` so the caller can prove real work ran.
fn run_pipeline(doc_json: &str) -> (usize, usize) {
    let mut doc = Node::from_json(doc_json).expect("parse doc_json");
    let note = NoteId::new();
    let mut n: u64 = 0;
    let mut gen = || {
        n += 1;
        BlockId::new(format!("b{n}"))
    };
    let (blocks, links) = validate_and_project(note, &mut doc, &mut gen).expect("project");
    (blocks.len(), links.len())
}

#[test]
fn save_to_projection_p95_under_budget() {
    // ~320 leaf-ish blocks (some sections expand to multiple blocks) — a large,
    // realistic single note. This is the source of truth we re-parse each pass.
    let doc_json = realistic_doc_json(320);

    // Sanity: the pipeline must produce a substantial projection with all three
    // link relations, or we would be timing a no-op and the gate would be moot.
    let (blocks, links) = run_pipeline(&doc_json);
    assert!(
        blocks >= 300,
        "expected a few hundred blocks, projected {blocks}"
    );
    assert!(links > 0, "expected extracted links, got {links}");
    {
        let mut doc = Node::from_json(&doc_json).unwrap();
        let mut n = 0u64;
        let mut gen = || {
            n += 1;
            BlockId::new(format!("b{n}"))
        };
        let (_b, ls) = validate_and_project(NoteId::new(), &mut doc, &mut gen).unwrap();
        assert!(ls.iter().any(|l| l.rel == LinkRel::Wikilink));
        assert!(ls.iter().any(|l| l.rel == LinkRel::Tagged));
        assert!(ls.iter().any(|l| l.rel == LinkRel::Mention));
    }

    // Warm up (regex Lazy statics, allocator) outside the measurement.
    for _ in 0..WARMUP {
        let _ = run_pipeline(&doc_json);
    }

    let mut samples_ms: Vec<f64> = Vec::with_capacity(ITERS);
    for _ in 0..ITERS {
        let start = Instant::now();
        let (b, _l) = run_pipeline(&doc_json);
        let elapsed = start.elapsed();
        // Touch the result so the optimizer cannot elide the pipeline.
        std::hint::black_box(b);
        samples_ms.push(elapsed.as_secs_f64() * 1_000.0);
    }

    samples_ms.sort_by(|a, b| a.partial_cmp(b).expect("no NaN latencies"));
    let p50 = samples_ms[samples_ms.len() / 2];
    let p95_idx = ((samples_ms.len() * 95) / 100).min(samples_ms.len() - 1);
    let p95 = samples_ms[p95_idx];
    let max = *samples_ms.last().unwrap();

    // Visible with `cargo test -- --nocapture` for regression tracking.
    eprintln!("save->projection over {blocks} blocks: p50={p50:.3}ms p95={p95:.3}ms max={max:.3}ms (budget p95<{P95_BUDGET_MS}ms, M1 gate 50ms)");

    assert!(
        p95 < P95_BUDGET_MS,
        "save->projection p95 {p95:.3}ms exceeded the {P95_BUDGET_MS}ms CI budget (M1 gate is 50ms p95); p50={p50:.3}ms max={max:.3}ms over {blocks} blocks"
    );
}

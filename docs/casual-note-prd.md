# Casual Note — Product Requirements Document (PRD)

*The master product spec for a fully-local, privacy-first notebook that also masters your meetings.*

**Status:** v1 canonical PRD. Consistent with, and subordinate to, the **Design Foundation** (the canonical spine). Where this document and the Foundation disagree, the Foundation wins and this document is corrected. This PRD owns the *what* and *why* for humans; it defers schema to **Data Model**, subsystem runtime design to **HLD**, decomposition to **Architecture**, and per-feature behavior detail to **Feature Specs**.

**Document owner:** Product. **Audience:** product, design, engineering, QA, and stakeholders. **Last updated:** 2026-07-23.

---

## Table of Contents

1. Vision & Positioning
2. The "Casual Notebook + Meeting Mastery" Story
3. Personas & Jobs-to-be-Done
4. The Four Pillars + Unified Search & AI Workspace
5. Functional Requirements Catalog (FR)
6. Non-Functional Requirements (NFR)
7. Success Metrics
8. Out-of-Scope / Non-Goals (v1)
9. Assumptions & Dependencies
10. Glossary

---

## 1. Vision & Positioning

### 1.1 Vision

**Casual Note** is a warm, everyday, fully-local personal knowledge space — notes, tasks, and reminders — that *also* happens to be a best-in-class local meeting recorder and understander. It leads with the notebook. The everyday surface is writing, capturing, planning, and remembering. Meeting intelligence — inherited and carried forward from the prior **EchoNote** product — is a deep, differentiating capability that lives *inside* the notebook rather than beside it. A meeting becomes a note; its action items become tasks; its decisions become searchable, cited facts — all in one encrypted store, one search index, one link graph, one AI workspace.

**Tagline:** *"Casually a notebook. Quietly, the best local meeting mind you'll ever own."*

### 1.2 Positioning

Casual Note sits at the intersection of three product categories that today force users to stitch together separate tools:

```
        PKM / Notebook            Task Manager            Meeting AI
     (Obsidian, Logseq,      (Things, Todoist,      (Otter, Fireflies,
        Notion, Tana)          Reminders.app)          cloud bots)
             \                       |                      /
              \                      |                     /
               \                     |                    /
                +-------------------- CASUAL NOTE --------+
                     one encrypted local store
                     one search + one link graph
                     one evidence-citing AI workspace
                     100% on-device, offline-capable
```

Cloud PKM and meeting-bot SaaS trade confidentiality, cost, and control for convenience. Casual Note refuses that trade: everything runs on-device, with the network touched only by two named, user-consented services (`model-download`, `updater`).

### 1.3 Core Differentiators

- **Four pillars, one brain.** Notes, reminders, tasks, and meetings share one encrypted local store, one search index, one link graph, and one AI workspace that reasons across all of them with evidence citations.
- **100% local by default.** All capture, transcription, LLM reasoning, and embeddings run on-device. "Offline Ready" is a first-class, visible state — not a degraded mode.
- **Evidence-carrying intelligence.** Every AI-produced fact — a meeting decision, an "ask your notes" answer, a suggested tag — cites a concrete source (note block, transcript segment + timestamp). The model never invents owners or dates.
- **Best-in-class local capture.** Native per-platform audio capture (macOS ScreenCaptureKit, Windows WASAPI process-loopback, Linux PipeWire) that no cross-platform Electron notebook can match, with honest per-platform capability reporting.

### 1.4 Why Now

On-device models crossed the usability threshold: small-footprint STT (whisper.cpp, Parakeet) and 4–14B LLMs (Qwen3 via llama.cpp/MLX) now run acceptably on consumer hardware, and compact embedders (EmbeddingGemma-300M, bge-base) make local semantic search practical. Privacy expectations have hardened in parallel. The technical baseline for a fully-local meeting recorder already exists (EchoNote); extending it into a unified notebook is the highest-leverage next step.

---

## 2. The "Casual Notebook + Meeting Mastery" Story

Casual Note is *casually a notebook*: the first thing a user does is write. On first launch they land in **today's daily note** inside a **default notebook**, with a hardware-appropriate model tier already selected — no account, no server, no wizard. Quick capture is under two seconds via a global hotkey and a frameless always-on-top panel.

Beneath that casual surface is meeting mastery. When a call starts, the same app captures audio from a chosen desktop application and/or the microphone, transcribes locally in two passes (live + final), and generates an evidence-linked **MeetingArtifactV1** (executive summary, topics, decisions, action items, risks, open questions). Because a meeting *is* a note in the unified store, its output flows without re-typing:

```
  QUICK CAPTURE ─► DAILY NOTE ─► NOTE ─► [[backlinks]] / #tags / @mentions
        │                          │
        │                          ▼
        │                   TASK  ◄──────────── ACTION ITEM
        │                    │        spawned_from (+ evidence_segment_ids)
        │                    ▼
        │                REMINDER  ─► native OS notification
        ▼
   MEETING (record) ─► live transcript ─► FINAL transcript ─► ARTIFACT
        │                                                        │
        └──────────────► one SEARCH (FTS ∪ vector, RRF) ◄────────┘
                                    │
                                    ▼
                      AI WORKSPACE ("Ask your notes") ─► cited AnswerV1
```

The end-to-end promise: **capture → note → task → meeting → ask**, all local, all cited, all in one place. A thought captured on the daily note, a decision extracted from this morning's call, and a task due Friday are the same kind of citizen in the same graph — one Cmd/Ctrl-K palette goes to any of them, does commands over them, or asks questions across them.

---

## 3. Personas & Jobs-to-be-Done

### 3.1 Personas

| ID | Persona | Description | Primary pillars |
|----|---------|-------------|-----------------|
| P1 | **The Consultant** ("Maya") | Client-confidential work; lives in back-to-back calls; bills by outcome. Cannot legally put client audio in a cloud bot. | Meetings, Tasks, Search |
| P2 | **The Founder/PM** ("Devin") | Runs many meetings, owns follow-through, plans across projects. Wants action items to become tracked work automatically. | Meetings, Tasks, Reminders |
| P3 | **The Researcher** ("Lin") | Deep note-taker; interviews, literature, linked thinking. Values durable, private, linkable knowledge and offline recall. | Notes, Search, AI |
| P4 | **The Power Note-Taker** ("Sam") | The Obsidian/Logseq/Things audience. Wants capture + tasks + meetings unified without stitching five tools. | Notes, Tasks, Reminders |
| P5 | **The Privacy Absolutist** ("Ari") | Rejects cloud PKM/meeting SaaS on principle. Will only adopt tools that are provably local and offline-capable. | All (gated on privacy) |

### 3.2 Jobs-to-be-Done

1. **JTBD-1 — Frictionless capture.** "Capture a thought, a task, or a reminder in under two seconds without breaking flow."
2. **JTBD-2 — Durable knowledge.** "Keep my personal knowledge — writing, links, dailies — durable and mine."
3. **JTBD-3 — Meeting to action.** "Record and understand a meeting locally, then act on it (tasks, follow-ups) without re-typing."
4. **JTBD-4 — Ask everything.** "Ask my whole knowledge base a question and get a cited answer, offline."
5. **JTBD-5 — Never drop a commitment.** "Reminders and tasks that survive app closes and crashes."

Each functional requirement below traces to at least one JTBD.

---

## 4. The Four Pillars + Unified Search & AI Workspace

### 4.1 Notes (knowledge pillar)

A rich block-based / Markdown notebook. Notebooks and folders (nested tree), daily notes, first-class tags, wiki-style `[[backlinks]]`, checklists, attachments, code blocks, tables, callouts, and quick capture. The document model is **JSON-doc-as-truth** (Tiptap/ProseMirror JSON in encrypted SQLite) with a derived block/link/FTS index; Markdown is an import/export *feature*, not the storage format. Every block carries a stable `blockId` so backlinks, reminders, and meeting evidence can anchor to it precisely.

### 4.2 Reminders (planning pillar)

Time-based and recurring reminders delivered through native OS notifications, with snooze, rich actions, and natural-language entry ("remind me tomorrow 3pm"). Reminders are a first-class polymorphic entity that can attach to a task, a note, a note block, or a meeting — or stand alone. Delivery uses a **dual-layer scheduler**: an in-app Tokio timer-wheel (authoritative while running) plus a rolling-horizon OS one-shot registration (survives app-closed on macOS/Windows), with honest reporting that Linux has no persistent OS layer.

### 4.3 Tasks (planning pillar, "Things"-style)

Projects and areas, headings, and the derived views **Today / Upcoming / Anytime / Someday** — computed as queries over fields, not stored states. Tasks precisely split three temporal concepts: `start_on` (When/scheduled — *hides* the task until then), `deadline_on` (due — does *not* hide), and a separate Reminder (alert time). Checklists and nested subtasks, natural-language quick entry, and O(1) drag-reorder via fractional indices. Tasks link to notes and meetings; meeting action items flow into tasks with provenance carried on the edge.

### 4.4 Meeting Intelligence (meeting pillar, inherited)

Capture audio from a selected desktop application and/or microphone; local two-pass transcription (live streaming + final); local-LLM generation of an evidence-linked **MeetingArtifactV1**. The session lifecycle is governed by an explicit state machine (`NEW → PREFLIGHT → READY → RECORDING ↔ PAUSED → STOPPING → CAPTURED → FINAL_TRANSCRIBING → GENERATING → INDEXING → COMPLETE`, plus `DEGRADED/FAILED/RECOVERING`). Every extracted fact carries transcript evidence; owners and dates are only populated when the model extracted them from evidence.

### 4.5 Unified Search

One search over notes + tasks + reminders + meetings + transcripts. **FTS5 (BM25) ∪ sqlite-vec KNN, fused by Reciprocal Rank Fusion (RRF)** over a universal `chunk`/`entity` spine. FTS returns synchronously (<10ms perceived); embeddings stream in and re-fuse. First-class filters (`type:`, `tag:`, `date:`, `person:`, `is:`) compile to SQL predicates before fusion. One command palette (Cmd/Ctrl-K), three modes: **Go** (quick-switcher), **Do** (commands), **Ask** (RAG).

### 4.6 AI Workspace

Retrieval-augmented, evidence-cited reasoning over the whole store: retrieve (hybrid + RRF, optional reranker) → grounded prompt with numbered evidence → constrained-decode **AnswerV1** `{answer, citations[], confidence, unanswered}` → **verify every citation resolves to a real chunk** before display. If nothing grounds the answer, the workspace returns `unanswered: true` ("I couldn't find this in your notes") rather than hallucinate. Auto-link and auto-tag are surfaced as reversible, cited `suggestion` rows the user approves — never silent edits.

---

## 5. Functional Requirements Catalog (FR)

Priority key: **P0** = must ship in v1; **P1** = should ship in v1; **P2** = v1 if capacity allows, else fast-follow. "Traces" links each FR to a JTBD.

### 5.1 Notes (FR-N)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-N01 | Create, edit, and delete notes in a block-based rich editor (paragraphs, headings, lists). `doc_json` is the source of truth and is schema-validated before persist. | P0 | JTBD-2 |
| FR-N02 | Organize notes in a nested notebook/folder tree; move notes between containers; reorder via fractional indices. | P0 | JTBD-2 |
| FR-N03 | Daily notes: one note per calendar date, auto-created on first access, reachable from a date navigator; today's daily note opens on launch. | P0 | JTBD-1, JTBD-2 |
| FR-N04 | Block types: heading, paragraph, bullet/numbered list, todo/checklist, code block (syntax-highlighted), table, callout, quote, divider, embed, and transcript-segment. | P0 | JTBD-2 |
| FR-N05 | Wiki-style `[[wikilinks]]` with autocomplete; create-on-type for missing targets; block-level link targets via `blockId`. | P0 | JTBD-2 |
| FR-N06 | Backlinks panel per note showing linked references, resolved from the polymorphic link table (derived on read, never dual-written). | P0 | JTBD-2 |
| FR-N07 | Unlinked-mention detection: surface occurrences of a note's title/aliases elsewhere, with one-click link creation. | P1 | JTBD-2 |
| FR-N08 | `#tags` as first-class entities with autocomplete; optional supertag-lite schema lending fields to tagged notes. | P1 | JTBD-2 |
| FR-N09 | `@mentions` of people (Person entity); mention becomes a link and a searchable filter. | P1 | JTBD-2 |
| FR-N10 | Attachments: drag/drop or paste files/images; content-addressed (SHA-256) storage; inline render for images. | P1 | JTBD-2 |
| FR-N11 | Markdown import and export per note and per notebook; round-trips block types where representable. | P1 | JTBD-2 |
| FR-N12 | Quick capture: global hotkey opens a frameless, always-on-top panel; text lands in today's daily note (or a chosen target) in <2s. | P0 | JTBD-1 |
| FR-N13 | Slash-command menu (`/`) to insert block types and run inline actions within the editor. | P1 | JTBD-1 |
| FR-N14 | Note history/versioning is rebuildable from the `entity_op` log; a crash never loses a keystroke (autosave + journal). | P0 | JTBD-2, JTBD-5 |

### 5.2 Reminders (FR-R)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-R01 | Create a reminder with an absolute fire time stored as UTC `fire_at` + IANA `tz` (DST-safe). | P0 | JTBD-5 |
| FR-R02 | Deliver reminders via native OS notifications with title, body, and rich actions (snooze, complete, open). | P0 | JTBD-5 |
| FR-R03 | Natural-language entry ("remind me tomorrow 3pm", "every weekday 9am") parsed by the hybrid `app-nlp` parser; never invents an unstated date. | P0 | JTBD-1, JTBD-5 |
| FR-R04 | Snooze from the notification or in-app (presets + custom); snooze updates `snoozed_until` and reschedules. | P0 | JTBD-5 |
| FR-R05 | Recurring reminders via RFC-5545 `rrule` with `fixed` vs `after_completion` modes; materialize-on-completion (template + next instance). | P1 | JTBD-5 |
| FR-R06 | Attach a reminder to any target: task, note, note block, or meeting — or standalone (polymorphic target). | P0 | JTBD-5 |
| FR-R07 | Dual-layer scheduling: in-app timer-wheel (authoritative while running) + OS one-shot registration within a rolling 14-day horizon (macOS/Windows). | P0 | JTBD-5 |
| FR-R08 | Missed-reminder catch-up: on launch and wake-from-sleep, sweep `pending AND fire_at < now`, coalesce into one grouped notification, mark `missed`, surface in-app. | P0 | JTBD-5 |
| FR-R09 | De-dup: whichever layer fires first flips `pending→fired`; the other no-ops. | P0 | JTBD-5 |
| FR-R10 | Honest capability reporting: on Linux, disclose that app-closed OS scheduling is unavailable (Layer B absent). | P0 | JTBD-5 |
| FR-R11 | Unlimited reminders per user; edits and cancellations propagate to both layers (cancel `os_handle` on any mutation). | P1 | JTBD-5 |

### 5.3 Tasks (FR-T)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-T01 | Create, edit, complete, and delete tasks with title, notes, checklist, and links. | P0 | JTBD-3, JTBD-5 |
| FR-T02 | Areas → Projects → Headings → Tasks hierarchy; projects may be dated and may back a Note. | P0 | JTBD-5 |
| FR-T03 | Derived views Today / Upcoming / Anytime / Someday computed as queries over `start_on`/`deadline_on`/status — never stored states. | P0 | JTBD-5 |
| FR-T04 | Precise temporal split: `start_on` hides the task until that date; `deadline_on` is due but does not hide; alert time is a separate Reminder. | P0 | JTBD-5 |
| FR-T05 | Checklist items (flat, ordered) and nested subtasks (`parent_task_id`). | P1 | JTBD-5 |
| FR-T06 | Natural-language quick entry for tasks with inline `#project @tag !priority` and live highlighting. | P0 | JTBD-1 |
| FR-T07 | Drag-reorder tasks and projects with O(1) fractional-index updates. | P0 | JTBD-5 |
| FR-T08 | Recurring tasks via `rrule` with `every`/`every!` (fixed vs after-completion) UX; materialize-on-completion. | P1 | JTBD-5 |
| FR-T09 | Tasks link to notes (`about`) and meetings (`spawned_from`); links are first-class and survive edits. | P0 | JTBD-3 |
| FR-T10 | A task can carry a Reminder (`reminds` link) for alerting, independent of its scheduling fields. | P0 | JTBD-5 |
| FR-T11 | Complete/uncomplete with completion timestamp; completed tasks remain searchable and linkable. | P0 | JTBD-5 |

### 5.4 Meeting Intelligence (FR-M)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-M01 | Select an audio source: a specific desktop application's audio and/or the microphone; exclude-self on capture. | P0 | JTBD-3 |
| FR-M02 | Record with an explicit session state machine; pause/resume; never let the LLM own recording state. | P0 | JTBD-3 |
| FR-M03 | Preflight checks (permissions, device availability, disk) before READY; never a silent system-wide fallback. | P0 | JTBD-3 |
| FR-M04 | Two-pass local transcription: live streaming transcript (1–2s latency) + higher-quality final transcript. | P0 | JTBD-3, JTBD-4 |
| FR-M05 | Speaker turns / segmentation producing time-anchored `TranscriptSegment`s (the atomic unit of evidence). | P1 | JTBD-3, JTBD-4 |
| FR-M06 | Generate MeetingArtifactV1 (`executive_summary, topics[], decisions[], action_items[], risks[], open_questions[]`) with GBNF-constrained decoding, one repair, deterministic fallback. | P0 | JTBD-3 |
| FR-M07 | Every artifact fact carries `evidence_segment_ids[]`; owners/dates are populated only if extracted from evidence. | P0 | JTBD-3, JTBD-4 |
| FR-M08 | Jump from any artifact fact to the exact transcript segment (and audio timestamp) that supports it. | P0 | JTBD-3, JTBD-4 |
| FR-M09 | Convert an action item into a Task: create the task, write `spawned_from` link with `evidence_segment_ids`, carry owner→assignee and due→`deadline_on` only if extracted. | P0 | JTBD-3 |
| FR-M10 | A meeting is a note: the artifact and transcript live in the unified store; the meeting appears on the daily note. | P0 | JTBD-2, JTBD-3 |
| FR-M11 | Crash-safe recording: append-only NDJSON session journal enables automatic recovery of a captured session. | P0 | JTBD-3, JTBD-5 |
| FR-M12 | Re-generate the artifact on demand; each generation is immutable-per-generation and provenance-tracked. | P1 | JTBD-3 |
| FR-M13 | Honest per-platform capability report for capture (what each OS supports and its limits). | P0 | JTBD-3 |

### 5.5 Unified Search (FR-S)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-S01 | One search across all four pillars over the universal `chunk`/`entity` spine. | P0 | JTBD-4 |
| FR-S02 | Hybrid retrieval: FTS5 BM25 ∪ sqlite-vec KNN fused by RRF; FTS returns synchronously, embeddings stream in and re-fuse. | P0 | JTBD-4 |
| FR-S03 | First-class filters `type:`, `tag:`, `date:`, `person:`, `is:` compiled to SQL predicates before fusion. | P1 | JTBD-4 |
| FR-S04 | Command palette (Cmd/Ctrl-K), three modes by leading sigil: Go (`[[`/`#`/`@` scoped, fuzzy title), Do (`>` commands), Ask (`?`/NL → RAG). | P0 | JTBD-1, JTBD-4 |
| FR-S05 | Quick-switcher (Go) with recency-boosted fuzzy title + BM25 for instant navigation to any entity. | P0 | JTBD-1 |
| FR-S06 | Search results cite their source entity and, for meeting hits, the transcript segment + timestamp. | P0 | JTBD-4 |
| FR-S07 | Incremental, content-hash-gated, debounced-on-save indexing keeps FTS and vectors current without blocking the editor. | P0 | JTBD-4 |

### 5.6 AI Workspace (FR-A)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-A01 | "Ask your notes": natural-language question → hybrid RAG → grounded, cited **AnswerV1** answer. | P0 | JTBD-4 |
| FR-A02 | Every citation is verified to resolve to a real chunk before display; unresolved citations are never shown. | P0 | JTBD-4 |
| FR-A03 | If no evidence grounds the answer, return `unanswered: true` with an honest "not found in your notes" message. | P0 | JTBD-4 |
| FR-A04 | Answers span all pillars (notes, tasks, reminders, meetings, transcripts) via the unified retrieval path. | P0 | JTBD-4 |
| FR-A05 | Auto-link and auto-tag suggestions are reversible `suggestion` rows (cited, user-approved), generated as idle-time batch jobs — never silent edits. | P1 | JTBD-2, JTBD-4 |
| FR-A06 | Optional bge-reranker stage improves retrieval ordering before grounded decoding. | P2 | JTBD-4 |
| FR-A07 | All AI runs on-device; no prompt, transcript, or note content ever leaves the machine. | P0 | JTBD-4, privacy |

### 5.7 Platform, Onboarding & Models (FR-P)

| ID | Requirement | Priority | Traces |
|----|-------------|----------|--------|
| FR-P01 | Zero-config first launch: default notebook, today's daily note, hardware-appropriate model tier auto-selected; no account/server. | P0 | JTBD-1 |
| FR-P02 | Model manager: signed manifests, SHA-256 checksums, resumable range downloads, disk preflight, offline USB/import. | P0 | privacy |
| FR-P03 | Network is touched only by `model-download` and `updater`, both user-consented; an explicit "Offline Ready" state exists. | P0 | privacy |
| FR-P04 | Two windows + tray: `main` (chrome) and `capture` (frameless, always-on-top, skip-taskbar); dynamic macOS activation policy. | P0 | JTBD-1 |
| FR-P05 | User-rebindable global quick-capture and record hotkeys. | P1 | JTBD-1 |
| FR-P06 | Settings surface: models, hotkeys, autostart (opt-in), and the honest per-platform capability report. | P0 | JTBD-3 |
| FR-P07 | Hardware-tier LLM selection (Tier 1 4B / Tier 2 8B / Tier 3 14B) with user override; STT profile selection (base/small/medium, opt-in Turbo). | P1 | — |

---

## 6. Non-Functional Requirements (NFR)

| ID | Category | Requirement | Target / Acceptance |
|----|----------|-------------|---------------------|
| NFR-01 | Performance — launch | Cold app launch to interactive editor | < 2 s on Tier-2 hardware |
| NFR-02 | Performance — capture | Record start latency (button press → capturing) | < 100 ms |
| NFR-03 | Performance — transcript | Live transcript segment latency | 1–2 s |
| NFR-04 | Performance — search | FTS (keyword) result latency, perceived | < 10 ms; vector results stream in async |
| NFR-05 | Performance — capture | Quick-capture panel open → text committed | < 2 s end-to-end |
| NFR-06 | Performance — memory | Steady-state RAM (typical session, one model resident) | < 3 GB |
| NFR-07 | Performance — editor | Keystroke-to-render in editor | < 16 ms (60 fps), no jank on large notes |
| NFR-08 | Offline | Every core path (write, plan, record, transcribe, reason, search) works with network disabled | 100% of core paths; verified with network off |
| NFR-09 | Offline | Only `model-download` and `updater` may open sockets | Enforced by network-isolation discipline; auditable |
| NFR-10 | Privacy | No telemetry by default | Zero outbound analytics; opt-in only, if ever |
| NFR-11 | Security — at rest | Whole-DB encryption | SQLCipher; key in OS keystore (Keychain / Credential Manager / Secret Service) |
| NFR-12 | Security — boundary | WebView never sees SQL or raw filesystem; PCM audio never crosses WebView or serializes to JSON | Enforced by IPC command surface + capabilities |
| NFR-13 | Security — capture | No silent system-wide audio fallback; capture is app/mic-scoped and exclude-self | Verified per platform |
| NFR-14 | Reliability — crash safety | A crash never loses a keystroke or a recorded second | Append-only NDJSON journals; automatic recovery on relaunch |
| NFR-15 | Reliability — rebuildability | Derived tables (block index, links, FTS, vectors) fully rebuildable from source (`doc_json`, `entity_op`) | Full rebuild passes integrity check |
| NFR-16 | Reliability — recovery | Automatic session/entity recovery after unexpected termination | Recovers to last journaled op on next launch |
| NFR-17 | Accessibility | Full keyboard operability; screen-reader labels; sufficient contrast; respects OS reduced-motion | WCAG 2.1 AA target for primary flows |
| NFR-18 | Cross-platform | Common contract across macOS, Windows, Linux with honest per-platform capability reporting | Feature parity where possible; disclosed gaps otherwise |
| NFR-19 | Data portability | Markdown export + one-file encrypted backup; content-addressed attachments included | User can export/backup without the app running services |
| NFR-20 | Integrity | Model artifacts verified by signed manifest + SHA-256 before use | Reject on checksum/signature mismatch |
| NFR-21 | Observability | Diagnostics and logs available locally without telemetry | Local-only structured logs; user-triggered export |
| NFR-22 | Packaging | Signed installers per platform | macOS DMG + notarize; Windows MSI/EXE Authenticode; Linux AppImage + Flatpak |
| NFR-23 | Scalability — local | Responsive at large personal scale | Smooth at 50k notes / 500k blocks / 1k meetings on Tier-2 hardware |

---

## 7. Success Metrics

Metrics are measured **locally and privately** (no telemetry by default); product-level targets are validated via opt-in research cohorts, beta feedback, and internal dogfooding — never silent data collection.

### 7.1 Activation & Adoption

- **Time-to-first-note:** median < 60 s from first launch (target).
- **Zero-config success:** ≥ 95% of first launches reach the editor with a working model tier and no manual setup.
- **First meeting captured:** ≥ 60% of users who install for meetings complete a successful record→artifact within their first week.

### 7.2 Core-Loop Engagement

- **Quick-capture usage:** median ≥ 5 quick-captures/week among active users.
- **Meeting-to-task conversion:** ≥ 40% of generated action items are accepted as tasks (measures artifact quality + flow).
- **Daily-note return rate:** ≥ 50% of active days open the daily note.

### 7.3 Intelligence Quality

- **Answer groundedness:** ≥ 98% of displayed AI answers have every citation resolve to a real chunk (hard invariant; violations are bugs).
- **Honest refusal rate:** the workspace returns `unanswered` rather than fabricate whenever evidence is absent (0 tolerated hallucinated facts in acceptance testing).
- **Artifact evidence coverage:** 100% of artifact facts carry ≥ 1 `evidence_segment_id`.

### 7.4 Reliability & Trust

- **Zero data-loss:** 0 lost keystrokes / recorded seconds across crash-recovery test suites (hard gate).
- **Crash-free sessions:** ≥ 99.5% of recording sessions complete without an unrecovered failure.
- **Offline correctness:** 100% of core paths pass with the network disabled (hard gate).

### 7.5 Performance Gates

- All NFR-01–NFR-07 targets met on Tier-2 reference hardware as release acceptance gates.

---

## 8. Out-of-Scope / Non-Goals (v1)

These are explicit v1 exclusions (from the Foundation). They are deferred, not rejected forever; several are on the post-v1 track.

- **No multi-device sync, no CRDT engine, no collaboration/sharing, no accounts.** (The op-log/HLC/UUID seam is built now so a later Loro + blind-relay sync is a re-encode, not a re-model.)
- **No mobile app.** Desktop only: macOS, Windows, Linux.
- **No cloud/hosted models or third-party AI APIs.** All inference is on-device.
- **No global force-directed graph as a headline feature.** Only a local neighborhood graph.
- **No plugin/extension SDK, no public API.**
- **No calendar/email integration.** The Person entity exists, but there is no external sync.
- **No web clipper / browser extension.**
- **No real-time collaborative editing.**
- **No relaunch-quit-app-to-fire-reminder OS scheduler integration** beyond the rolling-horizon one-shot layer (a later enhancement).

---

## 9. Assumptions & Dependencies

### 9.1 Assumptions

- Users have sufficient local hardware for at least Tier-1 (4B LLM + whisper base); the model manager auto-selects a tier and degrades gracefully.
- Users are willing to perform a one-time model download (or USB import) during setup; thereafter the app runs fully offline.
- The primary device is the source of truth; users accept single-device operation in v1 and will export/back up for durability.
- Meeting capture requires OS permissions (screen/audio recording, microphone) that the user grants during preflight.
- The desktop OS provides the native capture and notification primitives assumed per platform (see dependencies).

### 9.2 Technical Dependencies (inherited baseline, carried forward)

- **Shell/UI:** Tauri 2 + native WebView; React/TypeScript frontend; Tiptap (ProseMirror) editor.
- **Core:** Rust on Tokio (domain, state machine, media pipeline, storage, orchestration).
- **Capture:** macOS ScreenCaptureKit (Swift), Windows WASAPI process-loopback, Linux PipeWire/WirePlumber (+ portals on Wayland), behind a unified `capture-api` trait.
- **Speech:** whisper.cpp (default) behind `speech-api`; opt-in Parakeet TDT v3 (ONNX) and Apple SpeechTranscriber adapters.
- **LLM:** llama.cpp / GGUF (Qwen3 default), MLX on Apple Silicon, GBNF-constrained structured output.
- **Embeddings:** EmbeddingGemma-300M or bge-base, Matryoshka-truncated to 256 dims + int8.
- **Storage/Search:** SQLite + SQLCipher via `rusqlite`; FTS5 + sqlite-vec inside the single encrypted store; append-only NDJSON journals; content-addressed files.
- **Secrets:** OS keystore (Keychain / Credential Manager / Secret Service).
- **Networked services (only two):** `model-download` and `updater`, both user-consented.

### 9.3 Cross-Document Dependencies

- **Architecture** owns crate decomposition, process/threading, Tauri security posture, and packaging.
- **HLD** owns runtime designs (session state machine, dual-layer scheduler, RAG pipeline, note save→projection flow, media pipeline).
- **Data Model** owns the authoritative SQLite schema and the exact MeetingArtifactV1 / AnswerV1 / ParsedEntry JSON schemas.
- **Feature Specs** owns per-feature behavioral detail.
- **Roadmap** owns sequencing and acceptance gates.

This PRD references those documents and must not contradict them; where scope questions arise, the Foundation is authoritative.

---

## 10. Glossary

- **Pillar** — one of the four product domains: Notes, Reminders, Tasks, Meeting Intelligence.
- **Entity spine** — the thin universal `entity` table (one row per addressable thing) that unifies all pillars.
- **Block** — an addressable node inside a note's doc, carrying a stable `blockId`; target of block-level links, reminders, and evidence anchors.
- **Daily note** — an ordinary note keyed by date; the capture spine that threads quick-captured notes, tasks, and meeting stubs onto a day.
- **Link** — a polymorphic directed edge (wikilink, backlink, mention, tagged, spawned_from, about, attends, action_item_of, reminds, child_of) carrying origin and evidence.
- **MeetingArtifactV1** — the evidence-linked, generated meeting output (summary, topics, decisions, action items, risks, open questions).
- **AnswerV1** — the constrained AI answer object `{answer, citations[], confidence, unanswered}`.
- **RRF** — Reciprocal Rank Fusion, the method fusing FTS and vector rankings without score normalization.
- **Suggestion** — a reversible, cited, user-approved proposed edit (auto-link/auto-tag); never a silent change.
- **Offline Ready** — the first-class state indicating all core paths function with the network disabled.
- **Capability report** — the honest per-platform disclosure of what capture/scheduling features are supported.

---

*End of PRD. This document is subordinate to the Design Foundation and coordinated with the PRD's sibling documents (Architecture, HLD, Data Model, Feature Specs, Roadmap).*

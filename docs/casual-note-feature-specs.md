# Casual Note — Detailed Feature Specifications

*Behavior-level specs (UX, rules, edge cases, acceptance criteria) for every user-facing surface. Consistent with the Design Foundation §3–4 and the inherited EchoNote baseline. This document owns per-feature behavior; it does not redefine schema (see Data Model) or architecture (see Architecture/HLD).*

---

## 0. Conventions

- **Platform keys.** `Cmd` = macOS ⌘; on Windows/Linux read as `Ctrl`. `Opt` = ⌥/Alt.
- **Contract blocks.** Each feature states **Preconditions → Behavior → States → Edge cases → Acceptance criteria (AC-N)**. AC numbers are per-section and testable.
- **"Local-only" guarantee.** Every flow below completes with the network disabled. Any surface that could imply otherwise must render the honest capability state (see §9).
- **Provenance rule.** No AI-produced fact (tag, link, answer, action item) is written without a resolved evidence citation. This is invariant across §5–§7.

---

## 1. Block / Markdown Editor

The editor is Tiptap (ProseMirror) editing `note.doc_json`. The WebView edits JSON only; the Rust `notes` crate owns block-index projection, link extraction, FTS. Every block node carries a stable `blockId` in `attrs`, minted on creation and never reused.

### 1.1 Block types & slash menu

Trigger the block menu with `/` at the start of an empty block or after a space. Fuzzy-filter by typed query; `Esc` dismisses; `Enter`/click inserts.

| Block | Slash aliases | Markdown shortcut | Notes |
|---|---|---|---|
| Paragraph | `text`, `p` | — | Default node |
| Heading 1–3 | `h1`,`h2`,`h3` | `#`,`##`,`###`+space | Outline + chunk boundary (§6) |
| To-do | `todo`,`task`,`[]` | `[]`+space | Checkbox block; can promote to Task (§1.6) |
| Bulleted / Numbered list | `bullet`,`number` | `-`/`1.`+space | Nestable via Tab/Shift-Tab |
| Code block | `code` | ` ``` `+lang | Language attr; monospace; no spellcheck |
| Table | `table` | — | Insert N×M; tab-navigation; add/remove row/col |
| Callout | `callout`,`note`,`warn` | `> [!type]` | Typed (info/warn/success); collapsible |
| Quote | `quote` | `>`+space | — |
| Divider | `divider`,`---` | `---`+enter | Thematic break |
| Attachment / Image | `image`,`file` | paste/drop | Content-addressed (§1.5) |
| Embed | `embed` | `![[note]]` | Transcludes another note/block read-only |
| Transcript-segment | (system) | — | Inserted by meeting flow (§5); time-anchored, read-only body, editable speaker label |

**Edge cases.** Slash inside a code block is literal (no menu). Converting a populated block preserves inline text where the target supports it; converting to Divider requires the block be empty or prompts to insert-below.

**AC-1.1a** `/` in an empty paragraph opens the menu within 50 ms; arrow keys + Enter insert the highlighted block.
**AC-1.1b** Markdown shortcuts transform in place without a network call and are undoable with a single `Cmd-Z`.
**AC-1.1c** Every inserted block has a unique 22-char `blockId`; deleting and re-inserting yields a new id.

### 1.2 `[[Wiki-links]]` & backlinks panel

Typing `[[` opens an inline autocomplete over note titles (fuzzy + BM25, recency-boosted). Selecting inserts a `wikilink` inline node bound to the target `entity_id`. `[[Title#^blockId]]` targets a block; `[[Title|alias]]` sets display text.

- **Non-existent target.** Confirming a title with no match creates a *placeholder* link; the note is materialized on first visit ("Create *Title*"). Placeholders render dashed.
- **Backlinks panel.** Docked per note; two sections: **Linked mentions** (resolved `[[...]]` and `@` targeting this note) and **Unlinked mentions** (plain-text occurrences of the title, resolved by the `links` crate on read). Each row shows the source note, the surrounding block snippet, and a jump affordance. "Link" on an unlinked row rewrites the source block to a real wikilink.
- **Bidirectionality is derived on read** — never dual-written (Foundation §3).

**AC-1.2a** Creating `[[Foo]]` in note A makes A appear under Foo's Linked mentions immediately after save-projection.
**AC-1.2b** Renaming a note updates all inbound wikilink display text (alias-preserving) in one transaction; block anchors survive.
**AC-1.2c** An unlinked mention promoted to a link disappears from Unlinked and appears in Linked without a reindex stall > 200 ms.

### 1.3 Tags

`#` opens tag autocomplete (existing tags + "create"). A tag is `link(rel='tagged')` to a first-class Tag entity. A tag carrying `schema_json` (supertag-lite) lends optional typed fields to the tagged note, surfaced as an editable property strip at the top of the note.

**Edge cases.** Nested tags `#area/subarea` create a hierarchy via `child_of` links. Deleting a tag entity offers "remove tag from N notes" (keeps notes) vs "delete only if unused". Renaming a tag updates all `tagged` edges, not note text, unless the tag is inline in prose (then the inline token is rewritten).

**AC-1.3a** `#` filtering is case-insensitive; committing an unknown tag creates the Tag entity + edge atomically.
**AC-1.3b** A supertag's schema fields render as a property strip and persist to the note's typed detail, not into `doc_json` prose.

### 1.4 Daily notes

The daily note is the capture spine. Opening "Today" (`Cmd-D`) creates-or-opens the note keyed by `entity.daily_date = <local today>`. A configurable template seeds the body. Previous/next-day arrows and a mini-calendar navigate; empty past days are created lazily on visit.

**Edge cases.** Timezone/day-rollover is computed from the OS local date; a note started at 23:59 stays on its date. Quick-captured items (§2) with no explicit date thread onto today's daily note via `daily_date`.

**AC-1.4a** `Cmd-D` reaches Today in < 150 ms and focuses the first empty block.
**AC-1.4b** Crossing local midnight while the app is open updates "Today" target on next open without losing the prior day's unsaved edits.

### 1.5 Attachments

Paste or drop files/images into a block. Files are hashed (SHA-256), stored content-addressed under the scoped attachments dir (Tauri `fs` scope), and referenced by hash — never copied into `doc_json`. Images render inline with resize handles; other files render as a chip with name, size, and "Reveal/Open".

**Edge cases.** Identical bytes dropped twice dedupe to one stored blob (two references). Deleting the last reference schedules the blob for GC (idle sweep); references elsewhere keep it. Oversized files warn but do not block (local disk only).

**AC-1.5a** A pasted image appears optimistically and finalizes to its content-addressed path after hashing, with no raw bytes serialized through IPC as JSON.
**AC-1.5b** Duplicate content stores exactly one blob; reference count is correct after add/remove.

### 1.6 Checklists & to-do blocks

A to-do block is an in-note checkbox. Toggling sets done/undone with strikethrough. A to-do can be **promoted to a Task** (`Cmd-Shift-T` on the block, or from the block handle): this creates a `task` entity, writes `link(rel='about')` back to the block, and keeps the checkbox state mirrored bidirectionally (block toggle ↔ task completion) via the block↔task link. Un-promoted to-dos remain purely in-note.

**AC-1.6a** Promoting a to-do creates one Task and one `about` link; completing either surface reflects in the other within one save cycle.
**AC-1.6b** Deleting the note block offers to keep-or-delete the linked Task (default keep).

---

## 2. Quick Capture

A frameless, always-on-top `capture` window (created at startup, `skipTaskbar`) summoned by a global, user-rebindable hotkey (default `Cmd-Shift-Space`). Single text field with live NL highlighting; `Enter` commits, `Esc` dismisses without saving, `Cmd-Enter` commits and opens the created item.

### 2.1 Routing: note vs task vs reminder

The `app-nlp` hybrid parser tokenizes input into a `ParsedEntry` and chooses a **route**:

```
INPUT ── tokenize (grammar/regex, 90% path) ──► ParsedEntry{ intent, title, date?, time?,
                                                             project?, tags[], priority?, rrule? }
                │ low confidence
                └──► resident Qwen3 (schema-constrained) ──► ParsedEntry
ROUTE RULES (first match wins):
  has fire time/"remind me"      → REMINDER (standalone unless target given)
  starts with verb + has date    → TASK (deadline/start from date semantics)
  has #project @ or "todo/task"  → TASK
  otherwise                      → NOTE (appended to today's daily note)
```

A route pill (Note / Task / Reminder) shows the decision and is **manually overridable** before commit. Parsed tokens (dates, `#project`, `@tag`, `!priority`, `every[!]`) are highlighted inline; the resolved absolute datetime is shown as a ghost hint ("→ Fri Jul 24, 3:00 PM").

### 2.2 Parsed-string examples

| Input | Route | Parsed result |
|---|---|---|
| `Buy milk` | Note | Appended as a bullet to today's daily note |
| `todo Draft Q3 deck #Work !2 friday` | Task | title=Draft Q3 deck, project=Work, priority=2, `deadline_on`=Fri |
| `remind me tomorrow 3pm to call Sam` | Reminder | `fire_at`=tomorrow 15:00 local, title="call Sam" |
| `Review PRs every weekday 9am` | Reminder | rrule=`FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR`, time 09:00 |
| `Water plants every! 3 days` | Task+Reminder | after-completion recurrence (§4.2), 3-day interval |
| `Pay rent every month on the 1st !1` | Task | rrule monthly BYMONTHDAY=1, priority=1 |

**Never invent a date the user didn't state.** If no date is present, none is set (task lands in Anytime, §3).

**Edge cases.** Ambiguous "next Friday" resolves via a documented calendar rule (the coming Friday; "next" past a nearby Friday jumps a week — surfaced in the ghost hint so the user can correct). Multiple `#project` tokens: first is the project, rest are tags. Parser confidence below threshold that the LLM also cannot resolve routes to Note (safest, never loses text).

**AC-2.1** The hotkey shows the panel in < 120 ms from any app; the field is focused.
**AC-2.2** Every example in §2.2 parses to the stated route and fields offline via the grammar path (no LLM required).
**AC-2.3** Committing a Reminder registers it in the scheduler (§4) atomically with the DB write; a crash immediately after commit still fires it.
**AC-2.4** Overriding the route pill re-runs field mapping for the chosen intent without re-typing.

---

## 3. Tasks

Things-style derived-view model: **buckets are queries over fields, not stored states** (Foundation §4). Ordering by fractional index (`order_key`) for O(1) drag-reorder.

### 3.1 Buckets (query definitions)

| Bucket | Definition |
|---|---|
| **Today** | `status=open AND (start_on ≤ today OR deadline_on ≤ today) AND not(someday)`; plus manually "starred for today" |
| **Upcoming** | `status=open AND start_on > today` (or `deadline_on > today` with no start), grouped by date + calendar-projected recurrences |
| **Anytime** | `status=open AND start_on IS NULL AND not(someday)` — actionable, undated |
| **Someday** | `status=open AND someday=true` — hidden from Today/Upcoming until activated |
| **Logbook** | `status IN (completed, canceled)`, reverse-chronological by completion time |

**Key semantic split (Foundation §4):** `start_on` (**When/scheduled — *hides* the task** until that date), `deadline_on` (**due — does *not* hide**, shows a deadline flag/countdown), and a separate **Reminder** (§4) for an alert time. These are three distinct fields; UI never conflates them.

### 3.2 Areas, Projects, Headings

- **Area** — top-level life bucket ("Work", "Home"); contains projects and loose tasks.
- **Project** — belongs to an area; may itself be dated; can back a Note (project notes). Shows a progress ring (done/total). Completing a project offers to complete/keep open sub-tasks.
- **Heading** — lightweight in-project section to group tasks; drag tasks between headings.

### 3.3 Task detail & subtasks

A task carries: title, notes, `start_on`, `deadline_on`, project/area, `checklist_item[]` (flat ordered), `parent_task_id` (nested subtasks), tags, priority, links (`about`/`spawned_from`). Checklist items are lightweight in-task steps; subtasks are full tasks with their own dates. Completing a parent does not auto-complete subtasks unless configured; UI warns if open subtasks remain.

### 3.4 Drag-reorder & scheduling gestures

Dragging within a bucket rewrites only the moved item's `order_key` (fractional midpoint) — O(1), no bulk renumber. Dragging onto a date in the mini-calendar sets `start_on`; onto "Today" stars for today; onto a project moves it. Right-click / swipe exposes: Today, This Evening, Tomorrow, Someday, Add Reminder, Move, Complete, Cancel.

### 3.5 Logbook & completion

Completing a task (checkbox) animates it out after a short grace window (undo toast) and records completion time → Logbook. Canceling is distinct from completing (records `canceled`). Recurring tasks on completion **materialize the next instance** (§4.2), not pre-expanded.

**AC-3.1** A task with only `start_on = next Monday` is absent from Today/Anytime and appears in Upcoming under Monday; on Monday it moves to Today automatically.
**AC-3.2** A task with `deadline_on = today` and no `start_on` shows in Today **and** in Anytime-eligibility is overridden by the deadline; the deadline flag renders.
**AC-3.3** Dragging a task to a new position updates exactly one row's `order_key`; list order is stable after reload.
**AC-3.4** Completing a task moves it to Logbook with a working undo within the grace window; recurrence spawns exactly one next instance.
**AC-3.5** All bucket queries return correct membership offline and update reactively on field edits.

---

## 4. Reminders

First-class polymorphic entity (target = task | note | meeting | standalone). Absolute `fire_at` (UTC) + IANA `tz` (DST-safe), optional `rrule`, `state ∈ {pending, fired, snoozed, missed, dismissed, canceled}`, `snoozed_until`, `os_handle`.

### 4.1 Creation

From Quick Capture NL (§2), from a task/note/meeting ("Add reminder"), or the Reminders view. Absolute time is stored UTC with the origin IANA tz so DST shifts are correct. A reminder attached to an entity writes `link(rel='reminds')` + the polymorphic `reminder.target_*`.

### 4.2 Recurrence (RRULE)

RFC-5545 RRULE via the `rrule` crate, with `mode`:
- **`fixed` (Todoist `every`)** — next instance = next RRULE occurrence after the *scheduled* time, regardless of when completed. ("every day at 9am".)
- **`after_completion` (Todoist `every!`)** — next instance materialized as *completion time + interval*. ("every! 3 days" from when you actually did it.)

**Materialize-on-completion** (template + `next_scheduled_on`, never pre-expanded). `until`/`count` bound the series; `complete_instances[]` tracks history. Skipping an occurrence advances without firing a catch-up.

### 4.3 Notification delivery — dual-layer scheduler

```
                 SQLite (durable truth: reminders + state)
                          │  rebuilt on launch
   Layer A ──► Tokio timer-wheel / min-heap keyed on fire_at
   (running)     owns snooze, edit, rich actions, unlimited count, recurrence advance
   Layer B ──► OS one-shot within rolling 14-day horizon
   (app-closed)  UNCalendarNotificationTrigger (macOS) / ScheduledToastNotification (Win)
                 stores os_handle; CANCELLED on any mutation
   Linux ──► NO OS layer → reported honestly (fires only while running)
De-dup: delivery gated on reminder.state; first layer to fire flips pending→fired, other no-ops.
```

Notifications carry rich actions: **Complete**, **Snooze** (5m/1h/tomorrow/custom), **Open**. Snooze sets `state=snoozed`, `snoozed_until`, re-arms both layers.

### 4.4 Missed-reminder catch-up

On launch **and** on wake-from-sleep, sweep `state='pending' AND fire_at < now`, coalesce into **one grouped notification** ("3 reminders while you were away"), mark each `missed`, and surface an in-app **Missed** tray with per-item Complete/Reschedule/Dismiss.

**Edge cases.** Clock change / travel across tz: `fire_at` is absolute UTC so wall-clock display shifts but firing instant is stable. A reminder edited while snoozed cancels the OS handle and re-registers. Deleting a reminder cancels both layers.

**AC-4.1** A reminder set while the app is open fires within ±2 s of `fire_at` via Layer A.
**AC-4.2** Quitting the app after setting a reminder < 14 days out still fires it via Layer B (macOS/Windows); on Linux the capability report states it will not.
**AC-4.3** Two layers never double-notify: the de-dup gate is observed under a forced race.
**AC-4.4** After 5 missed reminders during downtime, exactly one grouped notification appears on launch and all 5 show in the Missed tray marked `missed`.
**AC-4.5** DST boundary: a 9:00 AM daily reminder fires at local 9:00 AM on both sides of the transition.

---

## 5. Meeting Intelligence

Inherited EchoNote pipeline, carried forward and unified. Session state machine: `NEW→PREFLIGHT→READY→RECORDING↔PAUSED→STOPPING→CAPTURED→FINAL_TRANSCRIBING→GENERATING→INDEXING→COMPLETE` (+ DEGRADED/FAILED/RECOVERING). The LLM never owns recording state.

### 5.1 Source picker (PREFLIGHT/READY)

Enumerate capturable app-audio sources + microphones via the native adapter (ScreenCaptureKit / WASAPI process-loopback / PipeWire). User selects app-audio and/or mic; **exclude-self** is default so the app's own output isn't captured. PREFLIGHT checks permissions, model presence (STT tier), and disk headroom, reporting each honestly (no silent system-wide fallback on Windows).

### 5.2 Live transcript (RECORDING)

Live-pass whisper.cpp (base) streams partial segments to a scrolling transcript with 1–2 s latency; speaker turns are grouped. User can pause/resume, drop a **marker** (timestamped bookmark), and type synchronized manual notes alongside. Raw PCM never crosses the WebView; only rendered `TranscriptSegment`s do.

### 5.3 Review (CAPTURED → FINAL_TRANSCRIBING)

On stop, a final higher-accuracy pass (small/medium) re-transcribes for the artifact. Review UI: transcript with timestamps, editable speaker labels, search-within, and audio playback that scrubs to any segment. Segments are the atomic unit of evidence.

### 5.4 Artifacts (GENERATING)

Local LLM (Qwen3, GBNF-constrained) produces **MeetingArtifactV1**: `executive_summary, topics[], decisions[], action_items[], risks[], open_questions[]`. **Every fact carries `evidence_segment_ids[]`**; the model must not invent owners or dates. One schema-repair attempt, then deterministic fallback. Each artifact element links to its transcript segment(s); clicking jumps + plays.

### 5.5 Action-items → Tasks (INDEXING)

Each `action_item` renders with a checkbox to **promote to Task**. On promote (Foundation §3):
```
create entity(kind=task) + task detail
link(src=task, dst=meeting, rel='spawned_from', evidence_segment_ids=[…])
if discussed in a note block → link(src=task, dst=block, rel='about')
carry owner→assignee, due_date→deadline_on  ONLY if extracted from evidence
```
The task then shows "From meeting *Q3 Planning* (00:14:22) → jump to evidence"; the meeting shows "N action items → tasks". INDEXING also writes the meeting note, chunks, links, and vectors into the unified spine so the meeting is searchable and answerable (§6–§7).

**Edge cases.** RECORDING→DEGRADED on device loss keeps buffering to the NDJSON journal and surfaces a banner; STOPPING always flushes. A crash mid-session recovers from the journal into RECOVERING and resumes at CAPTURED. Empty/near-silent audio yields an artifact with `open_questions` rather than fabricated content.

**AC-5.1** Source picker lists real app-audio sources and mics; excluding self is default and verified (app output absent from capture).
**AC-5.2** Live transcript renders first partials within 2 s of speech; markers land at correct timestamps.
**AC-5.3** Every artifact fact resolves to at least one real `TranscriptSegment`; an owner/date absent from transcript is never populated.
**AC-5.4** Promoting an action item creates one Task with a `spawned_from` edge carrying `evidence_segment_ids`; the jump-to-evidence link scrubs audio to the cited segment.
**AC-5.5** A forced crash during RECORDING recovers all captured seconds from the journal (no lost audio) and completes the pipeline.

---

## 6. AI Workspace

`ai-workspace`: retrieve (hybrid + RRF, optional bge-reranker) → grounded prompt with numbered evidence → constrained-decode **AnswerV1** `{answer, citations[], confidence, unanswered}` → **verify every citation resolves** before display.

### 6.1 Ask-your-notes (RAG)

```
QUESTION ──► retrieve: FTS5 (BM25) ∪ sqlite-vec KNN ──► RRF fuse ──► [optional bge-rerank]
        ──► grounded prompt (numbered evidence chunks) ──► GBNF-constrained AnswerV1
        ──► CITATION VERIFY: each citation → real chunk?
              all resolve → render answer with inline [1][2] evidence chips
              none resolve → return unanswered:true ("I couldn't find this in your notes")
```

Chunking: notes by heading/block (~200–400 tokens, breadcrumb-carried); transcripts by VAD/speaker turn (~30–60 s, time-anchored); tasks/reminders one chunk each. Every citation chip jumps to its source (note block or transcript timestamp). The workspace spans all four pillars.

### 6.2 Summarize

Summarize a note, a meeting, a project, or a date range. Output is grounded and cited the same way; a summary of a meeting reuses/refs the MeetingArtifactV1 rather than re-deriving.

### 6.3 Auto-tag / auto-link

Run as **idle-time batch jobs** producing reversible **`suggestion` rows** (cited, user-approved) — **never silent edits** (Foundation §4). A review surface lists proposed tags/links with the evidence that justifies each; the user Accepts (materializes the edge/tag) or Dismisses (tombstones the suggestion). Content-hash-gated, debounced-on-save embedding keeps the index incremental.

**Edge cases.** If retrieval returns nothing above threshold, Ask returns `unanswered:true` — it never fabricates. A citation that fails to resolve invalidates the whole answer (fail-closed). Suggestions are idempotent per content hash (no duplicate proposals).

**AC-6.1** Ask returns a cited answer offline; every citation chip resolves to a real chunk and jumps correctly.
**AC-6.2** A question with no supporting content yields the honest "couldn't find this" state, not a hallucinated answer.
**AC-6.3** Auto-tag/auto-link never mutate data without explicit acceptance; every suggestion carries visible evidence; Dismiss is permanent per hash.
**AC-6.4** FTS results render synchronously (< 10 ms class) while embeddings stream in and re-fuse without blocking the answer path.

---

## 7. Unified Search & Command Palette

**FTS5 (BM25) ∪ sqlite-vec KNN, fused by Reciprocal Rank Fusion** over the universal `chunk`/`entity` spine across all four pillars. FTS returns synchronously; embeddings stream and re-fuse. First-class filters compile to SQL predicates *before* fusion.

### 7.1 One palette (`Cmd/Ctrl-K`), three modes

| Mode | Sigil | Behavior |
|---|---|---|
| **Go** | (none) | Quick-switcher: fuzzy title + BM25, recency-boosted; jumps to any entity |
| **Do** | `>` | Command runner (create note, start meeting, toggle theme, open settings…) |
| **Ask** | `?` / NL | Hybrid RAG → cited AnswerV1 (§6) |
| Scoped entity | `#` `@` `[[` | Constrained search over tags / people / notes |

Mode is chosen by leading sigil; with no sigil, Go is default and an NL-looking query offers an "Ask instead" affordance.

### 7.2 Filters

`type:` (note/task/reminder/meeting), `tag:`, `date:` (ranges, `today`, `overdue`), `person:`, `is:` (`is:open`, `is:done`, `is:missed`). Filters combine (AND) and compile to predicates applied before RRF, so results stay ranked within the filtered set.

**Edge cases.** Empty query in Go shows recents. A filter with no matches shows an explicit empty state (not a spinner). Very large result sets paginate/virtualize; ranking is stable across pages.

**AC-7.1** `Cmd-K` opens instantly; typing yields Go results (title + BM25) within one frame class (< 16 ms perceived) from FTS.
**AC-7.2** `type:task is:open tag:Work` returns exactly the matching open tasks, ranked, offline.
**AC-7.3** Switching a query to Ask (`?`) reuses the same retrieval spine and returns a cited answer.
**AC-7.4** All four pillars are reachable from one palette; a meeting, a task, a reminder, and a note all appear for a shared term.

---

## 8. Export & Data Portability

The user's data is theirs; every store is exportable to open formats offline.

### 8.1 Note export

- **Markdown** — `doc_json → Markdown` via the `notes` crate. Wikilinks export as `[[Title]]` (Obsidian-compatible) or as relative `.md` links (option). Attachments export as files next to the note with relative references. Tags export as `#tag`. Transcript-segment blocks export as timestamped blockquotes.
- **HTML** / **PDF** — rendered from the same projection for sharing/printing.
- Round-trip: Markdown **import** re-parses to `doc_json`, minting fresh `blockId`s; import is a feature, not the storage format.

### 8.2 Structured export

- **Tasks/Reminders** — JSON (full fidelity: dates, rrule, links) and iCalendar (`.ics`) for reminders/recurrences (one-way out).
- **Meetings** — MeetingArtifactV1 as JSON + Markdown; transcript as timestamped text/`.srt`/`.vtt`; original audio (content-addressed) copied out.
- **Whole-vault backup** — a single encrypted archive: the SQLCipher DB + content-addressed files + a manifest. Because attachments/audio are content-addressed and the DB is one file, backup is a one-file (plus blob dir) copy.

### 8.3 Scope & guarantees

Export selection: a note, a notebook/folder tree, a project, a date range, or everything. All export runs locally with no network. Exports are deterministic given the same source. Import never overwrites silently — collisions create new entities (fresh UUIDs) or prompt to merge.

**AC-8.1** A notebook exports to Markdown with resolvable wikilinks and co-located attachments; re-importing yields equivalent notes with new blockIds.
**AC-8.2** Reminders export to a valid `.ics` that opens in a standard calendar app; recurrence rules survive.
**AC-8.3** A meeting exports artifact + transcript (`.srt`/`.vtt`) + audio; artifact citations map to transcript timestamps.
**AC-8.4** A whole-vault backup restores to a working store on a clean install with no data loss and no network access.

---

## 9. Capability Honesty (cross-cutting)

Every surface that depends on a platform capability renders the true state, never a false affirmative:
- **Reminders Layer B** — Linux shows "fires only while the app is running"; macOS/Windows show the 14-day OS-scheduled horizon.
- **Capture** — Windows never claims system-wide loopback when only process-loopback is available; missing permissions are shown, not silently degraded.
- **Models** — an absent STT/LLM/embedder tier shows "download required" and the affected features degrade explicitly (e.g., Ask unavailable until an LLM is present) rather than failing opaquely.
- **Offline Ready** — a first-class indicator confirms all core paths (write, plan, record, transcribe, reason, search) work with the network disabled.

**AC-9.1** Disabling the network leaves write/plan/record/transcribe/search/ask fully functional; the only affected surfaces are model-download and updater, which state their offline status.
**AC-9.2** On Linux, the reminder UI never promises app-closed delivery; on Windows, capture never promises system-wide loopback.

---

*End of Feature Specifications. Cross-references: domain entities and canonical decisions per Design Foundation §3–4; schema per Data Model; subsystem runtime designs per HLD; system decomposition per Architecture.*

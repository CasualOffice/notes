# Casual Note — Deep Research Dossier & Competitive Analysis

**Status:** Research synthesis. Feeds the PRD, Architecture, HLD, Data Model, Feature Specs, and Roadmap. Compiles six domain research briefs (2025–2026, web-grounded) into one reference, cross-checked against the canonical *Casual Note — Design Foundation*.

**Scope:** How leading products and libraries actually build the pieces of a fully-local, privacy-first notebook that unifies **notes, tasks, reminders, and meeting intelligence** — and what Casual Note should adopt, defer, or reject.

---

## 1. Executive Summary

Casual Note extends the inherited EchoNote meeting-intelligence baseline (Tauri 2 + Rust core + native audio capture + whisper.cpp/llama.cpp + encrypted SQLite) outward into a full four-pillar notebook. The research across six domains converges on a consistent set of choices, and — importantly — they reinforce rather than fight the existing baseline.

**The ten load-bearing findings:**

1. **Note storage: JSON-doc-as-truth, not files, not one-row-per-block.** The whole PKM field is converging on **SQLite as source of truth** with derived indexes on top — Logseq's 2025 migration off Markdown files to a SQLite DB backend is the tell-tale signal. Store each note as a single ProseMirror/Tiptap JSON blob; project block/link/FTS indexes from it. Markdown is an import/export *feature*, not the substrate.

2. **Editor: Tiptap (ProseMirror).** The 2026 default for this product class. Its document is a JSON node tree that mirrors the storage model 1:1, ships a JSON Schema for validation (matching the baseline's structured-output discipline), and supports the custom nodes Casual Note needs (task block, callout, `[[wikilink]]`, `#tag`, `@mention`, transcript-segment).

3. **Task/reminder semantics are the real differentiator, not features.** Three temporal concepts must stay distinct — **scheduled/start (hides), deadline/due (does not hide), reminder/alert (fires a notification)**. Things' four buckets (Today/Upcoming/Anytime/Someday) are *derived queries over fields*, not stored states. Copy that model.

4. **Reminders must be a first-class polymorphic entity**, not a task column — they attach to tasks, notes, meetings, or stand alone.

5. **Desktop notification scheduling is yours to own.** No cross-platform OS layer provides durable, editable, recurring closed-app scheduling. Ship a **dual-layer scheduler**: an authoritative in-process Tokio timer-wheel plus one-shot OS handoff (macOS UNCalendar / Windows ScheduledToast) for closed-app delivery, with a launch/wake catch-up sweep. Linux has no OS layer — report honestly.

6. **v1 is local-only, single-device, no CRDT, no sync** — but the future-sync seam is cheap insurance: **stable UUIDv7/ULID + HLC + an append-only op-log** with SQLite tables as a materialized projection. This is the single highest-leverage cheap-now decision. Loro (Rust-native movable-tree CRDT) is the intended later note-body engine; a central blind relay with compress-then-encrypt is the intended sync transport.

7. **On-device AI stays inside the one encrypted store.** whisper.cpp remains the portable STT default (Parakeet TDT v3 an opt-in English "Turbo" ONNX adapter); Qwen3 via llama.cpp/GGUF the LLM default with GBNF-grammar-constrained JSON; **sqlite-vec inside SQLCipher** the vector index (no FAISS/usearch — preserves the single encrypted store and one-file backup).

8. **Search: hybrid FTS5 (BM25) ∪ sqlite-vec (KNN) fused by Reciprocal Rank Fusion.** The 2025 SOTA local recipe (Alex Garcia / Simon Willison). No score normalization; merge by rank position. FTS returns synchronously; embeddings stream in and re-fuse.

9. **One universal `entity` spine + one polymorphic `link` table** unifies linking, backlinks, search, and the graph across all four pillars while per-type detail tables keep strong typing. Bidirectionality is derived on read, never dual-written.

10. **Tauri 2 covers the shell with first-party plugins**, but keep whisper/llama **in-process** (not sidecars — avoids notarization pitfalls) and keep storage on direct **`rusqlite` + SQLCipher** (not `tauri-plugin-sql` — the WebView never sees SQL). Two windows + tray with dynamic macOS activation policy.

**The through-line:** every domain independently points at *SQLite-as-truth, Rust-owns-semantics, WebView-is-a-thin-view, evidence-cited AI, offline-by-default*. Casual Note's job is to extend the meeting-intelligence schema outward into a general entity graph, not to rewrite anything.

---

## 2. Notebook Document Model & Block-Editor Architecture

### 2.1 Landscape — three document-model philosophies

The choice of document model cascades into editor, sync, search, and backlinks. Production apps split into four camps:

| Philosophy | Exemplars | How it works | Win | Cost |
|---|---|---|---|---|
| **Server/DB block-tree** | Notion, Craft, AppFlowy, AFFiNE | Every block is a UUID'd row; a page is a block whose children are blocks. Notion row = `id`, `type`, `properties` (JSON bag), `content` (ordered array of child IDs), `parent` (upward pointer). Child order is *implicit in the content array*. | Granular reuse, cheap block-level sync, lossless block transforms | A "document" is scattered across N rows; tree reassembled on read; plain-text portability lost |
| **Markdown-file-as-truth** | Obsidian, Bear, Reflect | Notes are `.md` files; app layers a disposable index (search, backlinks, tags) on top. Reflect-open is explicitly "local-first, Markdown source of truth." | Durability, user ownership, trivial interop, git-friendly | No stable block identity (can't address a paragraph); file-level not block-level sync; structured queries re-parse files |
| **Outliner block-over-MD/DB** | Logseq | Every bullet is an addressable block with an ID; historically serialized to (lossy) Markdown. **Actively migrating to a SQLite DB backend** for identity, properties, and query performance. | Block identity + query speed | The migration itself proves MD files and block semantics are in tension |
| **CRDT-native local-first** | Anytype (`any-sync`/libp2p), AppFlowy (`AppFlowy-Collab`, Yjs-compatible), AFFiNE (Yjs-family) | Document is a CRDT; offline multi-device merge "for free." | Conflict-free concurrent merge | Significant complexity (memory, GC, format migration) for a payoff single-user Casual Note doesn't need yet |

**Industry signal:** Notion runs sharded Postgres over 200B+ blocks; Craft caps ~185k blocks/space; **Logseq's move to SQLite is the field converging on DB-as-truth** whenever you need both identity and speed.

### 2.2 Editor libraries (the WebView layer)

Four real contenders, plus one notable wrapper:

- **ProseMirror** — battle-tested, schema-constrained model; the substrate the others wrap. Invalid docs are *rejected* (a reliability feature). Document = JSON tree: root `doc` with `content: 'block+'`; each node has `type`, optional `attrs`, children; only `text` nodes carry strings; marks handle inline styling. Ships a JSON Schema for validation.
- **Tiptap** — the ergonomic React-friendly wrapper over ProseMirror; Yjs collab built in; **recommended 2026 default for docs/KB apps.**
- **Lexical** (Meta) — lean, plugin-oriented, best at extreme scale/mobile; imperative extension is easier but you'd rebuild collab/serialization plumbing.
- **Slate** — invent-your-own model; most bespoke work.
- **BlockNote** — a Notion-style block editor built *on top of* Tiptap/ProseMirror.

### 2.3 Backlinks, mentions & search (universal patterns)

- **Backlinks/mentions:** parse `[[wiki-link]]` / `#tag` / `@mention` tokens at save time → write rows into a **link/edge table** → render a page's backlinks by querying inbound edges. A derived index, always rebuildable from source.
- **Unlinked references:** Logseq/Obsidian surface full-text matches of a page title where no explicit link exists, with one-click "linkify."
- **Search:** SQLite **FTS5** (inverted index, BM25 ranking, boolean/phrase, external-content tables so source text isn't duplicated) is the near-universal local choice, optionally paired with a semantic embedding index — exactly the inherited baseline.

### 2.4 Key trade-offs

- **Block identity vs. portability.** Stable block IDs (needed for `[[note#^block]]` refs, task↔block links, reminders-on-a-block, evidence anchoring) fight clean Markdown. You can't have vanilla `.md` *and* durable block addresses.
- **Reassembly cost.** Block-per-row = a query per page to rebuild the tree; document-blob = one read but loses block-level query/sync granularity.
- **CRDT now vs. later.** Casual Note is single-user, no realtime collab — a full CRDT engine's main payoff (concurrent merge) isn't needed yet, but retrofitting is painful, so the model *shape* should stay CRDT-friendly.
- **Editor lock-in.** ProseMirror's strict schema is a reliability feature but authoring custom nodes is real work; Lexical is easier to extend but sacrifices the PM↔storage symmetry.
- **Search freshness.** External-content FTS5 avoids double storage but demands disciplined triggers on every write.

### 2.5 Recommended model — hybrid "JSON-doc-as-truth + derived projections"

Note = one row holding a Tiptap JSON document blob (fast read/write, native to the editor), *plus* derived block-index and link rows for addressing, backlinks, tasks, and search. The "Markdown-file-as-truth minus the file" pattern — JSON doc is truth, everything else is a rebuildable projection.

```sql
note(
  id TEXT PRIMARY KEY,            -- UUIDv7 (sortable)
  title TEXT,
  notebook_id TEXT,              -- folder/notebook FK (adjacency-list tree)
  doc_json BLOB,                 -- ProseMirror/Tiptap JSON (or CRDT update log later)
  doc_schema_version INTEGER,
  is_daily INTEGER, daily_date TEXT,  -- daily notes are just notes w/ a date key
  created_at, updated_at, deleted_at  -- soft delete
)
notebook(id, parent_id, name, position, ...)  -- nested folders/notebooks

block(                            -- derived: one row per addressable block
  id TEXT PRIMARY KEY,           -- stable block id, lives inside node attrs.blockId
  note_id TEXT, parent_block_id TEXT,
  type TEXT,                     -- paragraph|heading|todo|code|table|embed|transcript_seg
  order_key TEXT,                -- fractional index (LexoRank-style) for O(1) reorder
  text_content TEXT,             -- flattened text for search/backlink scan
  attrs_json BLOB
)
link(src_note_id, src_block_id, target_note_id?, target_title, kind, resolved)
  -- kind = wikilink | tag | mention ; unresolved links keep target_title for later
tag(id, name); note_tag(note_id, tag_id, block_id?)
attachment(id, note_id, block_id, content_hash, mime, ...)  -- content-addressed

CREATE VIRTUAL TABLE block_fts USING fts5(
  text_content, content='block', content_rowid='rowid');  -- external-content, BM25
```

**Backlink maintenance flow (Rust core, on every save):** decode `doc_json` → walk node tree → (a) upsert `block` rows, delete orphans; (b) extract `[[…]]`/`#…`/`@…` tokens → replace all `link` rows for that note in one transaction; (c) push changed block text into `block_fts`. Backlinks panel = `SELECT … FROM link WHERE target_note_id = ?`. Unlinked mentions = FTS5 title query minus already-linked sources. All **idempotent and fully rebuildable** — drop every derived table and regenerate from `note.doc_json`: the crash-recovery guarantee.

### 2.6 Recommendations

1. **Document model:** hybrid JSON-doc-as-truth + derived block/link index in encrypted SQLite. Assign a **stable `blockId` in every block node's `attrs`** so blocks are addressable across the whole unified app. Markdown import/export as a feature, not storage.
2. **Editor:** Tiptap (ProseMirror) in the React WebView; custom nodes including a **transcript-segment node** carrying `evidence_segment_id` linking notes↔meeting artifacts. Validate `doc_json` against Tiptap's JSON Schema before persist. *Reject Lexical (loses symmetry) and Slate (too much bespoke work).*
3. **Keep the editor thin; own semantics in Rust.** The `notes` crate owns parsing, block projection, link extraction, FTS indexing, backlink queries. The WebView never writes derived tables.
4. **Backlinks/tags/daily-notes as derived projections.** One `link` edge table powers backlinks *and* unlinked mentions. Tags are rows, not folders. **Daily notes are ordinary notes keyed by date.** Unresolved `[[links]]` persist their target title and auto-resolve when the target is later created.
5. **Ordering via fractional index** (`order_key`), shared with the task list for O(1) drag-reorder.
6. **Search:** reuse the inherited FTS5 + embeddings pipeline unchanged; notes become a first-class citizen in unified search with block-level evidence citation.
7. **Defer CRDTs, stay CRDT-shaped.** Keep block IDs stable and edits block-granular so Loro can be adopted later without a model rewrite. Do **not** adopt Yjs-in-JS as source of truth — it would move authority into the WebView.

---

## 3. Tasks, Reminders & Natural-Language Scheduling

### 3.1 Date semantics — the deep differentiator

Three temporal concepts recur across every serious tool; conflating them is the classic mistake:

- **Scheduled / start / "When" / defer** — when the task *appears* and stops cluttering the active list. Things = *When*/start date; OmniFocus = *Defer Until*. A future start **hides** the task until that date.
- **Deadline / due** — when it *must* be done. In Things a deadline does **not** hide the task; it stays in Anytime and surfaces as overdue. OmniFocus surfaces both in **Forecast**.
- **Reminder / alert time** — a wall-clock instant firing an OS notification. Orthogonal: a task due "Friday" can have a reminder "Thursday 5pm."

**Things' four buckets are derived views, not stored states:** Today = start ≤ today OR overdue; Upcoming = future start or deadline; Anytime = actionable, no start; Someday = explicitly deferred. **Buckets are queries, not columns.** This is the model to copy.

### 3.2 Hierarchy & recurrence

**Hierarchy** converges on *Area → Project → Task → Subtask/Checklist*. Things adds **Headings** (lightweight sections); OmniFocus adds sequential-vs-parallel + **Review** intervals; Todoist uses Projects + Sections + nested tasks. **Things' Area/Project/Heading/Task/Checklist is the sweet spot** for a "casual" audience — richer than Apple Reminders, less ceremony than OmniFocus.

**Recurrence** everywhere converges on **RFC 5545 RRULE**, with one critical product split:

- **Fixed-schedule** ("every Monday") — next occurrence from the *rule anchor* (DTSTART), independent of completion.
- **Completion-relative** ("every 3 days *after I finish*") — Todoist's `every!` (recompute from completion) vs `every` (from scheduled date).

Dominant implementation (TaskNotes, dmfs OpenTasks, Todoist): the *template* holds the RRULE; on completion you **materialize the next occurrence** and advance the scheduled date, rather than pre-expanding an infinite series. TaskNotes stores completed instances in `complete_instances[]` and never mutates `status`; DTSTART injected on first completion. `COUNT`/`UNTIL` bound the series.

### 3.3 Natural-language quick-add

**Todoist is the gold standard:** "every 3rd Tuesday starting Aug 29 ending in 6 months," "tomorrow at 4pm," plus inline `#project @label p1` tokens parsed and highlighted live as you type. The approach is a **rule/grammar-based parser (not an LLM)** for latency and determinism.

**Rust ecosystem (verified):**
- **`rrule`** (fmeringdal) — mature, RFC-5545 compliant, `RRule` + `RRuleSet` (rrules/exrules/rdates/exdates), chrono iteration. **Recommended recurrence engine.**
- **NL date parsing:** `event-parser` / `date_time_parser` (chrono+regex) are thin; `two-timer`, `chrono-english` are candidates. **None match Todoist coverage** — expect to write a domain grammar.

### 3.4 Local notification scheduling — the hardest cross-platform problem

| Platform | Mechanism | Reality |
|---|---|---|
| **macOS** | `UNUserNotificationCenter` + `UNTimeInterval`/`UNCalendarNotificationTrigger` | Real OS-owned scheduling — system delivers without the app running. Requires a proper signed `.app` bundle; raw launch agents hit `UNErrorCodeNotificationsNotAllowed`. |
| **Windows** | `ScheduledToastNotification` (WinRT) | OS-owned, fires when app closed, but **one-shot only** (recurring ctor deprecated/no-op since Win10), **cannot be updated after scheduling**, snooze 60s–60min. Needs a registered AppUserModelID / Start-menu shortcut. |
| **Linux** | freedesktop `org.freedesktop.Notifications` / `notify-rust` | **No OS-owned scheduled service.** Fires *immediately* only; you must schedule in-process. Tauri push is UnifiedPush-only, irrelevant offline. |
| **Tauri** | `tauri-plugin-notification` | Shows toasts; weak/no true scheduled delivery when closed. `tauri-plugin-background-service` exists but force-quitting kills all background tasks. |

### 3.5 Key trade-offs

1. **OS-scheduled vs app-scheduled.** OS-owned survives app-closed and is battery-friendly but is *fire-and-forget, uneditable, one-shot, count-capped*. App-owned gives full control (edit, snooze, rich actions, unlimited) but only fires while a process runs. **Neither alone suffices.**
2. **Pre-expand vs materialize recurrences.** Pre-expanding is simple to query but explodes on unbounded rules and desyncs on edits. Materialize-on-completion is compact and correct but needs catch-up logic.
3. **Grammar vs LLM for NL entry.** Grammar is deterministic, <5ms, offline, testable but brittle at edges. The resident LLM can fall back for ambiguity at ~seconds latency. **Hybrid wins.**
4. **Deadline-hides vs not.** Things' model (only *start* hides; deadline keeps task active) is more intuitive for a casual audience than OmniFocus's defer-centric model.
5. **Reminder as entity vs field.** Reminders attach to notes, tasks, *and* meetings and can be standalone — so a **first-class table**, not a column.

### 3.6 Recommended data model

```sql
area(id, title, position, archived_at, created_at, updated_at)
project(id, area_id?, title, note_id?, kind,              -- 'parallel'|'sequential'
        status,                                            -- 'active'|'someday'|'done'|'dropped'
        start_on DATE?, deadline_on DATE?,
        review_interval_days INT?, last_reviewed_at?,
        position, created_at, updated_at, completed_at?)
heading(id, project_id, title, position)
task(id, project_id?, heading_id?, area_id?,
     title, notes_md TEXT,
     status,                    -- 'open'|'done'|'canceled'
     start_on DATE?,            -- "When"/scheduled: HIDES task until this date
     start_at DATETIME?,        -- optional time component
     deadline_on DATE?,         -- hard due date (does NOT hide)
     someday INTEGER DEFAULT 0,
     priority INTEGER,          -- 0..3
     recurrence_id?,            -- FK, template tasks only
     parent_task_id?,           -- subtasks
     source_meeting_id?,        -- action-item provenance (MeetingArtifactV1)
     source_evidence_segment_ids JSON?,
     position REAL,             -- fractional index for O(1) drag-reorder
     created_at, updated_at, completed_at?)
checklist_item(id, task_id, title, done INTEGER, position)
recurrence(id, rrule TEXT,      -- 'FREQ=WEEKLY;BYDAY=MO'
           dtstart DATETIME, mode TEXT,   -- 'fixed' | 'after_completion'
           until_on DATE?, count INT?,
           next_scheduled_on DATE?,       -- materialized head
           complete_instances JSON)       -- [dates] for skip/history
reminder(id,
         target_type TEXT,      -- 'task'|'note'|'meeting'|'standalone'
         target_id?, title TEXT,
         fire_at DATETIME,      -- UTC, absolute wall-clock instant
         tz TEXT,               -- IANA zone captured at creation (DST-safe)
         rrule TEXT?, state TEXT,          -- 'pending'|'fired'|'snoozed'|'dismissed'|'missed'
         snoozed_until DATETIME?,
         os_handle TEXT?,       -- macOS/Win scheduled-notification id (for cancel)
         delivered_at?, created_at, updated_at)
```

Store `fire_at` in **UTC** but keep the **IANA `tz`** so "9am every day" survives DST and travel — recompute against `tz`, never a frozen offset. **Buckets are views:**

```sql
-- TODAY
WHERE status='open' AND someday=0
  AND (start_on <= :today OR deadline_on < :tomorrow OR deadline_on = :today)
-- Upcoming = future start_on/deadline_on; Anytime = open, start_on null/past, someday=0;
-- Someday = someday=1
```

### 3.7 Recommended scheduler — dual-layer, belt-and-suspenders

Mirrors the crash-safe philosophy of the inherited session engine. SQLite is durable truth; the in-memory heap is derived and rebuilt on boot.

- **Layer A — in-process Rust timer (authoritative while running).** A Tokio timer-wheel / min-heap keyed on `fire_at`, rebuilt from SQLite on launch. On fire: mark `state='fired'`, deliver via native toast, emit to WebView if open, and (recurring) compute next `fire_at` with the `rrule` crate and re-enqueue. Owns snooze, edit, rich actions, unlimited reminders.
- **Layer B — OS-scheduled handoff (survives app-closed).** For each pending reminder within a rolling **14-day horizon**, also register a one-shot OS notification (`UNCalendarNotificationTrigger` / `ScheduledToastNotification`), storing `os_handle`. Cancel the matching handle on any edit/cancel/fire. **Linux has no OS layer** — document honestly per the baseline's capability-reporting discipline.
- **De-duplication.** Delivery gated on reminder `state`: whichever layer fires first flips `pending→fired`; the other no-ops. `os_handle` lets Layer A revoke Layer B on any mutation.
- **Missed-reminder catch-up.** On every launch and wake-from-sleep, sweep `state='pending' AND fire_at < now()`, batch into one grouped "You missed N reminders" notification (not a burst), mark `missed`, surface in-app. Essential for Linux and macOS/Windows edge cases; reuses the NDJSON journal recovery ethos.
- **Persistence.** Register the app as a **login item** (macOS `SMAppService`, Windows Run-key/Task Scheduler, Linux XDG autostart) with an optional lightweight "reminders-only" background mode so Layer A can run without the full WebView.

### 3.8 Natural-language entry (hybrid `app-nlp` crate)

1. **Grammar first** for the 90% path — a `nom`/regex grammar over date words, `every[!]`, times, and inline `#project @tag !priority` tokens (Todoist-style live highlighting), emitting `ParsedEntry { title, start, deadline, rrule, tags }`. Validate generated RRULEs with the `rrule` crate.
2. **Local-LLM fallback** (resident Qwen3, schema-constrained exactly like `MeetingArtifactV1`) only on low grammar confidence, returning the same struct with one-repair-then-deterministic-fallback. **Never invent a date the user didn't state** (mirrors the "must not invent owners/dates" rule).

### 3.9 Unification hooks

- **Meeting → Task:** `MeetingArtifactV1.action_items[]` map to `task` rows with `source_meeting_id` + `source_evidence_segment_ids`, owner→assignee and due_date→`deadline_on` **only if extracted from evidence**.
- **Task/Note/Meeting → Reminder:** polymorphic `reminder.target_*` covers all four pillars from one table and one scheduler.
- **Unified search:** `task`, `reminder`, `project` join the existing FTS5 + embedding index; buckets and Forecast become saved queries.

---

## 4. Local-First Sync, CRDTs & Encryption

### 4.1 Bottom line

Casual Note ships **single-device, local-only, no CRDT, no sync engine in v1** — but structures the store so optional E2E-encrypted sync later is a *bolt-on, not a rewrite*. Two decisions make future sync feasible-vs-painful: (1) every mutable entity gets a **stable UUID + a per-entity op-log**; (2) SQLite tables are a *materialized projection* of that log, not the source of truth.

### 4.2 CRDT library landscape (2025–2026)

| Library | State | Fit for Casual Note |
|---|---|---|
| **Yjs** | Production incumbent, ~920K weekly downloads, huge ecosystem (Tiptap, BlockNote, ProseMirror). Best rich-text collab; weak on history/versioning. | JS-centric, binding-heavy — **awkward from a Rust core; would move authority into the WebView. Reject as source of truth.** |
| **Automerge 3.0** (May 2025) | Rust core, stable JS API; 3.0 cut memory ~10x. JSON-like nested maps/lists/text, Git-like history, **built-in sync protocol** (`automerge::sync`), compact binary format. | Native Rust crate fits the stack. **Safe fallback** if you want the more mature ecosystem + built-in sync. Lacks native movable-tree. |
| **Loro 1.0** (2024, Rust) | Fastest in benchmarks, smallest on-disk (Fugue text; 2–5x smaller encoding), uniquely ships a **movable-tree CRDT** and **movable-list**. Youngest ecosystem. | **Best technical fit** — Rust-native (no JS binding tax), and movable-tree/list cover *both* note blocks/folder tree *and* reorderable task lists. **Intended note-body CRDT.** |

### 4.3 Product sync/encryption models

- **Obsidian Sync** — client-side AES-256, vault encrypted on-device with a password-derived key; server is a relay. **Metadata is not E2E** (paths, timestamps, version history visible server-side). File-based, so merge is coarse (whole-file + conflict copies).
- **Standard Notes** (Proton, 2024) — E2E by default (AES-256-GCM / XChaCha20-Poly1305); server is a **blind storage relay** that can't decrypt. Item-level encryption, LWW-ish with conflict duplicates.
- **Anytype** — object-graph, every object individually encrypted, **P2P replication** across user-controlled nodes, E2E by default. Demonstrates object-granular encryption without a mandatory central server.
- **Ink & Switch — Keyhive/BeeKEM** (2024–25 frontier) — capability-based authorization (read/write/admin as signed delegation chains), coordination-free revocation, E2EE with "causal keys" (post-compromise security). **Key engineering insight: don't encrypt each CRDT op separately** (kills compression) — **compress-then-encrypt *ranges* of changes** using the Automerge binary format.

### 4.4 Key trade-offs

1. **CRDT everywhere vs. only where it earns its keep.** CRDTs shine for concurrently-edited rich text; they're *overkill and lossy* for structured records (a `due_date` wants LWW, not character-merge). **Split the model by data type.**
2. **Document granularity.** One CRDT doc per note (and per project/task-list) bounds memory and lets you sync only touched docs. One giant doc scales badly and leaks all activity into every sync. **Per-entity docs are consensus.**
3. **Encrypt-then-compress vs compress-then-encrypt.** Per-op encryption defeats CRDT compression (Keyhive's finding). **Range-based compress-then-encrypt** keeps payloads small.
4. **Metadata leakage.** Even blind relays see object IDs, sizes, timing, graph shape. Obsidian punts; Standard Notes/Anytype minimize. **A "privacy-first" product should encrypt metadata too** — don't over-promise, don't under-deliver.
5. **Sync transport.** Central blind relay (simple, one endpoint, easy zero-knowledge) vs P2P (no server to trust, harder NAT/availability). For an *optional future* feature, a **blind relay is far less operational burden.**
6. **At-rest encryption granularity.** Whole-DB SQLCipher is trivial and transparent but all-or-nothing. Per-record keys enable selective sharing/revocation but add a key-management layer. **Whole-DB is right for local-only default; per-record keys are a sync-era concern.**

### 4.5 Schema sketch — op-log as source of truth

Extends the existing NDJSON session-journal philosophy from meeting sessions to *all* entities.

```sql
entity_op(                        -- source of truth: per-entity change log
  op_id        BLOB PRIMARY KEY,  -- ULID/UUIDv7 (sortable)
  entity_id    BLOB,             -- stable UUID of note/task/reminder/meeting
  entity_kind  TEXT,             -- 'note'|'task'|'reminder'|'meeting'|'tag'|'link'
  actor_id     BLOB,             -- device/replica id (const in v1)
  hlc          TEXT,             -- hybrid logical clock (Lamport+wall) for LWW
  payload      BLOB,             -- CBOR: {field,value} or CRDT change bytes
  synced       INTEGER DEFAULT 0
)
-- Materialized projections (rebuildable from ops):
note(id, notebook_id, title, body_ref, updated_hlc, ...)   -- body_ref -> CRDT doc/CAS
task(id, project_id, status, due, scheduled, sort_key, updated_hlc, ...)
reminder(id, target_entity_id, fire_at, rrule, snooze_until, updated_hlc, ...)
link(id, src_id, dst_id, kind)
```

**Per-type conflict policy (the important part):**

| Entity | v1 (local) | Sync-era merge |
|---|---|---|
| Note *body* | plain markdown/blocks | **CRDT** (Loro/Automerge), char-level merge |
| Note *metadata* (title, notebook) | column | **LWW by HLC** |
| Task fields (status, dates, notes) | columns | **per-field LWW by HLC** |
| Task/list ordering | fractional index | movable-list CRDT or fractional-index LWW |
| Reminders | columns | **LWW**; fire-state is device-local, never synced-as-truth |
| Tags/links | rows | **add-wins OR-Set** (grow-only + tombstones) |
| Meeting artifacts | immutable-once-generated | append-only; regeneration = new version, no merge |

Storing **HLC + stable UUIDs in v1 even single-device** is cheap insurance. A pure `INTEGER PRIMARY KEY AUTOINCREMENT` + wall-clock `updated_at` is **the trap** — those IDs collide across devices and force a painful migration.

**Encrypted-sync envelope (future):** `sync_object(object_id, entity_id, ciphertext, nonce, key_epoch, causal_deps[])` — compress-then-encrypt a *range* of ops; XChaCha20-Poly1305; epoch for key rotation; relay stores/forwards blindly by opaque IDs.

### 4.6 Recommendations

1. **v1 = local-only, whole-DB SQLCipher, no CRDT, no sync.** Key in OS keystore. Don't build sync speculatively.
2. **Adopt an op-log + HLC + UUID substrate now** in a new `sync-core` crate seam (**dormant in v1**). Every mutation also appends an `entity_op`. **The single highest-leverage decision** — the difference between "add a sync crate" and "rewrite storage."
3. **Pick Loro as the intended note-body CRDT, don't wire it in v1.** Store note bodies as blocks with stable block IDs from day one so the later swap is a re-encode, not a re-model. (Automerge 3.0 is the acceptable safe fallback.)
4. **Do NOT CRDT structured entities.** Tasks/reminders/tags use **per-field LWW by HLC**, OR-Set for tags/links, fractional indexing for order.
5. **Reserve sync for a central blind relay, not P2P.** Reuse the "network owned by one service" discipline — only a `sync` service touches the network.
6. **When sync ships, compress-then-encrypt over op ranges** (Keyhive lesson), XChaCha20-Poly1305, per-notebook/project content keys wrapped by a device/account key, HLC ordering. **Encrypt metadata too** — go beyond Obsidian's plaintext-metadata compromise. Follow Keyhive/BeeKEM rather than inventing crypto.
7. **Meeting artifacts need no CRDT** — generated, evidence-linked, immutable per generation. Sync as append-only encrypted blobs; regeneration = new version.

---

## 5. On-Device AI: STT, Local LLMs, Embeddings & RAG-over-Notes

### 5.1 Speech-to-text landscape

The 2025–26 on-device ASR field has bifurcated:

- **whisper.cpp** (inherited baseline) — portability king: GGUF-quantized, Metal/CUDA/CPU, 99 languages. Comparatively slow.
- **NVIDIA Parakeet TDT 0.6B v3** — CTC/TDT transducer now topping practical leaderboards: ~6.3% WER vs Whisper Large v3's ~7.4% on Open ASR, ~4–10x faster on CPU (thousands RTFx on GPU, ~30x realtime INT8 on modern CPU). **Costs: English/25–40 locales only; ONNX/NeMo runtime, not a clean single C++ lib.**
- **distil-whisper / faster-whisper** (CTranslate2, INT8/FP16) — Whisper accuracy at 4x throughput, but Python/CT2-runtime bound.
- **Moonshine v2** — streaming/edge niche: sub-200ms latency, tiny footprint, variable-length inference (no 30s padding); good for live captions on constrained hardware.
- **Apple SpeechAnalyzer/SpeechTranscriber** (macOS 26/iOS 26) — strong, free, OS-native on Apple Silicon; non-portable.

**Product pattern** (MacWhisper, Whisper Notes, OpenWhispr): use a **fast streaming model live, a more accurate model for the final pass** — exactly the two-pass design in the baseline.

### 5.2 Local LLM runtimes & models

- **Runtimes:** `llama.cpp` + GGUF is the cross-platform default (CPU/CUDA/Metal/Vulkan/SYCL); **MLX** is Apple-Silicon-optimized (better tok/s + memory on M-series); Ollama/LM Studio are the wrappers consumer "chat-with-notes" products embed.
- **Models:** **Qwen3** is the current small-model leader for local reasoning/structured output, with a thinking-mode toggle mapping well to meeting-artifact extraction. GGUF sweet spots: **Qwen3-4B** (Q4_K_M ~2.5GB) entry, **Qwen3-8B** (Q4_K_M ~5GB) mainstream, **12–14B** high-RAM/GPU. Alternatives: Llama 3.2 3B, Gemma 3 4B/12B, Phi-4-mini.
- **Structured output:** GBNF grammar/schema constraints in llama.cpp are the standard mechanism for the MeetingArtifactV1 contract.

### 5.3 Embeddings & vector search

- **Embedding models:** **BGE** (bge-base/large-en-v1.5, MTEB ~63–65), **Nomic Embed v1.5** (8192-token context, Matryoshka-truncatable), **GTE-large-v1.5**, **EmbeddingGemma-300M** (Google, <200MB quantized, ~22ms/embed, multilingual, Matryoshka 768→128). **Qwen3-Embedding** leads MTEB multilingual (0.6B viable) but is heavier.
- **Vector index:** **sqlite-vec** (asg017; pure-C, SIMD, WASM-portable, brute-force + partitioning, single-SQL-query join with your data) supersedes the older FAISS-backed sqlite-vss. **FAISS/usearch** (HNSW) are faster at million-vector scale (FAISS ~10ms vs sqlite-vec ~33ms on 1M×128) but are separate indexes to keep in sync and manage as extra native deps.
- **Obsidian-plugin consensus** (Smart Connections, ObsidianRAG): chunk each note, embed with nomic/BGE, store locally, **hybrid vector+keyword+rerank**, cite back to the exact heading.

### 5.4 Key trade-offs

1. **Accuracy vs speed vs portability (STT).** Parakeet wins English accuracy+speed but breaks the "one clean native lib, 99 languages, offline" contract and adds an ONNX runtime. whisper.cpp is the honest default; Parakeet is a compelling opt-in accelerator.
2. **Chunk granularity.** Transcripts need **time-anchored** chunks; notes need **structure-anchored** (heading/block) chunks. One size doesn't fit both.
3. **Embedded vs external index.** sqlite-vec keeps everything in the one encrypted SQLCipher store (crash-safe, one backup, transactional with FTS5) at the cost of raw ANN speed. For a *personal* notebook (10k–100k chunks, not millions), sqlite-vec's search is fast enough (<50ms) and vastly simpler. usearch/FAISS fracture the storage/encryption/recovery story.
4. **Embedding dim vs disk/RAM.** 768-dim f32 = 3KB/chunk; 100k chunks = 300MB. Matryoshka truncation to 256-dim + int8 cuts this ~9x with minor recall loss.
5. **Recall vs grounding discipline.** Pure vector misses exact terms (names, IDs, `[[wikilinks]]`); pure FTS misses paraphrase. **Hybrid (BM25 + vector) with reranking** is the reliable answer. Grounding is a prompt+schema discipline: every answer cites `note_id`/`segment_id`; the model refuses to invent owners/dates.
6. **Re-embedding cost.** Debounced, content-hashed, incremental re-embedding is mandatory or the CPU thrashes — index on save, batch-reconcile on idle.

### 5.5 Schema — unified chunk table + evidence-grounded answer contract

```sql
CREATE TABLE chunk (
  id INTEGER PRIMARY KEY,
  source_type TEXT NOT NULL,     -- 'note'|'transcript'|'task'|'reminder'
  source_id   TEXT NOT NULL,     -- note_id / session_id / task_id
  parent_ref  TEXT,              -- heading path OR segment span
  seq INTEGER, text TEXT NOT NULL,
  char_start INTEGER, char_end INTEGER,   -- note anchors
  t_start_ms INTEGER, t_end_ms INTEGER,    -- transcript anchors
  content_hash TEXT NOT NULL,     -- skip re-embed if unchanged
  embed_model TEXT,               -- provenance for re-index migrations
  updated_at INTEGER );
CREATE VIRTUAL TABLE chunk_fts USING fts5(
  text, content='chunk', content_rowid='id', tokenize='unicode61');
CREATE VIRTUAL TABLE chunk_vec USING vec0(
  chunk_id INTEGER PRIMARY KEY, embedding FLOAT[256]);  -- Matryoshka int8
```

**Chunking policy:** notes → split on headings/blocks, ~200–400 tokens, 1–2 sentence overlap, carry heading breadcrumb into `text`. Transcripts → VAD/speaker-turn windows ~30–60s with `t_start_ms/t_end_ms` for audio deep-links. Tasks/reminders → one chunk each.

**Answer contract (evidence-grounded), reusing MeetingArtifactV1 discipline:**

```jsonc
AnswerV1 {
  answer: string,                     // synthesis, no unsourced claims
  citations: [{ chunk_id, source_type, source_id,
                anchor: {char_start?, t_start_ms?}, quote }],
  confidence: "high"|"low",
  unanswered: boolean                 // true => "not found in your notes"
}
```

**Auto-linking/tagging as reversible suggestions** (never silent mutation): `suggestion(id, kind, from_id, to_id, payload, score, status DEFAULT 'pending', evidence_chunk_ids, created_at)`. Action-item→task: `MeetingArtifactV1.action_items[]` generate `suggestion(kind='task_from_action_item')`; on accept, insert a Task linked back to session + segment, preserving owner/due only if extracted from evidence.

### 5.6 Recommendations

- **STT:** whisper.cpp portable default (base live / small-medium final, two-pass). Add **Parakeet TDT v3 as opt-in "Turbo (English)"** behind the `speech-api` trait (ONNX Runtime adapter `speech-parakeet`). Expose **Apple SpeechTranscriber** as a zero-download native macOS adapter. Moonshine only for a low-power live-caption mode. Report capability honestly per platform.
- **LLM by hardware tier** (Qwen3 default, MLX on Apple Silicon): Tier 1 (≤8GB) Qwen3-4B Q4_K_M (fallback Llama 3.2 3B, Gemma 3 4B); Tier 2 (16GB / 8GB VRAM) **Qwen3-8B Q4_K_M — recommended default**; Tier 3 (32GB+ / 12GB+ VRAM) Qwen3-14B Q4_K_M/Q5. Single resident context + bounded queue. GBNF grammars hard-constrain MeetingArtifactV1 and AnswerV1, one repair then deterministic fallback. Thinking-mode for extraction/summarization, non-thinking for quick "ask your notes."
- **Embeddings:** default **EmbeddingGemma-300M** (or bge-base-en-v1.5), **Matryoshka-truncated to 256 dims + int8**. Offer Nomic Embed v1.5 as a "long-context/multilingual" upgrade. Record `embed_model` per chunk so a model change triggers background re-index, not silent corruption.
- **Vector search:** **sqlite-vec inside SQLCipher** — do *not* bolt on FAISS/usearch. Revisit usearch only above ~500k chunks.
- **Retrieval/orchestration** (`ai-workspace`): hybrid FTS5 BM25 ∪ sqlite-vec KNN → **RRF** → optional cross-encoder rerank (bge-reranker-base, opt-in) → top-k with evidence anchors, spanning all `source_type`s. Orchestrator: retrieve → grounded prompt with numbered evidence → constrained-decode AnswerV1 → **verify every citation resolves to a real chunk** before display; if none, return `unanswered:true` rather than hallucinate. Auto-link/tag run as **idle-time batch jobs producing reversible cited suggestions** — never automatic edits. Embedding indexing is incremental, content-hash-gated, debounced on save.
- **Crate additions:** `speech-parakeet`, `embeddings` (trait + `-gemma`/`-bge`), extend `search` (hybrid+RRF+rerank), add `ai-workspace`. Network confined to the ModelDownload service.

---

## 6. Tauri 2 / Rust Cross-Platform Desktop Shell

### 6.1 Landscape — the Raycast/Spotlight resident model

The reference shape for a quiet, always-present notebook + capture app: a **tray/menu-bar resident process**, a **global hotkey** summoning a frameless always-on-top quick-capture panel, a separate heavier main window, and a background scheduler for reminders — all while staying out of Dock/taskbar/Cmd-Tab when only the panel shows. Reference points: Obsidian (Electron), Things 3 (native Swift), Raycast/Alfred (native, hotkey-driven), Bear, newer Tauri apps.

**Tauri 2 (stable, 2.x through late 2025) covers almost all of this with first-party plugins:**

| Plugin | Version | Purpose |
|---|---|---|
| `tauri-plugin-global-shortcut` | ~2.3.x | System-wide hotkeys, runtime register/unregister, press/release events |
| `tauri-plugin-single-instance` | ~2.x | Forwards argv/cwd/deep-link URLs from a second launch to the running instance. **Register first.** |
| `tauri-plugin-deep-link` | ~2.4.x | `casualnote://` custom scheme; on Windows/Linux pairs with single-instance for cold-start links |
| `tauri-plugin-autostart` | ~2.5.x (Oct 2025) | Launch-at-login: LaunchAgent / registry Run key / XDG autostart |
| `tauri-plugin-notification` | ~2.x | Native toasts, local only. Scheduling is real on *mobile* but **not a durable desktop scheduler** — fire yourself |
| Tray/menu-bar | core (`TrayIconBuilder`) | First-class, no plugin |
| `tauri-plugin-updater` + `fs`/`dialog` | ~2.x | Signed auto-update, file/attachment I/O. Drag-drop built into the webview (`DragDrop` event, `dragDropEnabled`) |

**Security model** is the differentiator vs Electron: IPC commands are deny-by-default, granted per-window via JSON **capability** files referencing typed **permissions**; CSP is compile-time-injected with nonces/hashes so the WebView loads only bundled assets — a natural fit for "no remote content, 100% local."

### 6.2 Key trade-offs

- **Desktop reminder scheduling is yours to own** (the single most important finding). Do **not** rely on the notification plugin to "schedule" on desktop — no OS-backed durable trigger there. Need a **Rust-side Tokio scheduler** reading due reminders from SQLite + a **catch-up sweep on launch/wake**. OS scheduler integration (`launchd`/Task Scheduler) to *relaunch a quit app* is a later, heavy, platform-specific enhancement.
- **`tauri-plugin-sql` vs direct `rusqlite`.** The official `tauri-plugin-sql` is `sqlx`-based and exposes SQL to the *frontend* — wrong shape. Keep **direct `rusqlite`** (with `bundled-sqlcipher` / `libsqlite3-sys` → SQLCipher) behind Rust commands; the frontend never touches SQL. Preserves key handling in the OS keystore and one place for schema/migrations. `tauri-plugin-rusqlite2` exists but you already have the crate.
- **Single vs multi-window.** The quick-capture panel (frameless, transparent, always-on-top, `skipTaskbar`, <100ms summon) and the main window (normal chrome) have opposite needs — **two window definitions is cleaner.** On macOS a tray/panel app that hides the Dock icon uses `ActivationPolicy::Accessory`, but the meeting recorder needs a real window + Dock presence: **switch activation policy dynamically** (Accessory when idle, Regular when main/meeting opens).
- **Global hotkey conflicts.** Can collide with other apps; macOS may need Accessibility/Input-Monitoring grants. Make the hotkey **user-rebindable** and degrade honestly.
- **Sidecars vs in-process.** whisper.cpp/llama.cpp are already Rust-linked crates — **avoid sidecars** (they complicate notarization, add IPC latency). Reserve `externalBin` for genuinely external tools.

### 6.3 Scheduler & deep-link sketches

```
reminders(id TEXT PK, title, body, fire_at INTEGER,  -- unix ms
          rrule TEXT, snooze_until INTEGER,
          status TEXT,                                -- scheduled|fired|dismissed|snoozed|done
          link_kind TEXT, link_id TEXT, created_at, updated_at INTEGER);
CREATE INDEX idx_rem_due ON reminders(status, fire_at);
```
```rust
loop {
  let next = db.min_due();                 // WHERE status='scheduled' ORDER BY fire_at LIMIT 1
  tokio::select! {
    _ = sleep_until(next.fire_at) => { notify(next); advance_rrule_or_mark_fired(next); }
    _ = reload_rx.recv()          => { /* added/edited/snoozed */ }
    _ = wake_rx.recv()            => { /* system resume: sweep overdue, coalesce */ }
  }
}
```
On launch/resume, a **catch-up sweep** fires (coalesced) any `status='scheduled' AND fire_at <= now`. Deep links: `casualnote://note/<id>`, `.../task/<id>`, `.../capture?text=...`, `.../meeting/<id>` — parsed in Rust, emitted to the frontend router; single-instance forwards cold-start URLs. Capability layout: `capabilities/main.json` (notes/tasks/reminders/meetings + fs scoped to attachments) and `capabilities/capture.json` (only quick-capture + window show/hide/close).

### 6.4 Recommendations

1. **Adopt now:** `single-instance` (register first), `global-shortcut`, `deep-link`, `autostart`, `notification`, `updater`, `fs`, `dialog`, `os`, `process`. **Skip `tauri-plugin-sql`** — keep `storage` on direct `rusqlite`+SQLCipher.
2. **Own the reminder scheduler in Rust** (a `scheduler` crate: Tokio timer + SQLite-backed heap + launch/resume catch-up). Treat `tauri-plugin-notification` purely as the *delivery* surface. **Non-negotiable** given desktop's lack of durable scheduled notifications.
3. **Two windows + tray, dynamic activation policy.** `main` (chrome) and `capture` (frameless, transparent, always-on-top, `skipTaskbar`, `visible:false`, **created at startup and toggled** — not created-on-hotkey, too slow for <100ms). Tray menu: New Note, Quick Capture, Start Meeting, Today's Tasks, Reminders, Quit. macOS `Accessory` idle → `Regular` when main/meeting opens.
4. **Global quick-capture hotkey**, default `Cmd/Ctrl+Shift+Space`, **user-rebindable**, toggles the panel; capture writes straight into the encrypted store via a scoped command and routes to note/task/reminder by parse. Fail gracefully, report capability honestly.
5. **Security:** compile-time CSP with no remote sources; per-window capability files; no SQL or raw FS to the WebView; scope `fs` to the attachments/content-addressed dir only. Attachments via `DragDrop` event → hashed into content-addressed store in Rust.
6. **Packaging/signing/update:** macOS DMG + Developer ID codesign → notarize → staple (keep whisper/llama **in-process** to dodge the known `externalBin` notarization pitfalls). Windows MSI + Authenticode (EV/OV avoids SmartScreen). Linux AppImage + Flatpak. `tauri-plugin-updater` with a **separate updater signing key**, self-hosted static `latest.json`, gated behind user consent.
7. **Autostart off by default**, opt-in; on start launch minimized to tray, run the catch-up sweep, enter "Offline Ready" with no network call.

---

## 7. Unified Knowledge Model, Linking & Search UX

### 7.1 Landscape — where "structure" lives

The 2025–26 PKM field converged on a few ideas but split on where structure lives:

- **Obsidian** — file-first Markdown vaults; `[[wikilinks]]` → in-memory link graph; backlink panels, tags, **Dataview** (queries over inline `key:: value` fields + frontmatter). Structure is *emergent per-file*, no central schema — portable but slow/fragile for cross-entity typed queries.
- **Logseq** — outliner-first; every **block** is addressable/referenceable/transcludable, with page- and block-level backlinks. In 2025 shipped the **DB version**: SQLite persistence + in-memory **DataScript (Datalog)** graph, versioned properties, Malli validation, EAV model. **The single most relevant precedent** — they concluded file-scanning couldn't scale bidirectional links + queries and moved to SQLite-as-store + graph-in-memory.
- **Notion** — database-first; typed databases with **multiple views** (table/board/calendar/gallery/timeline) over the same rows, relation/rollup properties. Powerful but heavyweight + cloud-first. The *views-over-one-dataset* idea is central.
- **Tana & Capacities** — the most interesting convergence: **"everything is a node/object" + typed tagging.** Tana's **supertags** attach a schema (fields) to any node retroactively — freeform capture that becomes a structured database. Capacities uses explicit object types (Book, Person, Meeting, Project) with two-way links. Both make daily notes the capture spine and let AI generate typed objects from raw input. **Exactly Casual Note's problem shape.**

**Search — 2025 SOTA:** **hybrid retrieval fused with Reciprocal Rank Fusion (RRF).** Alex Garcia's canonical SQLite recipe (copied by litesearch, sqlite-rag, vstash): FTS5 (BM25) and sqlite-vec (ANN over 384-dim embeddings) as separate ranked CTEs, merged by *rank position* — `score = Σ weight/(k + rank)` — deliberately **not** normalizing incompatible scores. BM25 nails exact terms/IDs/phrases; embeddings nail synonymy/paraphrase; RRF captures both with zero training. Command-palette UX standardized on the **quick-switcher/Spotlight** pattern (Obsidian, Capacities Search 2.0, PowerToys Command Palette, Raycast): one keystroke, fuzzy title + full-text, inline type/date/tag filters, keyboard-only, increasingly an "ask" mode routing to RAG.

### 7.2 Key trade-offs

1. **Schema-on-write vs schema-on-read.** Notion forces structure up front (rigid, powerful); Obsidian defers all to read-time Dataview (flexible, slow, inconsistent). **Tana's supertag model is the sweet spot:** schema-on-write for the *four first-class entities*, schema-on-read (tags + inline fields) for everything user-defined.
2. **Files-as-store vs DB-as-store.** Logseq's migration is the verdict: for a linking + query + semantic-search-heavy app, **SQLite is source of truth**; Markdown export is a feature.
3. **One node table vs per-type tables.** "Everything is a node" makes linking uniform but pushes type-specific fields into EAV/JSON, weakening integrity. Per-type tables give clean constraints but painful cross-entity UNIONs. **Resolution: a thin universal `entity` spine + per-type detail tables** — uniform linking/search over the spine, strong typing in detail tables.
4. **Graph view vs table vs outline.** The force-directed graph is beautiful but low-utility for daily work; users navigate via backlinks, search, lists. **Invest in backlink panels, saved filters, and list/table/calendar views first; treat the global graph as a secondary "explore" surface** — ideally a *local neighborhood* graph, not a hairball.
5. **Semantic search cost.** Embeddings recompute on edit and add latency; pure FTS is instant. Resolved by hybrid + async: FTS5 gives instant results, semantic streams in and re-fuses.

### 7.3 Schema — universal spine + one polymorphic link table

```sql
entity(                             -- one row per addressable thing
  id TEXT PRIMARY KEY,             -- ULID
  kind TEXT NOT NULL,              -- note|task|reminder|meeting|person|block|tag|attachment
  title TEXT, snippet TEXT,        -- quick-switcher label + denormalized body for FTS
  created_at, updated_at, archived_at,
  daily_date TEXT );               -- non-null => belongs to a daily note (spine)
block(id, parent_entity_id, parent_block_id, order_key, kind, text, props JSON);
task(id, status, project_id, area_id, scheduled_date, due_date, priority, ...);
reminder(id, fire_at, rrule, snoozed_until, target_entity_id, ...);
meeting(id, started_at, ended_at, session_id, artifact_json, ...);
person(id, display_name, emails JSON, ...);

link(                               -- ONE polymorphic edge table for the whole graph
  src_id TEXT NOT NULL, dst_id TEXT NOT NULL,
  rel TEXT NOT NULL,               -- mentions|backlink|spawned_from|about|attends|
                                    -- action_item_of|reminds|tagged|child_of
  src_block_id TEXT,               -- exact origin block (precise backlinks)
  evidence_segment_ids JSON,       -- transcript segments (meeting-derived facts)
  created_at,
  PRIMARY KEY(src_id, dst_id, rel, src_block_id) );
CREATE INDEX link_dst ON link(dst_id, rel);   -- backlink panel = query by dst

tag(id, name, color, schema_json);            -- schema_json = optional field defs
CREATE VIRTUAL TABLE fts USING fts5(title, snippet, content='entity', ...);
CREATE VIRTUAL TABLE vec USING vec0(embedding float[384]);  -- sqlite-vec, BGE-small
```

**Backlinks fall out for free:** a panel for X is `SELECT src FROM link WHERE dst_id=X`. Bidirectionality is *derived*, not stored twice — write one directed edge, read from either side (avoids the dual-write consistency bug).

**The crucial cross-link — task ↔ note ↔ meeting.** When an action_item becomes a task: `entity(kind=task)` + `task` row; `link(src=task, dst=meeting, rel='spawned_from', evidence_segment_ids=[...])`; if discussed in a note block, `link(src=task, dst=block, rel='about')`. Now the task shows "From meeting *Q3 Planning* (00:14:22) → jump to transcript evidence," the meeting shows "3 action items → tasks," the note shows the task inline — all one `link` query per direction. **`evidence_segment_ids` ride on the edge, so provenance survives task edits.**

### 7.4 Recommendations

1. **Adopt the universal `entity` spine + one polymorphic `link` table** — the highest-leverage decision. Uniform linking, one backlink implementation, one search index, one graph; per-type detail tables keep strong typing. Store edges **directed**, derive bidirectionality on read.
2. **Make the daily note the capture spine.** `entity.daily_date` threads notes, quick-captured tasks, and meeting stubs onto a date. NL quick entry parses locally (Rust grammar, not the LLM on the hot path) into typed entities + links appended to today's daily note.
3. **Tags = typed entities with optional schema (supertags-lite).** Plain tag = `link(rel='tagged')`; a tag with `schema_json` upgrades tagged notes into a queryable "database of books" without Notion's rigidity.
4. **Search = FTS5 + sqlite-vec fused by RRF** (Garcia recipe) in the Rust `search` crate. BM25 returns synchronously (<10ms) for instant palette results; embedding query (BGE-small) runs async and re-fuses. First-class filters `type: tag: date: person: is:` compile to SQL predicates over the spine before fusion. Results cite entity + block/timestamp.
5. **One command palette (Cmd/Ctrl-K), three modes:** **Go** (quick-switcher: fuzzy title + BM25, recency-boosted), **Do** (commands: "New task", "Start recording", "Snooze reminder"), **Ask** (hybrid RAG → cited answer, routed to the resident llama.cpp + bounded queue). Mode by leading sigil: `>` command, `?`/NL → ask, `#`/`@`/`[[` → scoped entity search.
6. **Views, prioritized:** backlink panel + inline saved filters (**"smart lists"** = stored queries) first; table/board/calendar over any filter second; **local neighborhood graph** (1–2 hops) third. **Skip the global force-graph as a v1 headline feature.**
7. **Saved queries as a stored DSL**, not raw SQL — a small typed JSON object (`{kind, tags, date_range, rel, sort}`) compiled to SQL in Rust; safe, portable, versionable, exposable to the AI workspace as tools.
8. **Keep files as export, DB as truth.** Wikilink resolution on import maps `[[Title]]` → `entity.id` creating `link(rel='mentions')`; unresolved links become lightweight placeholder entities.

---

## 8. Competitive Landscape

### 8.1 Feature/architecture matrix

| Dimension | Notion | Obsidian | Logseq | Things 3 | Todoist | Granola / meeting-bots | **Casual Note** |
|---|---|---|---|---|---|---|---|
| **Primary job** | All-in-one docs+DB | PKM notebook | Outliner PKM | Task manager | Task manager | Meeting notes | **Notebook + tasks + reminders + meetings unified** |
| **Storage** | Cloud (sharded Postgres) | Local `.md` files | Migrating to local SQLite | Local (SQLite, iCloud sync) | Cloud | Cloud | **Local encrypted SQLite (SQLCipher), JSON-doc-as-truth** |
| **Offline** | Limited | Full | Full | Full | Limited | No | **Full — offline-by-default, network only for model/app download** |
| **Doc model** | DB block-tree | MD file | Block-over-MD/DB | N/A | N/A | Transcript+summary | **Tiptap JSON blob + derived block/link/FTS index** |
| **Editor** | Proprietary | CodeMirror/MD | Custom outliner | Native | Native | — | **Tiptap (ProseMirror), custom nodes incl. transcript-segment** |
| **Backlinks/graph** | Relations/rollups | `[[wikilinks]]` + graph | Block refs + graph | No | No | No | **One polymorphic `link` table; backlinks free; neighborhood graph** |
| **Tasks model** | DB with views | Plugins (Tasks/Dataview) | TODO markers | **Area/Project/Heading, derived buckets** | Projects/sections | Action items only | **Things-style derived buckets + start/deadline split** |
| **Reminders** | Basic | Plugin | Plugin | Native alerts | Native + NL | No | **First-class polymorphic + dual-layer scheduler + catch-up** |
| **Recurrence** | Basic | Plugin | Plugin | RRULE-ish | `every`/`every!` | No | **RFC-5545 via `rrule` crate, materialize-on-completion, `every`/`every!`** |
| **NL entry** | Limited | Plugin | No | Some | **Best-in-class grammar** | No | **Hybrid grammar + LLM-fallback, live highlighting** |
| **Meeting capture** | No | No | No | No | No | **Cloud bot / system audio** | **Native local capture (SCK / WASAPI loopback / PipeWire)** |
| **Transcription** | No | No | No | No | No | Cloud | **Local whisper.cpp (+Parakeet Turbo), two-pass** |
| **AI over content** | Notion AI (cloud) | Plugins (mostly cloud) | Plugins | No | No | Cloud LLM | **Local Qwen3, evidence-cited AnswerV1, refuses to hallucinate** |
| **Search** | Full-text (cloud) | FTS + plugins | Datalog + FTS | Simple | Full-text | Transcript search | **Hybrid FTS5 ∪ sqlite-vec + RRF over all four pillars** |
| **Privacy** | Cloud, business terms | Local; Sync metadata not E2E | Local | Local + iCloud | Cloud | Cloud (records meetings) | **100% local, encrypted at rest, no telemetry, evidence-cited** |
| **Sync** | Cloud native | Paid E2E-ish (metadata visible) | Optional | iCloud | Cloud | Cloud | **None in v1; op-log/HLC seam → future blind-relay E2E** |
| **Shell** | Web/Electron | Electron | Electron | Native Swift | Web/native | Web | **Tauri 2 + Rust core (small, native WebView)** |

### 8.2 Where Casual Note wins

- **The only tool that unifies all four pillars in one local, encrypted store with one search, one link graph, one AI workspace** — competitors each own one or two pillars and stitch the rest via plugins or clouds.
- **Best-in-class *local* meeting capture + understanding** that no cross-platform Electron notebook matches, with **evidence-cited** artifacts (Granola-class intelligence without the cloud/bot).
- **Privacy as architecture, not policy** — encrypted at rest, no telemetry, network touched by only two named consented services; goes beyond Obsidian's plaintext-sync-metadata compromise.
- **Rigorous task/reminder semantics** (Things' start/deadline split + Todoist's `every`/`every!` + a dual-layer scheduler that survives app-closed and crashes) that PKM apps bolt on weakly via plugins.

### 8.3 Where competitors remain ahead (v1 non-goals, honest)

- **Real-time collaboration & sharing** (Notion) — Casual Note is single-user v1.
- **Mobile** (all) — desktop only in v1.
- **Mature plugin ecosystems** (Obsidian, Logseq) — no plugin SDK in v1.
- **Cross-device sync out of the box** (Notion, Todoist, Things/iCloud) — deferred; the seam is built but dormant.
- **Calendar/email integration** (Todoist, Notion) — Person entity exists, no external sync in v1.

---

## 9. Consolidated Technology Recommendations

| Concern | Decision | Rejected / deferred |
|---|---|---|
| **Note storage** | JSON-doc-as-truth (Tiptap JSON blob) in encrypted SQLite + derived block/link/FTS projections | One-row-per-block (N-query reassembly); loose `.md` files (no block identity) |
| **Editor** | Tiptap (ProseMirror), custom nodes, JSON-Schema validation before persist | Lexical (loses PM↔storage symmetry); Slate (too much bespoke work) |
| **Block/task ordering** | Fractional index (`order_key`, LexoRank-style), O(1) reorder, shared blocks+tasks | Integer positions (renumber on reorder) |
| **Task model** | Things-style derived buckets; start(hides)/deadline(doesn't)/reminder(alert) split | Stored-state buckets; OmniFocus defer-centric model |
| **Recurrence** | `rrule` crate (RFC-5545), materialize-on-completion, `fixed`/`after_completion` (`every`/`every!`) | Pre-expanding infinite series |
| **Reminders** | First-class polymorphic table (task/note/meeting/standalone); UTC `fire_at` + IANA `tz` | Reminder as a task column; frozen UTC offset (DST bug) |
| **Notification scheduling** | Dual-layer: Tokio timer-wheel (authoritative) + OS one-shot handoff (macOS UNCalendar / Win ScheduledToast, 14-day horizon) + launch/wake catch-up sweep; Linux honest capability report | Relying on `tauri-plugin-notification` scheduling on desktop; OS relaunch-quit-app (later) |
| **NL entry** | Hybrid Rust `app-nlp`: grammar first, resident-Qwen3 LLM fallback on low confidence; never invent dates | LLM-only (latency, hallucinated dates) |
| **STT** | whisper.cpp portable default (two-pass); Parakeet TDT v3 opt-in "Turbo (English)" ONNX; Apple SpeechTranscriber native macOS | Parakeet as default (breaks 99-lang/portability) |
| **LLM** | Qwen3 via llama.cpp/GGUF, tiers 4B/8B/14B, MLX on Apple Silicon; GBNF-constrained JSON, one repair then deterministic fallback | Cloud APIs; unconstrained decoding |
| **Embeddings** | EmbeddingGemma-300M or bge-base, Matryoshka 256-dim + int8; `embed_model` per chunk | Full 768-dim f32 (disk bloat); silent model swaps |
| **Vector index** | sqlite-vec *inside* SQLCipher | FAISS/usearch (fractures single encrypted store) — revisit >500k chunks |
| **Search** | Hybrid FTS5 (BM25) ∪ sqlite-vec (KNN) fused by RRF, no score normalization; async semantic re-fuse | Pure FTS or pure vector; score normalization |
| **RAG** | Retrieve → RRF → optional bge-reranker → grounded prompt → constrained AnswerV1 → verify every citation → `unanswered:true` if none | Ungrounded synthesis; hallucinated citations |
| **Knowledge model** | Universal `entity` spine + per-type detail + one polymorphic `link` table; directed edges, bidirectionality derived on read | Pure node/EAV (weak integrity); pure per-type (UNION hell); dual-write links |
| **Tags** | First-class typed entities; supertag-lite optional `schema_json` | Folders-as-tags; Notion up-front rigidity |
| **Storage layer** | Direct `rusqlite` + SQLCipher in the `storage` crate; WebView never sees SQL/FS | `tauri-plugin-sql` (exposes SQL to frontend) |
| **Encryption** | Whole-DB SQLCipher; key in Keychain/Credential Manager/Secret Service; content-addressed files | Per-record keys (sync-era only) |
| **Sync seam** | Op-log + HLC + UUIDv7/ULID substrate now (dormant `sync-core`); tables = materialized projection | Autoincrement IDs + wall-clock `updated_at` (migration trap) |
| **Future sync** | Central blind relay, compress-then-encrypt over op ranges, XChaCha20-Poly1305, per-notebook keys; encrypt metadata | P2P (ops/trust burden); per-op encryption (kills compression); plaintext metadata |
| **Future note CRDT** | Loro (Rust-native movable-tree/list) — re-encode, not re-model | Yjs-in-JS as source of truth (moves authority to WebView) |
| **Desktop shell** | Tauri 2; plugins: single-instance (first), global-shortcut, deep-link, autostart, notification, updater, fs (scoped), dialog, os, process | `tauri-plugin-sql`; sidecars for whisper/llama |
| **Windows** | Two windows (`main` chrome + `capture` frameless, startup-created, toggled) + tray; dynamic macOS activation policy (Accessory↔Regular) | Single reconfigured window; create-panel-on-hotkey (too slow) |
| **AI/inference placement** | whisper.cpp + llama.cpp in-process (Rust-linked crates) | Sidecar binaries (notarization pitfalls, IPC latency) |
| **Packaging** | macOS DMG+notarize+staple, Windows MSI+Authenticode, Linux AppImage+Flatpak; updater with separate signing key, self-hosted `latest.json`, consent-gated | — |

---

## 10. Open Questions Requiring Prototyping

1. **Tiptap doc size & save latency at scale.** How large can a single `doc_json` grow (long daily notes, embedded transcripts) before parse→block-projection→FTS on save exceeds an acceptable budget? Prototype the save pipeline on a 50–100k-word note; measure. Decide whether very large notes need block-granular partial re-projection rather than whole-doc walk.

2. **sqlite-vec performance envelope.** Confirm <50ms KNN at realistic personal scale (target 100k chunks, 256-dim int8) inside an *encrypted* SQLCipher DB — SQLCipher's page encryption adds I/O overhead the sqlite-vec benchmarks don't include. Find the chunk count where brute-force/partitioned search degrades and re-evaluate the ~500k usearch threshold.

3. **Dual-layer scheduler de-dup correctness across sleep/wake/timezone-change.** Build the Layer-A/Layer-B state machine and adversarially test: laptop sleeps across a `fire_at`, DST transition, manual clock change, force-quit then relaunch. Verify no double-fire and no silent drop, and that `os_handle` cancellation is reliable on both macOS and Windows.

4. **Windows ScheduledToast constraints in practice.** Validate one-shot-only + uneditable + AppUserModelID registration end-to-end for closed-app delivery, and confirm the Layer-A cancel/re-register churn on frequent reminder edits stays within any rate limits. Determine the real UX floor for Linux (login-item helper vs "app must be running").

5. **NL grammar coverage vs LLM-fallback rate.** Build the `app-nlp` grammar against a corpus of real quick-entry strings; measure what fraction falls through to the LLM and the fallback's latency/accuracy. Tune the confidence threshold. Confirm the "never invent a date" guarantee holds under schema-constrained decoding.

6. **Whisper→artifact→entity-spine INDEXING throughput.** End-to-end: does a 60-minute meeting's final transcription + Qwen3 artifact generation + chunk/embed/index into the spine complete within an acceptable post-meeting window on Tier-1 (4B/8GB) hardware without blocking the UI? Identify where to parallelize vs queue.

7. **GBNF grammar constraint quality for MeetingArtifactV1/AnswerV1 on small models.** Measure how often Qwen3-4B/8B produce valid structured output first-pass vs needing the one repair vs hitting deterministic fallback, and whether evidence-segment IDs are faithfully grounded (not fabricated). This gates the "evidence-carrying intelligence" promise.

8. **Op-log write amplification in v1.** Every mutation writing both a projection update *and* an `entity_op` doubles write volume. Prototype to confirm the dormant-seam overhead is negligible for single-device use (typical note editing, bulk imports) and that projection rebuild-from-log is fast enough to be a viable crash-recovery path.

9. **macOS dynamic activation-policy switching UX.** Confirm Accessory↔Regular flips (idle panel/tray ↔ main/meeting window) are visually clean (no Dock-icon flicker, correct Cmd-Tab behavior) and that notification permissions from a proper signed `.app` bundle survive the accessory mode.

10. **Matryoshka-256 + int8 recall on personal corpora.** Measure retrieval recall loss vs full 768-dim f32 on realistic mixed note+transcript+task content. Confirm hybrid RRF + optional reranker recovers any recall the dimension truncation costs, so the disk/RAM savings don't degrade "ask your notes" answer quality.

11. **Loro re-encode path viability.** Validate that a note stored today as block-IDed Tiptap JSON can be losslessly re-encoded into a Loro movable-tree document later — the assumption underpinning "defer CRDT without a rewrite." A small spike now de-risks the entire future-sync bet.

---

## Appendix — Consolidated Sources

**Notebook/editor:** [Notion data model](https://www.notion.com/blog/data-model-behind-notion) · [Obsidian vs Logseq](https://itsfoss.com/comparison/obsidian-vs-logseq/) · [AFFiNE/AppFlowy/Anytype](https://affine.pro/blog/affine-vs-appflowy-vs-anytype) · [Rich text editors 2025](https://liveblocks.io/blog/which-rich-text-editor-framework-should-you-choose-in-2025) · [Tiptap schema/JSON](https://tiptap.dev/docs/editor/core-concepts/schema) · [reflect-open](https://github.com/team-reflect/reflect-open) · [SQLite FTS5](https://sqlite.org/fts5.html)

**Tasks/reminders:** [Things scheduling](https://culturedcode.com/things/support/articles/2803579/) · [Things lists](https://culturedcode.com/things/support/articles/4001304/) · [OmniFocus perspectives](https://support.omnigroup.com/documentation/omnifocus/universal/4.3.3/en/perspectives/) · [Todoist recurring dates](https://www.todoist.com/help/articles/introduction-to-recurring-dates-YUYVJJAV) · [Todoist dates & time](https://www.todoist.com/help/articles/introduction-to-dates-and-time-q7VobO) · [rrule crate](https://crates.io/crates/rrule) · [iCalendar RRULE](https://www.kanzaki.com/docs/ical/rrule.html) · [TaskNotes recurrence](https://tasknotes.dev/features/recurring-tasks/) · [Windows ScheduledToast](https://learn.microsoft.com/en-us/uwp/api/windows.ui.notifications.scheduledtoastnotification.-ctor) · [Apple local notifications](https://developer.apple.com/library/content/documentation/NetworkingInternet/Conceptual/RemoteNotificationsPG/SchedulingandHandlingLocalNotifications.html)

**Sync/CRDT/encryption:** [Yjs vs Automerge vs Loro 2026](https://www.pkgpulse.com/guides/yjs-vs-automerge-vs-loro-crdt-libraries-2026) · [Loro](https://github.com/loro-dev/loro) · [Automerge docs.rs](https://docs.rs/automerge) · [Kerkour: CRDT+E2EE notes](https://kerkour.com/crdt-end-to-end-encryption-research-notes) · [Ink & Switch Keyhive](https://www.inkandswitch.com/project/keyhive/) · [BeeKEM explainer](https://meri.garden/posts/a-deep-dive-explainer-on-beekem-protocol/) · [Obsidian Sync security](https://obsidian.md/sync) · [SQLCipher/Zetetic](https://www.zetetic.net/sqlcipher/)

**On-device AI:** [Best open-source STT 2026](https://northflank.com/blog/best-open-source-speech-to-text-stt-model-in-2026-benchmarks) · [Parakeet vs Whisper](https://openwhispr.com/blog/parakeet-vs-whisper-vs-nemotron) · [Moonshine](https://github.com/moonshine-ai/moonshine) · [Qwen3 llama.cpp](https://qwen.readthedocs.io/en/latest/run_locally/llama.cpp.html) · [sqlite-vec stable](https://alexgarcia.xyz/blog/2024/sqlite-vec-stable-release/index.html) · [sqlite-vec vs FAISS/usearch](https://github.com/asg017/sqlite-vec/issues/94) · [Best embedding models RAG 2026](https://milvus.io/blog/choose-embedding-model-rag-2026.md) · [ObsidianRAG](https://github.com/Vasallo94/ObsidianRAG)

**Tauri desktop:** [Global Shortcut](https://v2.tauri.app/plugin/global-shortcut/) · [deep-link plugin](https://github.com/FabianLars/tauri-plugin-deep-link) · [Capabilities](https://v2.tauri.app/security/capabilities/) · [CSP](https://v2.tauri.app/security/csp/) · [Notification scheduling issue #2141](https://github.com/tauri-apps/plugins-workspace/issues/2141) · [macOS signing](https://v2.tauri.app/distribute/sign/macos/) · [ExternalBin notarization #11992](https://github.com/tauri-apps/tauri/issues/11992) · [Menubar app guide](https://dev.to/hiyoyok/complete-guide-to-building-a-macos-menu-bar-app-with-tauri-v2-aji)

**Unified knowledge/search:** [Logseq DB version](https://github.com/logseq/docs/blob/master/db-version.md) · [Tana supertags](https://aiproductivity.ai/guides/tana-supertags-guide/) · [Capacities object types](https://docs.capacities.io/reference/object-properties) · [Hybrid FTS5+sqlite-vec+RRF (Simon Willison)](https://simonwillison.net/2024/Oct/4/hybrid-full-text-search-and-vector-search-with-sqlite/) · [sqlite-rag](https://github.com/sqliteai/sqlite-rag) · [Command Palette UX](https://uxpatterns.dev/patterns/advanced/command-palette)

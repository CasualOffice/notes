# Casual Note — Data Model & Storage Schema

*The authoritative physical schema, storage layout, journal formats, and AI/import-export JSON contracts for the unified local store.*

**Status:** Canonical for physical schema. Downstream code and migrations MUST match this document. Where this document and the Design Foundation disagree, the Foundation wins and this document is corrected. This document owns **columns, types, keys, indexes, constraints, on-disk layout, journal record formats, and the MeetingArtifactV1 / AnswerV1 / ParsedEntry / import-export JSON contracts**. It does **not** own product rationale (PRD), subsystem runtime designs (HLD), crate decomposition (Architecture), or per-feature behavior (Feature Specs).

---

## 1. Scope & Design Rules

This document specifies the single encrypted SQLite (SQLCipher) store that is the source of truth for all four pillars — Notes, Reminders, Tasks, Meetings — plus the cross-cutting substrate (links, chunks, embeddings, models, jobs, settings, op-log). All statements are given in SQLite dialect.

**Physical-schema invariants (non-negotiable):**

1. **Universal spine.** Every addressable thing is one row in `entity`. Per-type detail lives in a strongly-typed detail table keyed 1:1 by `entity_id`. Nothing polymorphic is duplicated into detail tables.
2. **One graph table.** All edges — wikilinks, backlinks (derived on read), mentions, tags, meeting provenance, reminder targets, parentage — live in `link`. Bidirectionality is a read-time query, never dual-written.
3. **Derived tables are rebuildable.** `block`, `link` (for wikilinks/tags/mentions parsed from docs), FTS, `chunk`, and `embedding` are projections. Truth is `note.doc_json`, the detail tables, and the `entity_op` log. A full rebuild from truth must reproduce them exactly.
4. **IDs.** Every `entity.id` is a **UUIDv7** (time-ordered, stored as 16-byte `BLOB`). Op-log rows use **ULID** `op_id`. Block IDs are short nanoid strings living inside `doc_json` and mirrored into `block.block_id`.
5. **Time.** Instants are `INTEGER` epoch-milliseconds UTC. Calendar-day fields (`daily_date`, `start_on`, `deadline_on`) are `TEXT` `YYYY-MM-DD` (local wall date, no zone). Reminder fire times are absolute UTC ms **plus** an IANA `tz` string so DST math is reconstructable.
6. **Soft delete.** Mutable entities carry `deleted_at` (nullable epoch-ms). Queries filter `deleted_at IS NULL`. Tombstones persist for the dormant sync seam; a compaction job hard-deletes after a retention window.
7. **Concurrency seam.** Every mutable row carries `hlc` (hybrid logical clock, `TEXT`, sortable) and `updated_at`. Structured entities are per-field LWW-by-HLC; tags/links are OR-Set. Note bodies stay block-IDed for a future Loro re-encode. None of this is *active* in v1 — it is recorded, not reconciled.

**Type conventions used throughout:** `BLOB16` = 16-byte UUIDv7; `TS` = `INTEGER` epoch-ms UTC; `DAY` = `TEXT` `YYYY-MM-DD`; `JSON` = `TEXT` validated at the Rust boundary before persist; `HLC` = `TEXT` (e.g. `"<physical_ms>:<counter>:<node>"`). Booleans are `INTEGER` `0/1`. `order_key` is `TEXT` (LexoRank-style fractional index).

---

## 2. Entity-Relationship Overview

```
                                   ┌───────────────────────────┐
                                   │          entity           │  universal spine
                                   │ id(uuidv7) kind daily_date │  (one row per thing)
                                   │ title created updated hlc  │
                                   │ deleted_at                 │
                                   └────┬───────────────────────┘
        1:1 detail (kind-keyed)         │
   ┌────────────┬────────────┬──────────┼───────────┬───────────┬────────────┬───────────┐
   ▼            ▼            ▼           ▼           ▼           ▼            ▼           ▼
 note        notebook      tag        task       project      area       reminder    session
(doc_json)  (parent tree) (schema)  (start/dl)  (area_id)   (bucket)  (target poly) (state m/c)
   │                                   │                                   │           │
   │ projected                         │ checklist_item                    │ recurrence│ audio_track
   ▼ on save                           ▼ (flat, ordered)                   ▼ _rule     ▼
 block ◄───────────────┐          subtask(parent_task_id)          recurrence_rule  transcript_segment
(block_id, node_type)  │                                                             │
   │                   │                                                             ▼
   │                   │                                                          artifact (MeetingArtifactV1)
   │                   │                                                             │
   ▼                   │                                                             ▼
 attachment            │                                                        action_item ──┐
(sha256, note/block)   │                                                        (bridge)       │ spawned_from
                       │                                                                       ▼
   ┌───────────────────┴───────────────────────────────────────────────────────────────┐  task
   │                                    link                                             │  (edge carries
   │  src_entity, dst_entity, rel, src_block_id, evidence_segment_ids[], hlc             │   evidence + owner)
   │  rel ∈ {wikilink,backlink,mention,tagged,spawned_from,about,attends,                │
   │         action_item_of,reminds,child_of}                                            │
   └─────────────────────────────────────────────────────────────────────────────────────┘

  person/entity ──(attends / owner)──►  session / action_item / link
  chunk ──(1:1)──►  embedding (sqlite-vec)          fts_note / fts_task / fts_transcript / fts_chunk (FTS5)
  model_installation      job (idle batch)      setting (kv)      suggestion (reversible AI edits)
  entity_op  ── append-only op-log; every table above is a materialized projection of it
```

**Cardinalities of note:** `entity 1:1 <detail>`; `note 1:N block`; `note/block 1:N attachment`; `session 1:N audio_track`; `session 1:N transcript_segment`; `session 1:1 artifact` (immutable-per-generation, versioned); `artifact 1:N action_item`; `action_item 0:1 task` (bridge, via `link.spawned_from`); `chunk 1:1 embedding`; everything-to-everything via `link`.

---

## 3. The Universal Spine

### 3.1 `entity`

```sql
CREATE TABLE entity (
  id           BLOB    PRIMARY KEY,           -- UUIDv7, 16 bytes
  kind         TEXT    NOT NULL,              -- see CHECK below
  title        TEXT,                          -- denormalized display title (nullable for blocks-as-entities? no: blocks not entities)
  daily_date   TEXT,                          -- DAY; threads quick-capture onto a date (nullable)
  created_at   INTEGER NOT NULL,              -- TS
  updated_at   INTEGER NOT NULL,              -- TS
  hlc          TEXT    NOT NULL,              -- HLC of last mutation
  deleted_at   INTEGER,                       -- TS; NULL = live
  CHECK (kind IN ('note','notebook','tag','task','project','area',
                  'reminder','session','artifact','action_item',
                  'person','recurrence_rule'))
);
CREATE INDEX idx_entity_kind        ON entity(kind) WHERE deleted_at IS NULL;
CREATE INDEX idx_entity_daily       ON entity(daily_date) WHERE daily_date IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_entity_updated     ON entity(updated_at);
CREATE INDEX idx_entity_kind_upd    ON entity(kind, updated_at) WHERE deleted_at IS NULL;
```

`block`, `attachment`, `audio_track`, `transcript_segment`, `checklist_item`, `chunk`, `embedding`, `link`, `entity_op`, `model_installation`, `job`, `setting`, `suggestion` are **not** entities (not link-addressable as first-class nodes); they use their own PKs. `block` is addressable by `block_id` but lives under a note, so it is a projected sub-node, not a spine row.

### 3.2 Detail tables (kind-keyed 1:1)

Each detail table's PK is `entity_id BLOB` with `FOREIGN KEY(entity_id) REFERENCES entity(id) ON DELETE CASCADE`. Detail rows never carry `title`/`created_at`/`hlc` — those live on the spine.

---

## 4. Knowledge Pillar

### 4.1 `note`

```sql
CREATE TABLE note (
  entity_id          BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  notebook_id        BLOB REFERENCES entity(id),        -- kind='notebook', NULL = inbox/loose
  doc_json           TEXT NOT NULL,                     -- ProseMirror/Tiptap JSON = SOURCE OF TRUTH
  doc_schema_version INTEGER NOT NULL DEFAULT 1,        -- editor schema migration marker
  daily_date         TEXT,                              -- DAY; set iff this note is a daily note
  is_pinned          INTEGER NOT NULL DEFAULT 0,
  content_hash       TEXT NOT NULL,                     -- BLAKE3 of doc_json; gates re-projection/re-embed
  word_count         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_note_notebook ON note(notebook_id);
CREATE UNIQUE INDEX idx_note_daily ON note(daily_date) WHERE daily_date IS NOT NULL;
```

`doc_json` is schema-validated against the active Tiptap schema before persist. `daily_date` uniqueness enforces one daily note per calendar day. `content_hash` is the gate: on save, if unchanged, skip block-projection / link-extraction / FTS / embedding work.

### 4.2 `block` (projected from `doc_json`)

```sql
CREATE TABLE block (
  block_id     TEXT NOT NULL,          -- stable nanoid from node attrs
  note_id      BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  node_type    TEXT NOT NULL,          -- paragraph|heading|todo|code|table|callout|embed|transcript_segment|...
  seq          INTEGER NOT NULL,       -- document order within the note
  depth        INTEGER NOT NULL DEFAULT 0,
  text_content TEXT,                   -- flattened plain text for FTS/backlink targets
  attrs_json   TEXT,                   -- node-specific attrs (heading level, todo checked, lang, session_id...)
  order_key    TEXT NOT NULL,          -- fractional index (LexoRank) for reorder
  PRIMARY KEY (note_id, block_id)
);
CREATE INDEX idx_block_note_seq ON block(note_id, seq);
CREATE INDEX idx_block_type     ON block(node_type);
```

Rebuilt on every save where `content_hash` changed: delete-and-reinsert per note inside the save transaction. `transcript_segment` blocks carry `attrs_json.session_id` + `attrs_json.segment_id` so a transcript embedded in a note points back to `transcript_segment`.

### 4.3 `notebook` (folder tree)

```sql
CREATE TABLE notebook (
  entity_id   BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  parent_id   BLOB REFERENCES entity(id),   -- adjacency list; NULL = root
  order_key   TEXT NOT NULL,                -- sibling ordering
  icon        TEXT,
  color       TEXT
);
CREATE INDEX idx_notebook_parent ON notebook(parent_id);
```

Nested via adjacency list. A cycle-check runs at the Rust `notes` boundary on reparent. Tree depth is unbounded but UI-collapsed.

### 4.4 `tag` (first-class typed entity, supertag-lite)

```sql
CREATE TABLE tag (
  entity_id   BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,                -- canonical, case-folded for matching
  display     TEXT NOT NULL,                -- original casing
  color       TEXT,
  schema_json TEXT                          -- optional supertag field schema (NULL = plain tag)
);
CREATE UNIQUE INDEX idx_tag_name ON tag(name) WHERE entity_id IN (SELECT id FROM entity WHERE deleted_at IS NULL);
```

A plain tagging is `link(rel='tagged')` from a note to a tag entity. A `schema_json` tag lends optional structured fields to tagged notes (rendered by the editor); the field *values* are stored in the tagging edge's `data_json` (see `link`).

### 4.5 `attachment` (content-addressed)

```sql
CREATE TABLE attachment (
  id           BLOB PRIMARY KEY,             -- UUIDv7 (row id; file identity is sha256)
  owner_id     BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,  -- note or session
  block_id     TEXT,                         -- optional precise block anchor
  sha256       TEXT NOT NULL,                -- content address = filename on disk
  filename     TEXT NOT NULL,                -- original display name
  mime         TEXT NOT NULL,
  byte_size    INTEGER NOT NULL,
  created_at   INTEGER NOT NULL,
  deleted_at   INTEGER
);
CREATE INDEX idx_attach_owner  ON attachment(owner_id);
CREATE INDEX idx_attach_sha    ON attachment(sha256);
```

Bytes live under `files/<sha256[0:2]>/<sha256>` (§12). De-dup is automatic: same bytes → same `sha256` → one file, many `attachment` rows. A reference-count sweep (via `job`) garbage-collects orphaned blobs.

---

## 5. The Polymorphic Link Graph

### 5.1 `link`

```sql
CREATE TABLE link (
  id                   BLOB PRIMARY KEY,      -- UUIDv7
  src_entity           BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  dst_entity           BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  rel                  TEXT NOT NULL,
  src_block_id         TEXT,                  -- precise origin block inside src (nullable)
  dst_block_id         TEXT,                  -- precise target block inside dst (nullable)
  evidence_segment_ids TEXT,                  -- JSON array of transcript_segment ids (meeting provenance)
  data_json            TEXT,                  -- edge payload: supertag field values, mention offsets, etc.
  origin               TEXT NOT NULL DEFAULT 'user',  -- user|projected|ai_suggested|meeting
  created_at           INTEGER NOT NULL,
  hlc                  TEXT NOT NULL,
  deleted_at           INTEGER,
  CHECK (rel IN ('wikilink','backlink','mention','tagged','spawned_from',
                 'about','attends','action_item_of','reminds','child_of'))
);
CREATE INDEX idx_link_src ON link(src_entity, rel) WHERE deleted_at IS NULL;
CREATE INDEX idx_link_dst ON link(dst_entity, rel) WHERE deleted_at IS NULL;
CREATE INDEX idx_link_rel ON link(rel) WHERE deleted_at IS NULL;
CREATE UNIQUE INDEX idx_link_uniq ON link(src_entity, dst_entity, rel, src_block_id)
  WHERE deleted_at IS NULL;
```

**Backlinks are a read, not a write.** To render backlinks for entity `X`: `SELECT * FROM link WHERE dst_entity = X AND rel IN ('wikilink','mention') AND deleted_at IS NULL`. The `rel='backlink'` value exists only for explicit user-authored reverse edges; ordinary reverse rendering never materializes a row.

**Projected vs authored.** `origin='projected'` edges (wikilinks/tags/mentions parsed from `doc_json`) are rebuilt on save: delete all `projected` edges with `src_entity = note`, re-extract, re-insert, inside the save transaction. `origin='user'` / `'meeting'` / `'ai_suggested'` edges are never touched by projection.

**Unlinked mentions** are *not* rows — they are a query-time FTS match of an entity title against note text with no corresponding `link` row, surfaced in the backlinks panel as "unlinked mentions."

---

## 6. Planning Pillar

### 6.1 `area`

```sql
CREATE TABLE area (
  entity_id BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  order_key TEXT NOT NULL,
  icon      TEXT
);
```

### 6.2 `project`

```sql
CREATE TABLE project (
  entity_id   BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  area_id     BLOB REFERENCES entity(id),      -- kind='area', NULL = loose project
  note_id     BLOB REFERENCES entity(id),      -- optional backing note (kind='note')
  status      TEXT NOT NULL DEFAULT 'active',  -- active|completed|canceled
  start_on    TEXT,                            -- DAY (projects may be dated)
  deadline_on TEXT,                            -- DAY
  completed_at INTEGER,                        -- TS
  order_key   TEXT NOT NULL
);
CREATE INDEX idx_project_area ON project(area_id) WHERE status='active';
```

### 6.3 `task`

```sql
CREATE TABLE task (
  entity_id     BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  project_id    BLOB REFERENCES entity(id),    -- kind='project'
  area_id       BLOB REFERENCES entity(id),    -- kind='area' (loose task in an area)
  heading_id    BLOB REFERENCES heading(id),   -- section within a project
  parent_task_id BLOB REFERENCES entity(id),   -- nested subtask (kind='task')
  notes_md      TEXT,                           -- lightweight task body (markdown)
  status        TEXT NOT NULL DEFAULT 'open',   -- open|completed|canceled
  priority      INTEGER NOT NULL DEFAULT 0,     -- 0..3 (from inline !priority)
  someday       INTEGER NOT NULL DEFAULT 0,     -- 1 = Someday bucket (deferred; hidden from Today/Upcoming/Anytime)
  start_on      TEXT,                            -- DAY: WHEN/scheduled — HIDES task until this date
  deadline_on   TEXT,                            -- DAY: due — does NOT hide
  completed_at  INTEGER,                         -- TS
  order_key     TEXT NOT NULL,                   -- fractional index for O(1) drag-reorder
  assignee_person_id BLOB REFERENCES entity(id), -- kind='person'; owner carried from a promoted action item (only if extracted from evidence)
  recurrence_id BLOB REFERENCES entity(id)       -- kind='recurrence_rule' (template task)
);
CREATE INDEX idx_task_project ON task(project_id) WHERE status='open';
CREATE INDEX idx_task_area    ON task(area_id)    WHERE status='open';
CREATE INDEX idx_task_parent  ON task(parent_task_id);
CREATE INDEX idx_task_start    ON task(start_on)    WHERE status='open';
CREATE INDEX idx_task_deadline ON task(deadline_on) WHERE status='open';
```

**Buckets are derived queries, never stored** (Things model):

| Bucket | Predicate (all with `status='open' AND deleted_at IS NULL`) |
|---|---|
| **Today** | `(start_on <= :today` OR a reminder fires today OR `deadline_on <= :today) AND someday = 0` |
| **Upcoming** | `(start_on > :today` OR `deadline_on > :today) AND someday = 0` |
| **Anytime** | `start_on IS NULL AND deadline_on IS NULL AND someday = 0` |
| **Someday** | `someday = 1` (explicit deferred flag; hidden from Today/Upcoming/Anytime until activated) |

`start_on` **hides** (When); `deadline_on` **shows a due badge but never hides**; alert timing is a separate `reminder`. These three concepts are never conflated in one column.

### 6.4 `heading` (project section)

```sql
CREATE TABLE heading (
  id         BLOB PRIMARY KEY,               -- UUIDv7 (not a spine entity)
  project_id BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  title      TEXT NOT NULL,
  order_key  TEXT NOT NULL
);
CREATE INDEX idx_heading_project ON heading(project_id);
```

### 6.5 `checklist_item` (flat, ordered — distinct from nested subtasks)

```sql
CREATE TABLE checklist_item (
  id        BLOB PRIMARY KEY,
  task_id   BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  text      TEXT NOT NULL,
  checked   INTEGER NOT NULL DEFAULT 0,
  order_key TEXT NOT NULL
);
CREATE INDEX idx_checklist_task ON checklist_item(task_id);
```

Two distinct sub-structures per the Foundation: **`checklist_item`** (flat, lightweight, inside one task) and **`parent_task_id`** (a full nested subtask that is itself a `task` entity).

---

## 7. Reminders & Recurrence

### 7.1 `reminder` (first-class polymorphic)

```sql
CREATE TABLE reminder (
  entity_id     BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  target_kind   TEXT,                           -- task|note|session|NULL(standalone)
  target_id     BLOB REFERENCES entity(id),     -- polymorphic target (NULL = standalone)
  target_block_id TEXT,                          -- reminder anchored to a specific block
  fire_at       INTEGER NOT NULL,               -- TS absolute UTC ms (authoritative instant)
  tz            TEXT NOT NULL,                   -- IANA zone, e.g. 'America/New_York' (DST-safe reconstruction)
  state         TEXT NOT NULL DEFAULT 'pending',-- pending|fired|snoozed|missed|dismissed|canceled
  snoozed_until INTEGER,                          -- TS
  os_handle     TEXT,                             -- OS notification identifier (Layer B), NULL if none
  os_layer      TEXT,                             -- 'uncalendar'|'toast'|NULL (Linux)
  recurrence_id BLOB REFERENCES entity(id),       -- kind='recurrence_rule'
  body          TEXT,                             -- notification text override
  created_at    INTEGER NOT NULL,
  CHECK (state IN ('pending','fired','snoozed','missed','dismissed','canceled')),
  CHECK (target_kind IN ('task','note','session') OR target_kind IS NULL)
);
CREATE INDEX idx_reminder_fire  ON reminder(fire_at) WHERE state IN ('pending','snoozed');
CREATE INDEX idx_reminder_target ON reminder(target_kind, target_id);
CREATE INDEX idx_reminder_state ON reminder(state);
```

The `link(rel='reminds')` edge mirrors `target_*` for graph traversal; `target_*` columns are the authoritative pointer (fast scheduler lookups without a graph join). The dual-layer scheduler (HLD-owned) reads `idx_reminder_fire` to (A) rebuild the in-memory timer-wheel on boot and (B) register OS one-shots within the 14-day horizon, writing `os_handle`/`os_layer`. Delivery is gated on `state`; the catch-up sweep is `state='pending' AND fire_at < now`. **Linux:** `os_layer` is `NULL` — capability reported honestly; only Layer A fires, and only while running.

### 7.2 `recurrence_rule`

```sql
CREATE TABLE recurrence_rule (
  entity_id          BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  rrule              TEXT NOT NULL,               -- RFC-5545 RRULE string
  mode               TEXT NOT NULL,               -- 'fixed' (every) | 'after_completion' (every!)
  next_scheduled_on  TEXT,                         -- DAY: materialized next instance
  until_on           TEXT,                         -- DAY (optional bound)
  count_remaining    INTEGER,                      -- optional COUNT bound, decremented
  complete_instances TEXT,                         -- JSON array of completed instance DAYs
  CHECK (mode IN ('fixed','after_completion'))
);
```

**Materialize-on-completion** (not pre-expansion): the store holds a *template* task/reminder plus exactly one next materialized instance. On completion, the `rrule` crate computes the next date; `fixed` advances from the scheduled date (Todoist `every`), `after_completion` advances from the completion date (Todoist `every!`). `next_scheduled_on` and `complete_instances` track state; `count_remaining`/`until_on` terminate the series.

---

## 8. Meeting Pillar (inherited, carried forward)

### 8.1 `session` (Meeting — owns the state machine)

```sql
CREATE TABLE session (
  entity_id      BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  state          TEXT NOT NULL,                  -- state-machine value (see CHECK)
  note_id        BLOB REFERENCES entity(id),     -- the note this meeting becomes (kind='note')
  started_at     INTEGER,                         -- TS
  ended_at       INTEGER,                         -- TS
  duration_ms    INTEGER,
  capture_source TEXT,                            -- JSON: {app_audio:bool, mic:bool, app_bundle_id, ...}
  platform       TEXT NOT NULL,                   -- macos|windows|linux
  degraded_reason TEXT,                            -- populated in DEGRADED/FAILED
  journal_path   TEXT,                             -- relative path to NDJSON session journal
  CHECK (state IN ('NEW','PREFLIGHT','READY','RECORDING','PAUSED','STOPPING',
                   'CAPTURED','FINAL_TRANSCRIBING','GENERATING','INDEXING',
                   'COMPLETE','DEGRADED','FAILED','RECOVERING'))
);
CREATE INDEX idx_session_state ON session(state);
CREATE INDEX idx_session_started ON session(started_at);
```

The LLM never owns recording state. The **INDEXING** stage writes into the unified spine: creates/links the `note`, populates `chunk`/`embedding`, resolves `action_item → task` suggestions, and writes provenance `link` rows.

### 8.2 `audio_track`

```sql
CREATE TABLE audio_track (
  id          BLOB PRIMARY KEY,
  session_id  BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  source_kind TEXT NOT NULL,                    -- 'app_audio' | 'mic'
  source_label TEXT,                             -- app name / device name
  sample_rate INTEGER NOT NULL,                  -- native captured rate
  channels    INTEGER NOT NULL,
  audio_sha256 TEXT,                             -- content-addressed captured PCM/compressed file
  byte_size   INTEGER
);
CREATE INDEX idx_track_session ON audio_track(session_id);
```

Raw PCM never crosses the WebView and is never serialized to JSON; the persisted artifact is a content-addressed compressed file under `files/`.

### 8.3 `transcript_segment` (the atomic unit of evidence)

```sql
CREATE TABLE transcript_segment (
  id          BLOB PRIMARY KEY,                 -- referenced by evidence_segment_ids
  session_id  BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  track_id    BLOB REFERENCES audio_track(id),
  seq         INTEGER NOT NULL,                 -- order within session
  t_start_ms  INTEGER NOT NULL,                 -- ms from session start
  t_end_ms    INTEGER NOT NULL,
  speaker     TEXT,                             -- speaker turn label (diarization, optional)
  person_id   BLOB REFERENCES entity(id),       -- resolved person (kind='person'), optional
  text        TEXT NOT NULL,
  pass        TEXT NOT NULL DEFAULT 'final',    -- 'live' | 'final' (two-pass STT)
  confidence  REAL
);
CREATE INDEX idx_seg_session_seq ON transcript_segment(session_id, seq);
CREATE INDEX idx_seg_time        ON transcript_segment(session_id, t_start_ms);
```

`live`-pass rows are superseded by `final`-pass rows keyed by overlapping time window; final is authoritative for evidence citation. Every `evidence_segment_ids` reference in `link` and in artifacts resolves to a row here.

### 8.4 `artifact` (MeetingArtifactV1 — immutable-per-generation)

```sql
CREATE TABLE artifact (
  entity_id      BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  session_id     BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  schema_version INTEGER NOT NULL DEFAULT 1,     -- MeetingArtifactV1
  generation     INTEGER NOT NULL DEFAULT 1,     -- regeneration counter; old rows retained
  is_current     INTEGER NOT NULL DEFAULT 1,
  llm_model      TEXT NOT NULL,                  -- provenance (model id + quant)
  artifact_json  TEXT NOT NULL,                  -- full MeetingArtifactV1 (see §14)
  generated_at   INTEGER NOT NULL
);
CREATE INDEX idx_artifact_session ON artifact(session_id) WHERE is_current=1;
```

Immutable per generation: regenerating writes a new row (`generation+1`, `is_current=1`) and flips the prior to `is_current=0`. The JSON is the canonical structured artifact; `action_item` rows below are the *projected*, actionable extraction for the task bridge.

### 8.5 `action_item` (bridge into Task)

```sql
CREATE TABLE action_item (
  entity_id            BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  artifact_id          BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  session_id           BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  idx                  INTEGER NOT NULL,        -- position within artifact.action_items[]
  task_text            TEXT NOT NULL,
  owner_person_id      BLOB REFERENCES entity(id),  -- ONLY if model extracted from evidence
  owner_text           TEXT,                         -- raw extracted owner string
  due_date             TEXT,                          -- DAY; ONLY if extracted from evidence
  evidence_segment_ids TEXT NOT NULL,                 -- JSON array; every item MUST cite evidence
  promoted_task_id     BLOB REFERENCES entity(id),    -- kind='task' once user promotes (NULL until)
  status               TEXT NOT NULL DEFAULT 'suggested' -- suggested|promoted|dismissed
);
CREATE INDEX idx_action_artifact ON action_item(artifact_id);
CREATE INDEX idx_action_session  ON action_item(session_id);
```

**The bridge (Foundation §3, verbatim mapping):** when the user promotes an action item to a task:
1. create `entity(kind='task')` + `task` detail row;
2. `link(src=task, dst=session, rel='spawned_from', evidence_segment_ids=<copied>)`;
3. if discussed in a note block, `link(src=task, dst=block, rel='about')`;
4. carry `owner_person_id → task.assignee_person_id` and `due_date → task.deadline_on` **only if extracted from evidence** — never invented;
5. set `action_item.promoted_task_id` and `status='promoted'`.

Provenance rides the `link` edge, so it survives later task edits.

---

## 9. Cross-Cutting Entities

### 9.1 `person`

```sql
CREATE TABLE person (
  entity_id   BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  display     TEXT NOT NULL,
  canonical   TEXT NOT NULL,                    -- case-folded for @mention resolution
  aliases     TEXT,                             -- JSON array of alternative names
  email       TEXT,                             -- local only; no external sync
  avatar_sha256 TEXT
);
CREATE UNIQUE INDEX idx_person_canonical ON person(canonical);
```

`@mention` targets; `link(rel='attends')` to sessions; `owner_person_id` on action items. No external calendar/email sync (v1 non-goal).

### 9.2 `chunk` (source-agnostic retrieval unit)

```sql
CREATE TABLE chunk (
  id           BLOB PRIMARY KEY,               -- UUIDv7
  source_kind  TEXT NOT NULL,                  -- note_block|transcript_window|task|reminder
  source_id    BLOB NOT NULL,                  -- entity/segment id the chunk derives from
  source_block_id TEXT,                         -- for note_block
  seq          INTEGER NOT NULL,
  breadcrumb   TEXT,                            -- e.g. "Notebook / Note / Heading" carried into text
  text         TEXT NOT NULL,                  -- chunk text used for FTS + embedding
  t_start_ms   INTEGER,                        -- transcript time anchor (nullable)
  t_end_ms     INTEGER,
  token_count  INTEGER,
  content_hash TEXT NOT NULL,                  -- gates incremental re-embed
  updated_at   INTEGER NOT NULL
);
CREATE INDEX idx_chunk_source ON chunk(source_kind, source_id);
CREATE INDEX idx_chunk_hash   ON chunk(content_hash);
```

Chunking (per Foundation §4): notes by heading/block (~200–400 tokens, breadcrumb-carried); transcripts by VAD/speaker turn (~30–60s, time-anchored); tasks/reminders one chunk each. Everything unifies into one retrieval path.

### 9.3 `embedding` (1:1 with chunk; vectors via sqlite-vec)

```sql
-- Metadata row (plain table)
CREATE TABLE embedding (
  chunk_id     BLOB PRIMARY KEY REFERENCES chunk(id) ON DELETE CASCADE,
  embed_model  TEXT NOT NULL,                  -- provenance, e.g. 'embeddinggemma-300m@256d-int8'
  dims         INTEGER NOT NULL,               -- 256 (Matryoshka-truncated)
  created_at   INTEGER NOT NULL
);

-- Vector index: sqlite-vec virtual table INSIDE the SQLCipher store
CREATE VIRTUAL TABLE vec_chunk USING vec0(
  chunk_id  TEXT PRIMARY KEY,
  embedding FLOAT[256]                          -- int8-quantized at storage layer
);
```

sqlite-vec lives inside the encrypted store (no FAISS/usearch) — one encrypted file, transactional consistency with the source rows, one-file backup. `embed_model` + `dims` recorded per chunk so a model swap triggers a scoped re-embed of stale rows only.

### 9.4 `model_installation` (signed registry)

```sql
CREATE TABLE model_installation (
  id            BLOB PRIMARY KEY,
  role          TEXT NOT NULL,                 -- 'stt' | 'llm' | 'embedder' | 'reranker'
  family        TEXT NOT NULL,                 -- whisper|parakeet|qwen3|embeddinggemma|bge|...
  variant       TEXT NOT NULL,                 -- base|small|medium|4b|8b|14b|300m|...
  quant         TEXT,                          -- Q4_K_M|int8|... (nullable)
  file_sha256   TEXT NOT NULL,                 -- checksum from signed manifest
  manifest_sig  TEXT NOT NULL,                 -- signature over the manifest
  file_path     TEXT NOT NULL,                 -- relative to models/
  byte_size     INTEGER NOT NULL,
  source        TEXT NOT NULL,                 -- 'download' | 'usb_import'
  installed_at  INTEGER NOT NULL,
  is_active     INTEGER NOT NULL DEFAULT 0     -- selected for its role
);
CREATE INDEX idx_model_role ON model_installation(role) WHERE is_active=1;
```

### 9.5 `job` (idle-time batch work)

```sql
CREATE TABLE job (
  id           BLOB PRIMARY KEY,
  kind         TEXT NOT NULL,                  -- reindex|embed|auto_link|auto_tag|blob_gc|compact
  state        TEXT NOT NULL DEFAULT 'queued', -- queued|running|done|failed
  payload_json TEXT,
  priority     INTEGER NOT NULL DEFAULT 0,
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL
);
CREATE INDEX idx_job_state ON job(state, priority);
```

### 9.6 `suggestion` (reversible AI edits — never silent)

```sql
CREATE TABLE suggestion (
  id            BLOB PRIMARY KEY,
  kind          TEXT NOT NULL,                 -- auto_link|auto_tag|action_item|merge_person
  target_id     BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  proposed_json TEXT NOT NULL,                 -- the concrete proposed mutation
  citations_json TEXT,                          -- evidence backing the suggestion
  state         TEXT NOT NULL DEFAULT 'pending',-- pending|accepted|rejected|expired
  created_at    INTEGER NOT NULL,
  resolved_at   INTEGER
);
CREATE INDEX idx_suggestion_state ON suggestion(state);
```

Auto-link / auto-tag are cited, user-approved suggestion rows — never silent edits (Foundation §4).

### 9.7 `setting` (kv)

```sql
CREATE TABLE setting (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);
```

Holds hotkeys, autostart flag, selected model tiers, capability-report cache, `schema_version` mirror, and the offline/network-consent flags.

---

## 10. Full-Text Search (FTS5)

Per-source external-content FTS5 tables keep the index lean and rebuildable; a unified `fts_chunk` backs RAG retrieval.

```sql
CREATE VIRTUAL TABLE fts_note USING fts5(
  title, body,
  content='',                                   -- contentless (we feed rows explicitly)
  tokenize='unicode61 remove_diacritics 2'
);
-- rowid ↔ note entity mapped via a side table fts_note_map(rowid INTEGER PK, entity_id BLOB)

CREATE VIRTUAL TABLE fts_task USING fts5(
  title, notes_md,
  tokenize='unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE fts_transcript USING fts5(
  text,
  tokenize='unicode61 remove_diacritics 2'
);

CREATE VIRTUAL TABLE fts_chunk USING fts5(
  breadcrumb, text,
  tokenize='unicode61 remove_diacritics 2'
);
```

Because `doc_json`/tasks are the source of truth, FTS is fully rebuildable and is repopulated on save inside the same transaction (delete+insert by rowid). BM25 ranking (`bm25(fts_chunk)`) returns synchronously (<10 ms); the command palette's **Go/Ask** modes read it first, then re-fuse with vector KNN.

### 10.1 Hybrid retrieval (RRF)

Retrieval fuses **FTS5 BM25 ∪ sqlite-vec KNN** by **Reciprocal Rank Fusion** (Garcia recipe, no score normalization): `score(d) = Σ 1/(k + rank_i(d))`, `k=60`. FTS returns immediately; embeddings stream in and re-fuse. First-class filters (`type:`, `tag:`, `date:`, `person:`, `is:`) compile to SQL predicates applied *before* fusion. Optional bge-reranker re-orders the fused top-N for the Ask pipeline.

---

## 11. Append-Only Journals (crash-safety for all four pillars)

Two NDJSON journal families provide crash-safety independent of SQLite's own WAL.

### 11.1 Session journal (inherited)

One NDJSON file per recording session at `journals/sessions/<session_id>.ndjson`. Records:

```
{"t":<ms>,"type":"session_state","from":"READY","to":"RECORDING"}
{"t":<ms>,"type":"capture_config","app_audio":true,"mic":true,"sample_rate":48000}
{"t":<ms>,"type":"segment","seg_id":"...","t_start_ms":1200,"t_end_ms":4300,"pass":"live","text":"..."}
{"t":<ms>,"type":"track_flush","track_id":"...","sha256":"...","bytes":123456}
{"t":<ms>,"type":"artifact","generation":1,"model":"qwen3-8b-q4","json_sha256":"..."}
{"t":<ms>,"type":"checkpoint","session_state":"CAPTURED"}
```

On crash-recovery the session enters `RECOVERING`, replays the journal to the last consistent checkpoint, and re-derives DB rows. A recorded second is never lost.

### 11.2 Entity op journal (extended — notes/tasks/reminders now crash-safe too)

Every mutation to a mutable entity appends a `entity_op` row **and** an NDJSON line to a daily-rotated `journals/ops/<YYYY-MM-DD>.ndjson`. This is the crash-safe write-ahead for the *notebook* pillars, not just meetings.

```sql
CREATE TABLE entity_op (
  op_id      TEXT PRIMARY KEY,                  -- ULID (time-sortable)
  entity_id  BLOB NOT NULL,
  kind       TEXT NOT NULL,                     -- create|update|delete|link|unlink|field_set
  hlc        TEXT NOT NULL,                     -- hybrid logical clock
  actor      TEXT NOT NULL DEFAULT 'local',
  payload    TEXT NOT NULL,                     -- JSON op body (field diffs / doc patch / link tuple)
  applied    INTEGER NOT NULL DEFAULT 1,        -- projection applied to detail tables
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_op_entity ON entity_op(entity_id, hlc);
CREATE INDEX idx_op_hlc    ON entity_op(hlc);
```

NDJSON op record shape (one per line):

```
{"op_id":"01J...","entity":"<uuid>","kind":"update","hlc":"172...:3:nodeA",
 "payload":{"table":"task","fields":{"deadline_on":"2026-08-01","status":"open"}},"t":172...}
{"op_id":"01J...","entity":"<uuid>","kind":"field_set","hlc":"...",
 "payload":{"table":"note","doc_patch":{"blockId":"b7","node":{...}}},"t":...}
{"op_id":"01J...","entity":"<uuid>","kind":"link","hlc":"...",
 "payload":{"src":"<uuid>","dst":"<uuid>","rel":"tagged"}}
```

**Truth-vs-projection contract.** `entity_op` + `note.doc_json` + detail tables are truth. `block`, projected `link` rows, FTS, `chunk`, `embedding` are rebuildable projections. A cold rebuild (`job.kind='reindex'`) replays truth and reproduces every projection deterministically. This is also the dormant **sync seam**: tables are a materialized projection of the op-log; a future Loro/blind-relay sync (v-next) ships op ranges, no re-model required.

---

## 12. On-Disk Directory Layout

```
<app_data_dir>/                       # OS app-data (macOS ~/Library/Application Support/CasualNote, etc.)
├── casualnote.db                     # SQLCipher-encrypted: ALL tables + FTS5 + sqlite-vec
├── casualnote.db-wal                 # SQLite WAL (also encrypted by SQLCipher)
├── casualnote.db-shm
├── files/                            # content-addressed blobs (attachments + captured audio)
│   ├── 3f/3f9a...c1                   # sharded by sha256[0:2]
│   └── a0/a0b2...ee
├── journals/
│   ├── sessions/<session_id>.ndjson  # per-recording crash journal
│   └── ops/<YYYY-MM-DD>.ndjson       # rotated entity-op write-ahead
├── models/                           # downloaded/imported model files
│   ├── stt/whisper-small-q5.gguf
│   ├── llm/qwen3-8b-q4_k_m.gguf
│   └── embed/embeddinggemma-300m-int8.gguf
├── models/manifests/                 # signed manifests + checksums
├── exports/                          # user-initiated Markdown/JSON exports (transient)
├── backups/                          # optional local snapshots (single-file DB copies)
└── logs/                             # local diagnostic logs (no telemetry, opt-in verbosity)
```

The WebView never sees SQL or raw filesystem paths; the Rust `storage` crate (direct `rusqlite`, **not** `tauri-plugin-sql`) owns every access. Blob reads reach the UI only through scoped Tauri `fs` (attachments dir) or a native asset handler.

---

## 13. Encryption Boundaries, Migrations & Versioning

### 13.1 Encryption boundaries

| Asset | At rest | Key custody |
|---|---|---|
| `casualnote.db` (+WAL/SHM, incl. FTS5 & sqlite-vec) | **SQLCipher** whole-DB AES-256 | DB key in OS keystore (Keychain / Credential Manager / Secret Service) |
| `files/` blobs (attachments, audio) | Content-addressed; encrypted-at-rest via OS disk or per-file XChaCha20 *(reserved for v-next)* | same keystore-held key |
| `journals/*.ndjson` | Written to app-data; **encrypted-at-rest** consistent with the DB envelope | same |
| `models/` | Plain (public model weights), integrity via SHA-256 + signed manifest | n/a (integrity, not secrecy) |
| Secrets (DB key, future sync key) | Never in the DB or files | **only** in the OS keystore |

Network is touched by exactly two services — `model-download` and `updater` — both user-consented; every other path works offline ("Offline Ready" is a first-class state). No telemetry by default.

### 13.2 Schema migration & versioning

- **DB schema version** stored in `PRAGMA user_version` and mirrored into `setting('schema_version')`. Migrations are ordered, forward-only Rust functions (`storage::migrations::V001..VNNN`), each idempotent and transactional; the app refuses to open a DB whose `user_version` exceeds the binary's known max (prevents old binary + new DB corruption).
- **Editor doc version** — `note.doc_schema_version` per note. A Tiptap schema change ships a doc-migration pass that upgrades `doc_json` lazily on open (or eagerly via a `job.kind='reindex'`), bumping the per-note marker. Old docs remain readable.
- **Artifact version** — `artifact.schema_version` (MeetingArtifactV1 → V2 later). Old artifacts are retained immutably; regeneration produces the current version.
- **Embedding provenance** — `embedding.embed_model` + `dims`. Swapping the embedder enqueues a scoped re-embed of rows whose `embed_model` differs; retrieval tolerates mixed provenance during the transition (KNN restricted to matching `embed_model` per query batch).
- **Soft-delete retention** — tombstones (`deleted_at` set) survive for a retention window (default 90 days) to preserve the op-log's referential story for future sync, then a `job.kind='compact'` hard-deletes and vacuums.

---

## 14. AI Artifact JSON Contracts

### 14.1 `MeetingArtifactV1` (carried forward, authoritative here)

Stored in `artifact.artifact_json`. Every fact carries transcript evidence (`evidence_segment_ids` → `transcript_segment.id`). The model must not invent owners or dates.

```jsonc
{
  "schema": "MeetingArtifactV1",
  "session_id": "<uuid>",
  "executive_summary": "string",
  "topics": [
    { "title": "string", "summary": "string",
      "evidence_segment_ids": ["<segment_id>", "..."] }
  ],
  "decisions": [
    { "statement": "string", "rationale": "string|null",
      "evidence_segment_ids": ["<segment_id>"] }
  ],
  "action_items": [
    { "task": "string",
      "owner": "string|null",          // null unless stated in evidence
      "due_date": "YYYY-MM-DD|null",    // null unless stated in evidence
      "evidence_segment_ids": ["<segment_id>"] }   // REQUIRED, non-empty
  ],
  "risks":          [ { "statement": "string", "evidence_segment_ids": ["..."] } ],
  "open_questions": [ { "question": "string",  "evidence_segment_ids": ["..."] } ]
}
```

**Constraints (enforced by GBNF-constrained decode + post-validation):** `evidence_segment_ids` non-empty on every fact and each id resolves to a real `transcript_segment`; `owner`/`due_date` null unless present in cited evidence. One repair pass on schema-violation, then deterministic fallback (topics-only from transcript). Validation failures never surface invented data.

### 14.2 `AnswerV1` (Ask-your-notes RAG output)

```jsonc
{
  "schema": "AnswerV1",
  "answer": "string",
  "citations": [
    { "chunk_id": "<uuid>", "source_kind": "note_block|transcript_window|task|reminder",
      "source_id": "<uuid>", "t_start_ms": 0, "snippet": "string" }
  ],
  "confidence": 0.0,                   // 0..1
  "unanswered": false                  // true ⇒ "I couldn't find this in your notes"
}
```

**Citation-verify contract:** before display, every `citations[].chunk_id` MUST resolve to a real `chunk`. If none resolve, return `{"unanswered": true}` with empty citations rather than hallucinate (Foundation §4).

### 14.3 `ParsedEntry` (natural-language quick entry)

Emitted by `app-nlp` (grammar fast-path; LLM fallback only on low confidence). Never invents a date the user didn't state.

```jsonc
{
  "schema": "ParsedEntry",
  "kind": "task|reminder|note",
  "title": "string",
  "start_on": "YYYY-MM-DD|null",
  "deadline_on": "YYYY-MM-DD|null",
  "reminder": { "fire_at": 0, "tz": "IANA", "rrule": "string|null" } || null,
  "recurrence": { "rrule": "string", "mode": "fixed|after_completion" } || null,
  "project": "string|null",            // from inline #project
  "tags": ["string"],                  // from inline #tag
  "assignee": "string|null",           // from inline @person
  "priority": 0,                        // from inline !priority
  "confidence": 0.0,
  "used_llm_fallback": false
}
```

---

## 15. Import / Export Contracts

### 15.1 Markdown import/export (a feature, not the storage format)

- **Note export.** `note.doc_json` → CommonMark + GFM extensions. Custom nodes map: `todo` → `- [ ]` / `- [x]`; `callout` → blockquote with `[!type]` admonition; `code` → fenced block with language; `table` → GFM table; `[[wikilink]]` → `[[Title]]` preserved (or `[Title](note://<uuid>)` in "portable" mode); `#tag` / `@mention` preserved literally; `transcript_segment` → blockquote with `> [mm:ss] speaker: text` and a trailing `<!-- seg:<id> -->` provenance comment. Front-matter YAML carries `id`, `title`, `notebook`, `tags`, `daily_date`, `created_at`.
- **Note import.** Markdown → `doc_json`; front-matter `id` reused if present (idempotent re-import), else a fresh UUIDv7. `[[wikilink]]` targets resolved by title; unresolved links become creation stubs on demand. Import runs through the same `notes` projection (blocks, links, FTS, chunks) as any save.

### 15.2 Task import/export (interop)

- **Export** — JSON array of tasks with `{id, title, notes_md, project, area, start_on, deadline_on, status, checklist[], tags[], created_at}`. A Markdown mode renders `- [ ]`/`- [x]` grouped by project heading. TaskPaper-style `@tags` supported for round-trip with Things/Todoist-adjacent tools.
- **Import** — JSON or `- [ ]` Markdown / TaskPaper. Dates parsed via `app-nlp`; `every`/`every!` recurrence phrases mapped to `recurrence_rule`. Import never invents dates absent from the source.

### 15.3 Full-vault export (portability guarantee)

A single **portable bundle**: `manifest.json` (schema versions, entity counts, model provenance) + `notes/*.md` (with front-matter) + `tasks.json` + `reminders.json` + `meetings/<id>/` (artifact JSON + transcript NDJSON + optional audio) + `attachments/` (content-addressed). This guarantees the user's data is theirs and leaves in open formats — no lock-in — consistent with local-first principle #1. The bundle is round-trippable back through the import paths above.

---

## 16. Consistency Checklist (self-audit against the Foundation)

| Foundation decision | Where honored here |
|---|---|
| Universal `entity` spine + typed detail + one `link` table | §3, §4–§9, §5 |
| JSON-doc-as-truth (Tiptap), derived block/link/FTS | §4.1–4.2, §10, §11.2 |
| Fractional indices for blocks & tasks | `order_key` in §4.2, §6.3 |
| Things buckets = derived queries; start/deadline/reminder split | §6.3 table |
| Reminders first-class polymorphic; dual-layer scheduler fields | §7.1 (`os_handle`,`os_layer`) |
| Recurrence via rrule, materialize-on-completion, fixed vs after_completion | §7.2 |
| Hybrid FTS5 ∪ sqlite-vec, RRF, filters compile to SQL | §10, §10.1 |
| sqlite-vec inside SQLCipher (no FAISS) | §9.3, §12, §13.1 |
| Evidence-cited artifacts; owner/date only if extracted | §8.4–8.5, §14.1 |
| Citation-verify or `unanswered` | §14.2 |
| Session state machine incl. INDEXING-into-spine | §8.1 |
| Op-log/HLC/UUIDv7 seam; tables = projection; Loro-ready | §1, §11.2, §13.2 |
| Crash-safe journals for notes/tasks/reminders too | §11.2 |
| Two network services only; no telemetry; offline-ready | §13.1 |
| Content-addressed attachments/audio; single encrypted store | §4.5, §8.2, §9.3, §12 |

---

*End of Data Model & Storage Schema. Physical schema is authoritative here; product rationale (PRD), subsystem runtime (HLD), decomposition (Architecture), and per-feature behavior (Feature Specs) live in their respective documents.*

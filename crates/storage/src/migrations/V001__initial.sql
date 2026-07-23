-- Casual Note — migration V001 (initial Phase-1 schema).
--
-- Authoritative source: docs/casual-note-data-model.md. Every table, column,
-- key, index, and CHECK below is transcribed from that document. Do NOT invent
-- columns here; change the Data Model first, then this file.
--
-- Scope (Phase 1 — Core Notebook + Planning + Local Store):
--   spine  : entity
--   notes  : note, block, notebook, tag, attachment
--   graph  : link
--   plan   : area, project, heading, task, checklist_item
--   remind : reminder, recurrence_rule
--   people : person
--   oplog  : entity_op
--   kv     : setting
--   fts    : fts_note / fts_task / fts_transcript / fts_chunk (+ rowid<->entity maps)
--
-- Later-phase detail tables (session, audio_track, transcript_segment, artifact,
-- action_item, chunk, embedding, vec_chunk, model_installation, job, suggestion)
-- are intentionally NOT created here; they arrive with their owning crates. All
-- Phase-1 foreign keys reference only entity(id) or heading(id), both present.

------------------------------------------------------------------------------
-- §3.1 Universal spine
------------------------------------------------------------------------------
CREATE TABLE entity (
  id           BLOB    PRIMARY KEY,           -- UUIDv7, 16 bytes
  kind         TEXT    NOT NULL,
  title        TEXT,
  daily_date   TEXT,
  created_at   INTEGER NOT NULL,
  updated_at   INTEGER NOT NULL,
  hlc          TEXT    NOT NULL,
  deleted_at   INTEGER,
  CHECK (kind IN ('note','notebook','tag','task','project','area',
                  'reminder','session','artifact','action_item',
                  'person','recurrence_rule'))
);
CREATE INDEX idx_entity_kind     ON entity(kind) WHERE deleted_at IS NULL;
CREATE INDEX idx_entity_daily    ON entity(daily_date) WHERE daily_date IS NOT NULL AND deleted_at IS NULL;
CREATE INDEX idx_entity_updated  ON entity(updated_at);
CREATE INDEX idx_entity_kind_upd ON entity(kind, updated_at) WHERE deleted_at IS NULL;

------------------------------------------------------------------------------
-- §4 Knowledge pillar
------------------------------------------------------------------------------
CREATE TABLE note (
  entity_id          BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  notebook_id        BLOB REFERENCES entity(id),
  doc_json           TEXT NOT NULL,
  doc_schema_version INTEGER NOT NULL DEFAULT 1,
  daily_date         TEXT,
  is_pinned          INTEGER NOT NULL DEFAULT 0,
  content_hash       TEXT NOT NULL,
  word_count         INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX idx_note_notebook ON note(notebook_id);
CREATE UNIQUE INDEX idx_note_daily ON note(daily_date) WHERE daily_date IS NOT NULL;

CREATE TABLE block (
  block_id     TEXT NOT NULL,
  note_id      BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  node_type    TEXT NOT NULL,
  seq          INTEGER NOT NULL,
  depth        INTEGER NOT NULL DEFAULT 0,
  text_content TEXT,
  attrs_json   TEXT,
  order_key    TEXT NOT NULL,
  PRIMARY KEY (note_id, block_id)
);
CREATE INDEX idx_block_note_seq ON block(note_id, seq);
CREATE INDEX idx_block_type     ON block(node_type);

CREATE TABLE notebook (
  entity_id   BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  parent_id   BLOB REFERENCES entity(id),
  order_key   TEXT NOT NULL,
  icon        TEXT,
  color       TEXT
);
CREATE INDEX idx_notebook_parent ON notebook(parent_id);

CREATE TABLE tag (
  entity_id   BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  display     TEXT NOT NULL,
  color       TEXT,
  schema_json TEXT
);
-- NOTE (deviation from Data Model §4.4): the doc's partial-unique index
--   `... WHERE entity_id IN (SELECT id FROM entity WHERE deleted_at IS NULL)`
-- is not expressible in SQLite (partial-index WHERE forbids subqueries). We keep
-- a plain lookup index; live-uniqueness of `name` is enforced at the Rust
-- `notes`/`tags` boundary. Flagged in the crate return notes for reconciliation.
CREATE INDEX idx_tag_name ON tag(name);

CREATE TABLE attachment (
  id           BLOB PRIMARY KEY,
  owner_id     BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  block_id     TEXT,
  sha256       TEXT NOT NULL,
  filename     TEXT NOT NULL,
  mime         TEXT NOT NULL,
  byte_size    INTEGER NOT NULL,
  created_at   INTEGER NOT NULL,
  deleted_at   INTEGER
);
CREATE INDEX idx_attach_owner ON attachment(owner_id);
CREATE INDEX idx_attach_sha   ON attachment(sha256);

------------------------------------------------------------------------------
-- §5 Polymorphic link graph
------------------------------------------------------------------------------
CREATE TABLE link (
  id                   BLOB PRIMARY KEY,
  src_entity           BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  dst_entity           BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  rel                  TEXT NOT NULL,
  src_block_id         TEXT,
  dst_block_id         TEXT,
  evidence_segment_ids TEXT,
  data_json            TEXT,
  origin               TEXT NOT NULL DEFAULT 'user',
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

------------------------------------------------------------------------------
-- §6 Planning pillar
------------------------------------------------------------------------------
CREATE TABLE area (
  entity_id BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  order_key TEXT NOT NULL,
  icon      TEXT
);

CREATE TABLE project (
  entity_id    BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  area_id      BLOB REFERENCES entity(id),
  note_id      BLOB REFERENCES entity(id),
  status       TEXT NOT NULL DEFAULT 'active',
  start_on     TEXT,
  deadline_on  TEXT,
  completed_at INTEGER,
  order_key    TEXT NOT NULL,
  CHECK (status IN ('active','completed','canceled'))
);
CREATE INDEX idx_project_area ON project(area_id) WHERE status = 'active';

CREATE TABLE heading (
  id         BLOB PRIMARY KEY,
  project_id BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  title      TEXT NOT NULL,
  order_key  TEXT NOT NULL
);
CREATE INDEX idx_heading_project ON heading(project_id);

CREATE TABLE task (
  entity_id          BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  project_id         BLOB REFERENCES entity(id),
  area_id            BLOB REFERENCES entity(id),
  heading_id         BLOB REFERENCES heading(id),
  parent_task_id     BLOB REFERENCES entity(id),
  notes_md           TEXT,
  status             TEXT NOT NULL DEFAULT 'open',
  priority           INTEGER NOT NULL DEFAULT 0,
  someday            INTEGER NOT NULL DEFAULT 0,
  start_on           TEXT,
  deadline_on        TEXT,
  completed_at       INTEGER,
  order_key          TEXT NOT NULL,
  assignee_person_id BLOB REFERENCES entity(id),
  recurrence_id      BLOB REFERENCES entity(id),
  CHECK (status IN ('open','completed','canceled'))
);
CREATE INDEX idx_task_project  ON task(project_id)  WHERE status = 'open';
CREATE INDEX idx_task_area     ON task(area_id)     WHERE status = 'open';
CREATE INDEX idx_task_parent   ON task(parent_task_id);
CREATE INDEX idx_task_start     ON task(start_on)    WHERE status = 'open';
CREATE INDEX idx_task_deadline  ON task(deadline_on) WHERE status = 'open';

CREATE TABLE checklist_item (
  id        BLOB PRIMARY KEY,
  task_id   BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  text      TEXT NOT NULL,
  checked   INTEGER NOT NULL DEFAULT 0,
  order_key TEXT NOT NULL
);
CREATE INDEX idx_checklist_task ON checklist_item(task_id);

------------------------------------------------------------------------------
-- §7 Reminders & recurrence
------------------------------------------------------------------------------
CREATE TABLE reminder (
  entity_id       BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  target_kind     TEXT,
  target_id       BLOB REFERENCES entity(id),
  target_block_id TEXT,
  fire_at         INTEGER NOT NULL,
  tz              TEXT NOT NULL,
  state           TEXT NOT NULL DEFAULT 'pending',
  snoozed_until   INTEGER,
  os_handle       TEXT,
  os_layer        TEXT,
  recurrence_id   BLOB REFERENCES entity(id),
  body            TEXT,
  created_at      INTEGER NOT NULL,
  CHECK (state IN ('pending','fired','snoozed','missed','dismissed','canceled')),
  CHECK (target_kind IN ('task','note','session') OR target_kind IS NULL)
);
CREATE INDEX idx_reminder_fire   ON reminder(fire_at) WHERE state IN ('pending','snoozed');
CREATE INDEX idx_reminder_target ON reminder(target_kind, target_id);
CREATE INDEX idx_reminder_state  ON reminder(state);

CREATE TABLE recurrence_rule (
  entity_id          BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  rrule              TEXT NOT NULL,
  mode               TEXT NOT NULL,
  next_scheduled_on  TEXT,
  until_on           TEXT,
  count_remaining    INTEGER,
  complete_instances TEXT,
  CHECK (mode IN ('fixed','after_completion'))
);

------------------------------------------------------------------------------
-- §9.1 People
------------------------------------------------------------------------------
CREATE TABLE person (
  entity_id     BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  display       TEXT NOT NULL,
  canonical     TEXT NOT NULL,
  aliases       TEXT,
  email         TEXT,
  avatar_sha256 TEXT
);
CREATE UNIQUE INDEX idx_person_canonical ON person(canonical);

------------------------------------------------------------------------------
-- §11.2 Append-only op-log (crash-safe write-ahead + dormant sync seam)
------------------------------------------------------------------------------
CREATE TABLE entity_op (
  op_id      TEXT PRIMARY KEY,   -- ULID (time-sortable)
  entity_id  BLOB NOT NULL,
  kind       TEXT NOT NULL,      -- create|update|delete|link|unlink|field_set
  hlc        TEXT NOT NULL,
  actor      TEXT NOT NULL DEFAULT 'local',
  payload    TEXT NOT NULL,
  applied    INTEGER NOT NULL DEFAULT 1,
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_op_entity ON entity_op(entity_id, hlc);
CREATE INDEX idx_op_hlc    ON entity_op(hlc);

------------------------------------------------------------------------------
-- §9.7 Settings (kv). Mirrors PRAGMA user_version into setting('schema_version').
------------------------------------------------------------------------------
CREATE TABLE setting (
  key        TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

------------------------------------------------------------------------------
-- §10 Full-text search (FTS5). All contentless with a rowid<->entity side map,
-- so the index is a pure, deterministically-rebuildable projection of truth.
-- (Deviation: the Data Model spells out only fts_note_map; we give every FTS
-- table a companion map for a uniform rebuild. Flagged in the return notes.)
------------------------------------------------------------------------------
CREATE VIRTUAL TABLE fts_note USING fts5(
  title, body,
  content='',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE fts_note_map (rowid INTEGER PRIMARY KEY, entity_id BLOB NOT NULL UNIQUE);

CREATE VIRTUAL TABLE fts_task USING fts5(
  title, notes_md,
  content='',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE fts_task_map (rowid INTEGER PRIMARY KEY, entity_id BLOB NOT NULL UNIQUE);

CREATE VIRTUAL TABLE fts_transcript USING fts5(
  text,
  content='',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE fts_transcript_map (rowid INTEGER PRIMARY KEY, segment_id BLOB NOT NULL UNIQUE);

CREATE VIRTUAL TABLE fts_chunk USING fts5(
  breadcrumb, text,
  content='',
  tokenize='unicode61 remove_diacritics 2'
);
CREATE TABLE fts_chunk_map (rowid INTEGER PRIMARY KEY, chunk_id BLOB NOT NULL UNIQUE);

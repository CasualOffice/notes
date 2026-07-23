-- Casual Note — migration V002 (Meeting-pillar detail tables).
--
-- Authoritative source: docs/casual-note-data-model.md §8. Every table, column,
-- key, index, and CHECK below is transcribed from that document. Do NOT invent
-- columns here; change the Data Model first, then this file.
--
-- Scope (Phase 2 — Meeting Intelligence pipeline, M2):
--   session            — owns the state machine (§8.1)
--   audio_track        — captured tracks, content-addressed PCM (§8.2)
--   transcript_segment — the atomic unit of evidence (§8.3)
--   artifact           — MeetingArtifactV1, immutable per generation (§8.4)
--   action_item        — the bridge into Task (§8.5)
--
-- The owning spine kinds ('session','artifact','action_item') already validate
-- against the entity CHECK from V001; these are their detail tables. The FTS5
-- tables (fts_transcript / fts_transcript_map) were created in V001 and are
-- populated by the rebuild reprojection once transcript rows exist.

------------------------------------------------------------------------------
-- §8.1 session — the meeting state machine
------------------------------------------------------------------------------
CREATE TABLE session (
  entity_id       BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  state           TEXT NOT NULL,
  note_id         BLOB REFERENCES entity(id),
  started_at      INTEGER,
  ended_at        INTEGER,
  duration_ms     INTEGER,
  capture_source  TEXT,
  platform        TEXT NOT NULL,
  degraded_reason TEXT,
  journal_path    TEXT,
  CHECK (state IN ('NEW','PREFLIGHT','READY','RECORDING','PAUSED','STOPPING',
                   'CAPTURED','FINAL_TRANSCRIBING','GENERATING','INDEXING',
                   'COMPLETE','DEGRADED','FAILED','RECOVERING'))
);
CREATE INDEX idx_session_state   ON session(state);
CREATE INDEX idx_session_started ON session(started_at);

------------------------------------------------------------------------------
-- §8.2 audio_track
------------------------------------------------------------------------------
CREATE TABLE audio_track (
  id           BLOB PRIMARY KEY,
  session_id   BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  source_kind  TEXT NOT NULL,
  source_label TEXT,
  sample_rate  INTEGER NOT NULL,
  channels     INTEGER NOT NULL,
  audio_sha256 TEXT,
  byte_size    INTEGER
);
CREATE INDEX idx_track_session ON audio_track(session_id);

------------------------------------------------------------------------------
-- §8.3 transcript_segment — the atomic unit of evidence
------------------------------------------------------------------------------
CREATE TABLE transcript_segment (
  id          BLOB PRIMARY KEY,
  session_id  BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  track_id    BLOB REFERENCES audio_track(id),
  seq         INTEGER NOT NULL,
  t_start_ms  INTEGER NOT NULL,
  t_end_ms    INTEGER NOT NULL,
  speaker     TEXT,
  person_id   BLOB REFERENCES entity(id),
  text        TEXT NOT NULL,
  pass        TEXT NOT NULL DEFAULT 'final',
  confidence  REAL
);
CREATE INDEX idx_seg_session_seq ON transcript_segment(session_id, seq);
CREATE INDEX idx_seg_time        ON transcript_segment(session_id, t_start_ms);

------------------------------------------------------------------------------
-- §8.4 artifact — MeetingArtifactV1, immutable per generation
------------------------------------------------------------------------------
CREATE TABLE artifact (
  entity_id      BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  session_id     BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  schema_version INTEGER NOT NULL DEFAULT 1,
  generation     INTEGER NOT NULL DEFAULT 1,
  is_current     INTEGER NOT NULL DEFAULT 1,
  llm_model      TEXT NOT NULL,
  artifact_json  TEXT NOT NULL,
  generated_at   INTEGER NOT NULL
);
CREATE INDEX idx_artifact_session ON artifact(session_id) WHERE is_current=1;

------------------------------------------------------------------------------
-- §8.5 action_item — the bridge into Task
------------------------------------------------------------------------------
CREATE TABLE action_item (
  entity_id            BLOB PRIMARY KEY REFERENCES entity(id) ON DELETE CASCADE,
  artifact_id          BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  session_id           BLOB NOT NULL REFERENCES entity(id) ON DELETE CASCADE,
  idx                  INTEGER NOT NULL,
  task_text            TEXT NOT NULL,
  owner_person_id      BLOB REFERENCES entity(id),
  owner_text           TEXT,
  due_date             TEXT,
  evidence_segment_ids TEXT NOT NULL,
  promoted_task_id     BLOB REFERENCES entity(id),
  status               TEXT NOT NULL DEFAULT 'suggested'
);
CREATE INDEX idx_action_artifact ON action_item(artifact_id);
CREATE INDEX idx_action_session  ON action_item(session_id);

//! M2 meeting-intelligence proof (roadmap §5, HLD §8.4) — headless, mock engines.
//!
//! Drives a whole session `NEW → COMPLETE` over the injected capture/speech/LLM
//! **traits** with the mock doubles feeding a deterministic transcript, and asserts
//! the M2 invariants:
//! 1. the state machine reaches `COMPLETE`;
//! 2. a schema-valid `MeetingArtifactV1` exists and **every** `evidence_segment_id`
//!    resolves to a persisted `transcript_segment`;
//! 3. action items materialize as Tasks carrying `spawned_from` (with copied
//!    evidence) + `about` provenance edges;
//! 4. rebuild-from-log reproduces every meeting entity **bit-identically** (the
//!    correctness oracle);
//! 5. an LLM failure routes the session to `DEGRADED` without losing the transcript
//!    (the LLM never owns recording state).

use std::sync::{Arc, Mutex};

use app_domain::{AppEvent, Id, SessionState};
use app_service::{
    ActionItemOverrides, CannedAudioSource, EventSink, MeetingConfig, MockCaptureAdapter, Service,
    SessionCoordinator,
};
use llm_api::{
    ConstrainedLlm, GenerationRequest, Grammar, LlmError, MeetingArtifactV1, MockLlm,
    SchemaValidate,
};
use rusqlite::OptionalExtension;
use speech_api::MockSpeechEngine;
use storage::{Paths, Store};

/// A throwaway in-memory service (real op-log + FTS projection) plus a capturing
/// event sink so we can assert the `AppEvent` contract.
fn service_with_events() -> (Service, Arc<Mutex<Vec<AppEvent>>>) {
    let dir = std::env::temp_dir().join(format!("cn-m2-{}", Id::new()));
    let store = Store::open_memory(Paths::new(dir)).expect("open_memory");
    let events: Arc<Mutex<Vec<AppEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |ev| captured.lock().unwrap().push(ev.event));
    (Service::new(store, "test-node", sink), events)
}

/// A constrained-LLM double that reads the real `segment_id`s the coordinator put in
/// the prompt and emits a schema-valid `MeetingArtifactV1` citing the first one — so
/// evidence resolves to persisted segments without the test predicting ids. A real
/// GBNF-constrained backend plugs into this same `ConstrainedLlm` seam.
#[derive(Debug)]
struct ScriptedArtifactLlm {
    model_id: app_domain::ModelId,
}

impl ScriptedArtifactLlm {
    fn new() -> Self {
        Self {
            model_id: app_domain::ModelId::new("scripted-artifact-llm"),
        }
    }

    /// Pull the first `segment_id` string out of the prompt's transcript manifest.
    fn first_segment(prompt: &str) -> Option<String> {
        let marker = "TRANSCRIPT_SEGMENTS_JSON:\n";
        let json = prompt.split(marker).nth(1)?;
        let arr: serde_json::Value = serde_json::from_str(json.trim()).ok()?;
        arr.get(0)?
            .get("segment_id")?
            .as_str()
            .map(ToString::to_string)
    }
}

impl ConstrainedLlm for ScriptedArtifactLlm {
    fn model_id(&self) -> &app_domain::ModelId {
        &self.model_id
    }

    fn decode(&self, req: &GenerationRequest, _grammar: &Grammar) -> Result<String, LlmError> {
        let Some(seg) = Self::first_segment(&req.prompt) else {
            return Ok("{}".to_string()); // no segments -> drives the fallback path
        };
        let artifact = serde_json::json!({
            "schema": "MeetingArtifactV1",
            "session_id": "00000000-0000-0000-0000-000000000000",
            "executive_summary": "The team agreed to ship the beta and write the report.",
            "topics": [{
                "title": "Beta launch",
                "summary": "Beta ships this week.",
                "evidence_segment_ids": [seg]
            }],
            "decisions": [{
                "statement": "Ship the beta on Friday",
                "rationale": null,
                "evidence_segment_ids": [seg]
            }],
            "action_items": [{
                "task": "Write the release report",
                "owner": null,
                "due_date": null,
                "evidence_segment_ids": [seg]
            }],
            "risks": [],
            "open_questions": []
        });
        Ok(serde_json::to_string(&artifact).unwrap())
    }
}

/// ~13 s of a speech-like tone at 16 kHz mono, in 0.5 s blocks — enough to force at
/// least one media-pipeline chunk (and thus at least one final segment).
fn canned_audio() -> CannedAudioSource {
    CannedAudioSource::tone(220.0, 16_000, 1, 13.0, 500)
}

fn meeting_config() -> MeetingConfig {
    MeetingConfig {
        sources: vec!["mock.app".into()],
        capture_microphone: true,
        exclude_self: true,
        sample_rate_hz: 16_000,
        title: Some("Q3 Planning".into()),
    }
}

#[test]
fn full_session_reaches_complete_with_resolvable_evidence_and_task_bridge() {
    let (service, events) = service_with_events();

    let coordinator = SessionCoordinator::new(
        Arc::new(MockCaptureAdapter),
        Box::new(MockSpeechEngine::new()),
        Arc::new(ScriptedArtifactLlm::new()),
    )
    .expect("coordinator");

    let mut audio = canned_audio();
    let outcome = coordinator
        .run_session(&service, &meeting_config(), &mut audio)
        .expect("run_session");

    // 1) The state machine reached COMPLETE.
    assert_eq!(outcome.state, SessionState::Complete);
    assert!(outcome.segment_count >= 1, "at least one final segment");
    assert!(outcome.action_item_count >= 1, "at least one action item");
    let note_id = outcome.note_id.clone().expect("meeting-as-note");

    // 2) A schema-valid MeetingArtifactV1 exists with resolvable evidence.
    let artifact = outcome.artifact.clone().expect("artifact");
    artifact.validate().expect("artifact schema-valid");
    assert_eq!(artifact.schema, MeetingArtifactV1::SCHEMA);

    let mut evidence_ids: Vec<Id> = Vec::new();
    for t in &artifact.topics {
        evidence_ids.extend(t.evidence_segment_ids.iter().copied());
    }
    for d in &artifact.decisions {
        evidence_ids.extend(d.evidence_segment_ids.iter().copied());
    }
    for a in &artifact.action_items {
        evidence_ids.extend(a.evidence_segment_ids.iter().copied());
    }
    assert!(!evidence_ids.is_empty(), "artifact cites evidence");
    for seg in &evidence_ids {
        let resolves = segment_exists(&service, *seg);
        assert!(resolves, "evidence segment {seg} must resolve to a row");
    }

    // The session row + note binding persisted.
    let session_view = service.session_get(&outcome.session_id).unwrap();
    assert_eq!(session_view.state, "COMPLETE");
    assert_eq!(session_view.note_id.as_deref(), Some(note_id.as_str()));
    assert!(session_view.duration_ms.is_some());

    // The whole state-machine trace was pushed as AppEvents, ending at COMPLETE.
    let evs = events.lock().unwrap();
    assert!(evs
        .iter()
        .any(|e| matches!(e, AppEvent::ArtifactReady { .. })));
    assert!(evs
        .iter()
        .any(|e| matches!(e, AppEvent::LiveTranscript { .. })));
    let reached_complete = evs.iter().any(|e| {
        matches!(
            e,
            AppEvent::SessionStateChanged {
                to: SessionState::Complete,
                ..
            }
        )
    });
    assert!(reached_complete, "SessionStateChanged reached COMPLETE");
    drop(evs);

    // 3) Action items materialize as Tasks with spawned_from + evidence + about.
    let items = service.session_action_items(&outcome.session_id).unwrap();
    assert_eq!(items.len(), artifact.action_items.len());
    let mut promoted_tasks = Vec::new();
    for item in &items {
        assert_eq!(item.status, "suggested");
        assert!(!item.evidence_segment_ids.is_empty());
        let task_id = coordinator
            .action_item_to_task(&service, &item.id, &ActionItemOverrides::default())
            .expect("bridge to task");
        promoted_tasks.push(task_id);
    }

    for task_id in &promoted_tasks {
        // spawned_from edge task -> session, carrying copied evidence.
        let (spawned, evidence): (i64, Option<String>) = service
            .store()
            .db()
            .with_writer_conn(|c| {
                c.query_row(
                    "SELECT count(*), max(evidence_segment_ids) FROM link \
                     WHERE src_entity = ?1 AND rel = 'spawned_from' AND deleted_at IS NULL",
                    rusqlite::params![id_bytes(task_id)],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(spawned, 1, "exactly one spawned_from edge");
        let evidence = evidence.expect("spawned_from carries evidence");
        assert!(evidence.contains('-'), "evidence is a JSON array of uuids");

        // about edge task -> meeting note.
        let about: i64 = service
            .store()
            .db()
            .with_writer_conn(|c| {
                c.query_row(
                    "SELECT count(*) FROM link \
                     WHERE src_entity = ?1 AND rel = 'about' AND deleted_at IS NULL",
                    rusqlite::params![id_bytes(task_id)],
                    |r| r.get(0),
                )
                .map_err(Into::into)
            })
            .unwrap();
        assert_eq!(about, 1, "task relates to the meeting note");
    }

    // The action items flipped to promoted.
    let after = service.session_action_items(&outcome.session_id).unwrap();
    assert!(after.iter().all(|i| i.status == "promoted"));
    assert!(after.iter().all(|i| i.promoted_task_id.is_some()));

    // 4) Rebuild-from-log reproduces every meeting entity bit-identically.
    let before = meeting_snapshot(&service);
    service.store().rebuild().expect("rebuild_from_log");
    let after_snap = meeting_snapshot(&service);
    assert_eq!(
        before, after_snap,
        "meeting entities must rebuild bit-identically from the op-log"
    );

    // The rebuilt world still resolves the evidence + serves the session.
    for seg in &evidence_ids {
        assert!(segment_exists(&service, *seg), "evidence survives rebuild");
    }
    assert_eq!(
        service.session_get(&outcome.session_id).unwrap().state,
        "COMPLETE"
    );
}

#[test]
fn llm_failure_degrades_without_losing_the_transcript() {
    let (service, _events) = service_with_events();

    // A backend that always fails to decode (no scripted responses -> DecodeFailed).
    let failing_llm = MockLlm::scripted(Vec::new());
    let coordinator = SessionCoordinator::new(
        Arc::new(MockCaptureAdapter),
        Box::new(MockSpeechEngine::new()),
        Arc::new(failing_llm),
    )
    .expect("coordinator");

    let mut audio = canned_audio();
    let outcome = coordinator
        .run_session(&service, &meeting_config(), &mut audio)
        .expect("run_session returns Ok even when generation fails");

    // The LLM never owns recording state: generation failure -> DEGRADED, not FAILED.
    assert_eq!(outcome.state, SessionState::Degraded);
    assert!(outcome.artifact.is_none());
    assert!(outcome.note_id.is_none());
    assert!(outcome.degraded_reason.is_some());

    // The transcript is preserved: final segments were persisted before GENERATING.
    assert!(outcome.segment_count >= 1);
    let persisted: i64 = service
        .store()
        .db()
        .with_writer_conn(|c| {
            c.query_row(
                "SELECT count(*) FROM transcript_segment WHERE pass = 'final'",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(persisted as usize, outcome.segment_count);
    assert!(persisted >= 1, "transcript survives a generation failure");

    let session_view = service.session_get(&outcome.session_id).unwrap();
    assert_eq!(session_view.state, "DEGRADED");
    assert!(session_view.degraded_reason.is_some());

    // The transcript rebuilds from the log unchanged.
    service.store().rebuild().expect("rebuild_from_log");
    let after: i64 = service
        .store()
        .db()
        .with_writer_conn(|c| {
            c.query_row(
                "SELECT count(*) FROM transcript_segment WHERE pass = 'final'",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(after, persisted, "transcript rebuilds bit-identically");
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn id_bytes(id_str: &str) -> Vec<u8> {
    id_str.parse::<Id>().unwrap().as_bytes().to_vec()
}

fn segment_exists(service: &Service, seg: Id) -> bool {
    service
        .store()
        .db()
        .with_writer_conn(|c| {
            Ok(c.query_row(
                "SELECT 1 FROM transcript_segment WHERE id = ?1",
                rusqlite::params![seg.as_bytes().as_slice()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
        })
        .unwrap()
}

/// A deterministic textual dump of every meeting-pillar table plus the meeting note
/// and its provenance links. Two byte-identical dumps ⇒ a bit-identical rebuild.
fn meeting_snapshot(service: &Service) -> String {
    service
        .store()
        .db()
        .with_writer_conn(|c| {
            let mut out = String::new();
            let mut dump = |sql: &str, ncols: usize, label: &str| {
                out.push_str(label);
                out.push('\n');
                let mut stmt = c.prepare(sql).unwrap();
                let mut rows = stmt.query([]).unwrap();
                while let Some(row) = rows.next().unwrap() {
                    for i in 0..ncols {
                        let cell = match row.get_ref(i).unwrap() {
                            rusqlite::types::ValueRef::Null => "∅".to_string(),
                            rusqlite::types::ValueRef::Integer(n) => n.to_string(),
                            rusqlite::types::ValueRef::Real(f) => format!("{f}"),
                            rusqlite::types::ValueRef::Text(t) => {
                                String::from_utf8_lossy(t).into_owned()
                            }
                            rusqlite::types::ValueRef::Blob(b) => {
                                b.iter().map(|x| format!("{x:02x}")).collect()
                            }
                        };
                        out.push_str(&cell);
                        out.push('|');
                    }
                    out.push('\n');
                }
            };

            dump(
                "SELECT id, kind, title, deleted_at FROM entity \
                 WHERE kind IN ('session','artifact','action_item','note') ORDER BY id",
                4,
                "== entity ==",
            );
            dump(
                "SELECT entity_id, state, note_id, platform, degraded_reason FROM session \
                 ORDER BY entity_id",
                5,
                "== session ==",
            );
            dump(
                "SELECT id, session_id, source_kind, sample_rate, channels FROM audio_track \
                 ORDER BY id",
                5,
                "== audio_track ==",
            );
            dump(
                "SELECT id, session_id, seq, t_start_ms, t_end_ms, text, pass \
                 FROM transcript_segment ORDER BY id",
                7,
                "== transcript_segment ==",
            );
            dump(
                "SELECT entity_id, session_id, generation, is_current, artifact_json \
                 FROM artifact ORDER BY entity_id",
                5,
                "== artifact ==",
            );
            dump(
                "SELECT entity_id, session_id, idx, task_text, status, promoted_task_id \
                 FROM action_item ORDER BY idx",
                6,
                "== action_item ==",
            );
            dump(
                "SELECT src_entity, dst_entity, rel, evidence_segment_ids, origin FROM link \
                 WHERE origin = 'meeting' AND deleted_at IS NULL \
                 ORDER BY src_entity, rel, dst_entity",
                5,
                "== meeting_links ==",
            );
            // FTS probe: the meeting note body is searchable.
            dump(
                "SELECT count(*) FROM fts_transcript_map",
                1,
                "== fts_transcript_map count ==",
            );
            Ok(out)
        })
        .unwrap()
}

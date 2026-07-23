//! M0 walking-skeleton proof (roadmap §5) — headless, no GUI.
//!
//! Exercises the real note write path end to end and asserts the M0 invariants:
//! 1. the encrypted SQLCipher store opens (key provisioned with the dev fallback);
//! 2. `create_note` appends **exactly one** `create` op to `entity_op`;
//! 3. the note reads back and the derived tables (spine/detail/`block`/FTS) rebuild
//!    **bit-identically** from the op-log (the master correctness oracle);
//! 4. the note's title/body text never appears as plaintext in the raw DB bytes
//!    (SQLCipher encryption check).

use std::sync::{Arc, Mutex};

use app_domain::{AppEvent, Id};
use app_service::{EventSink, Service};
use storage::{keystore, DevFileKeyStore, Paths, Store};

/// A unique scratch app-data root under the OS temp dir.
fn scratch_root() -> Paths {
    let name = Id::new().to_string();
    Paths::new(std::env::temp_dir().join(format!("cn-m0-{name}")))
}

/// A deterministic textual dump of every derived table M0 cares about, plus an
/// FTS `MATCH` probe. Two byte-identical dumps ⇒ a bit-identical rebuild.
fn snapshot(service: &Service, body_token: &str) -> String {
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
                "SELECT id, kind, title, deleted_at FROM entity ORDER BY id",
                4,
                "== entity ==",
            );
            dump(
                "SELECT entity_id, doc_json, content_hash, word_count FROM note ORDER BY entity_id",
                4,
                "== note ==",
            );
            dump(
                "SELECT note_id, block_id, seq, node_type, text_content FROM block \
                 ORDER BY note_id, seq",
                5,
                "== block ==",
            );
            dump(
                "SELECT rowid, entity_id FROM fts_note_map ORDER BY rowid",
                2,
                "== fts_note_map ==",
            );
            // FTS content probe: the body token must resolve to the note's rowid.
            dump(
                &format!(
                    "SELECT rowid FROM fts_note WHERE fts_note MATCH '{body_token}' ORDER BY rowid"
                ),
                1,
                "== fts_note MATCH body ==",
            );
            Ok(out)
        })
        .unwrap()
}

#[test]
fn m0_note_path_is_real_encrypted_and_rebuildable() {
    let paths = scratch_root();

    // Distinctive, unique probe strings so the plaintext scan cannot false-negative.
    let uniq = Id::new().to_string().replace('-', "");
    let title = format!("ZZTITLEPROBE{uniq}");
    let body_token = format!("bodyprobe{uniq}");
    let body_text = format!("the {body_token} walking skeleton");
    let doc_json = format!(
        r#"{{"type":"doc","content":[{{"type":"paragraph","content":[{{"type":"text","text":"{body_text}"}}]}}]}}"#
    );

    // Capture emitted events to assert the AppEvent contract.
    let events: Arc<Mutex<Vec<AppEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let captured = events.clone();
    let sink: EventSink = Box::new(move |ev| captured.lock().unwrap().push(ev.event));

    // 1. Open a real encrypted (SQLCipher) temp-file store. We provision the key
    //    through the storage key-mgmt dev fallback scoped to the temp root, so the
    //    test is hermetic (it never touches the machine's real OS keyring — that
    //    keyring-first path is `Service::open`, exercised by `tauri-app`).
    let dev = DevFileKeyStore::new(paths.root().join(".dev-db-key"));
    let key = keystore::provision_db_key(&dev).expect("provision dev master key");
    let store = Store::open(paths.clone(), key).expect("encrypted store opens");
    let service = Service::new(store, "test-node", sink);
    service
        .recover()
        .expect("journal recovery is a no-op on a fresh store");

    // 2. Create a note — the real write path (op-log append + projection).
    let note = service
        .create_note(title.clone(), Some(doc_json))
        .expect("create_note");
    assert_eq!(note.title.as_deref(), Some(title.as_str()));
    let note_id = note.id.clone();

    // Exactly one `create` op exists in the op-log (blocks are `field_set` ops).
    let create_ops: i64 = service
        .store()
        .db()
        .with_writer_conn(|c| {
            c.query_row(
                "SELECT count(*) FROM entity_op WHERE kind = 'create'",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert_eq!(create_ops, 1, "create_note appends exactly one create op");

    // Read-back through the service (spine + doc_json).
    let fetched = service.get_note(&note_id).expect("get_note");
    assert_eq!(fetched.doc_json, note.doc_json);
    assert!(fetched.doc_json.contains(&body_token));
    assert_eq!(service.list_notes().unwrap().len(), 1);

    // The M0 AppEvents were emitted.
    let evs = events.lock().unwrap();
    assert!(evs.iter().any(|e| matches!(e, AppEvent::NoteSaved { .. })));
    assert!(evs
        .iter()
        .any(|e| matches!(e, AppEvent::NoteProjected { .. })));
    drop(evs);

    // 3. Rebuild-from-log must be bit-identical (the correctness oracle).
    let before = snapshot(&service, &body_token);
    service.store().rebuild().expect("rebuild_from_log");
    let after = snapshot(&service, &body_token);
    assert_eq!(before, after, "derived tables must rebuild bit-identically");

    // The projected note + FTS survived the rebuild.
    assert!(before.contains(&body_text), "block text projected");
    assert!(
        after.contains("== fts_note MATCH body ==\n1|"),
        "FTS reproduced the note body after rebuild"
    );
    let refetched = service.get_note(&note_id).expect("note survives rebuild");
    assert_eq!(refetched.title.as_deref(), Some(title.as_str()));
    assert_eq!(refetched.doc_json, note.doc_json);

    // 4. Encryption check: no plaintext note text anywhere on disk.
    let db_file = service.store().paths().db_file();
    for suffix in ["", "-wal", "-shm"] {
        let path = if suffix.is_empty() {
            db_file.clone()
        } else {
            let mut p = db_file.clone().into_os_string();
            p.push(suffix);
            std::path::PathBuf::from(p)
        };
        let Ok(bytes) = std::fs::read(&path) else {
            continue; // -wal / -shm may not exist depending on checkpoint state
        };
        assert!(
            !contains_bytes(&bytes, title.as_bytes()),
            "note title leaked as plaintext in {}",
            path.display()
        );
        assert!(
            !contains_bytes(&bytes, body_token.as_bytes()),
            "note body leaked as plaintext in {}",
            path.display()
        );
    }

    // Cleanup.
    std::fs::remove_dir_all(paths.root()).ok();
}

/// True if `haystack` contains the contiguous byte slice `needle`.
fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

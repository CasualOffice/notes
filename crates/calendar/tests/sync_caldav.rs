//! Sync-layer tests (doc §3, §9): CalDAV protocol over `MockTransport`, conflict
//! resolution with local-edit preservation, capability honesty, and a full
//! pull -> local-edit -> push round-trip over `MockSyncAdapter`. No network.

use calendar::adapters::{remote_event, NativeBackend};
use calendar::caldav::CalDavClient;
use calendar::conflict::{resolve, Winner};
use calendar::sync::{
    CalId, CalendarCapability, CalendarSyncAdapter, ChangeSet, EventOp, LocalCalendarState,
    PushOutcome, RemoteEvent, SyncToken,
};
use calendar::transport::{HttpResponse, MockTransport};
use calendar::{
    AppointmentManagerAdapter, CalendarEvent, EdsAdapter, EventKitAdapter, MockSyncAdapter,
};

use app_domain::{Id, Timestamp};

const HOME: &str = "https://dav.example.com/calendars/alice/";
const WORK: &str = "https://dav.example.com/calendars/alice/work/";

/// A minimal, flush-left, unfolded ICS document (no leading spaces so the parser's
/// unfolding leaves it intact).
fn ics(uid: &str, title: &str) -> String {
    format!(
        "BEGIN:VCALENDAR\nVERSION:2.0\nPRODID:-//Test//EN\nBEGIN:VEVENT\nUID:{uid}\n\
         DTSTAMP:20260724T090000Z\nDTSTART:20260724T090000Z\nDTEND:20260724T100000Z\n\
         SUMMARY:{title}\nEND:VEVENT\nEND:VCALENDAR"
    )
}

fn event(uid: &str, title: &str) -> CalendarEvent {
    CalendarEvent::new(
        Id::new(),
        uid,
        title,
        Timestamp::from_millis(1_000),
        Timestamp::from_millis(2_000),
    )
}

/// A 207 sync-collection body: one upsert (200 + calendar-data) and one tombstone
/// (404), plus the collection-level next sync-token.
fn sync_collection_body(uid: &str, title: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav">
<d:response>
<d:href>/calendars/alice/work/{uid}.ics</d:href>
<d:propstat>
<d:prop>
<d:getetag>"etag-aaa"</d:getetag>
<c:calendar-data>{data}</c:calendar-data>
</d:prop>
<d:status>HTTP/1.1 200 OK</d:status>
</d:propstat>
</d:response>
<d:response>
<d:href>/calendars/alice/work/removed.ics</d:href>
<d:status>HTTP/1.1 404 Not Found</d:status>
</d:response>
<d:sync-token>https://dav.example.com/sync/42</d:sync-token>
</d:multistatus>"#,
        data = ics(uid, title)
    )
}

// ---------------------------------------------------------------------------
// 1. sync-collection incremental pull upserts events (and idempotency).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sync_collection_pull_upserts_and_tombstones() {
    let transport = MockTransport::with_responses([HttpResponse::new(
        207,
        sync_collection_body("evt-1", "Team Sync"),
    )
    .header("DAV", "1")]);
    let client = CalDavClient::new(transport, HOME);

    let cs = client
        .pull(&CalId::from(WORK), &SyncToken::some("prev-token"))
        .await
        .expect("pull ok");

    // Changed set carries the parsed event + etag; deleted carries the tombstone.
    assert_eq!(cs.changed.len(), 1);
    let re = &cs.changed[0];
    assert_eq!(re.event.uid, "evt-1");
    assert_eq!(re.event.title, "Team Sync");
    assert_eq!(re.event.etag.as_deref(), Some("\"etag-aaa\""));
    assert!(re.href.ends_with("/calendars/alice/work/evt-1.ics"));
    assert_eq!(cs.deleted.len(), 1);
    assert!(cs.deleted[0].ends_with("removed.ics"));
    assert_eq!(
        cs.next_token,
        SyncToken::some("https://dav.example.com/sync/42")
    );

    // The request was a REPORT carrying the prior sync-token at Depth 1.
    let req = client.transport().last_request().expect("a request");
    assert_eq!(req.method, "REPORT");
    assert_eq!(req.header_value("Depth"), Some("1"));
    assert!(req.body.contains("<d:sync-token>prev-token</d:sync-token>"));

    // Applying it upserts into local state; re-applying is idempotent.
    let mut state = LocalCalendarState::new();
    state.apply_pull(cs.clone());
    assert_eq!(state.len(), 1);
    assert_eq!(
        state.get("evt-1").map(|e| e.title.as_str()),
        Some("Team Sync")
    );
    assert!(state.revisions.is_empty());

    state.apply_pull(cs);
    assert_eq!(state.len(), 1);
    assert!(state.revisions.is_empty(), "re-apply must not duplicate");
}

// ---------------------------------------------------------------------------
// 2. push sets If-Match; a 412 is detected -> losing local edit preserved.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn push_sets_if_match_and_412_is_detected() {
    // A single Update op with a known ETag; server replies 412 Precondition Failed.
    let transport = MockTransport::with_responses([HttpResponse::new(412, "")]);
    let client = CalDavClient::new(transport, HOME);

    let mut ev = event("evt-9", "Local title");
    ev.etag = Some("\"stale-etag\"".to_string());
    let op = EventOp::Update {
        event: ev,
        href: "evt-9.ics".to_string(),
        etag: Some("\"stale-etag\"".to_string()),
    };

    let results = client
        .push(&CalId::from(WORK), &[op])
        .await
        .expect("push ok");
    assert_eq!(results.len(), 1);
    assert!(matches!(results[0].outcome, PushOutcome::Conflict { .. }));

    // The PUT carried the If-Match precondition (doc §3.2).
    let req = client.transport().last_request().expect("a request");
    assert_eq!(req.method, "PUT");
    assert_eq!(req.header_value("If-Match"), Some("\"stale-etag\""));
}

#[tokio::test]
async fn conflict_412_preserves_losing_local_edit() {
    // Base event synced clean.
    let base = event("evt-5", "Original");
    let mut state = LocalCalendarState::new();
    state.apply_pull(ChangeSet {
        changed: vec![remote_event(
            "https://dav.example.com/calendars/alice/work/evt-5.ics",
            {
                let mut b = base.clone();
                b.etag = Some("\"v1\"".to_string());
                b.sequence = 1;
                b.last_modified = Some(Timestamp::from_millis(1000));
                b
            },
        )],
        deleted: vec![],
        next_token: SyncToken::some("t1"),
    });

    // User edits locally (dirty) — a genuine divergent edit.
    let mut local_edit = state.get("evt-5").cloned().unwrap();
    local_edit.title = "My local rename".to_string();
    local_edit.last_modified = Some(Timestamp::from_millis(2000));
    state.local_edit(local_edit.clone());
    assert_eq!(state.dirty_uids(), vec!["evt-5".to_string()]);

    // Push conflicts (simulated 412).
    let transport = MockTransport::with_responses([HttpResponse::new(412, "")]);
    let client = CalDavClient::new(transport, HOME);
    let ops = state.pending_ops();
    let results = client.push(&CalId::from(WORK), &ops).await.unwrap();
    let conflicted = state.apply_push_results(&results);
    assert_eq!(conflicted, vec!["evt-5".to_string()]);
    // Still dirty pending resolution.
    assert_eq!(state.dirty_uids(), vec!["evt-5".to_string()]);

    // Re-pull yields the newer remote (SEQUENCE bumped, LAST-MODIFIED later).
    let mut remote = base.clone();
    remote.title = "Server rename".to_string();
    remote.sequence = 2;
    remote.last_modified = Some(Timestamp::from_millis(3000));
    remote.etag = Some("\"v2\"".to_string());
    state.apply_pull(ChangeSet {
        changed: vec![remote_event(
            "https://dav.example.com/calendars/alice/work/evt-5.ics",
            remote,
        )],
        deleted: vec![],
        next_token: SyncToken::some("t2"),
    });

    // Remote won, but the losing local edit is preserved as a revision, never dropped.
    assert_eq!(
        state.get("evt-5").map(|e| e.title.as_str()),
        Some("Server rename")
    );
    assert_eq!(state.revisions.len(), 1, "local edit preserved");
    assert_eq!(state.revisions[0].title, "My local rename");
    // Conflict resolved: no longer dirty.
    assert!(state.dirty_uids().is_empty());
}

// ---------------------------------------------------------------------------
// 3. conflict::resolve precedence rules (doc §3.2).
// ---------------------------------------------------------------------------

#[test]
fn resolve_prefers_higher_sequence() {
    let mut local = event("u", "local");
    local.sequence = 5;
    local.last_modified = Some(Timestamp::from_millis(1)); // older, but higher seq
    let mut remote = event("u", "remote");
    remote.sequence = 3;
    remote.last_modified = Some(Timestamp::from_millis(9999));

    let out = resolve(&local, &remote, true);
    assert_eq!(out.winner, Winner::Local);
    assert!(out.preserved_local.is_none());
    assert_eq!(out.merged.title, "local");
}

#[test]
fn resolve_breaks_sequence_tie_by_last_modified() {
    let mut local = event("u", "local");
    local.sequence = 2;
    local.last_modified = Some(Timestamp::from_millis(100));
    let mut remote = event("u", "remote");
    remote.sequence = 2;
    remote.last_modified = Some(Timestamp::from_millis(200)); // newer -> wins

    let out = resolve(&local, &remote, true);
    assert_eq!(out.winner, Winner::Remote);
    // Losing dirty local edit is preserved.
    assert_eq!(
        out.preserved_local.as_ref().map(|e| e.title.as_str()),
        Some("local")
    );
}

#[test]
fn resolve_full_tie_goes_remote_and_clean_local_not_preserved() {
    let local = event("u", "same");
    let remote = event("u", "same"); // identical, no last_modified/seq
                                     // Not dirty -> nothing to preserve even though remote wins the tie.
    let out = resolve(&local, &remote, false);
    assert_eq!(out.winner, Winner::Remote);
    assert!(out.preserved_local.is_none());
}

// ---------------------------------------------------------------------------
// 4. Capability reporting — honest, per-platform, no silent downgrade.
// ---------------------------------------------------------------------------

#[test]
fn capability_reporting_is_honest() {
    // CalDAV (Tier B) is the universal full two-way path.
    assert_eq!(
        MockSyncAdapter::caldav().capability(),
        CalendarCapability::CalDav {
            read: true,
            write: true
        }
    );
    let client = CalDavClient::new(MockTransport::new(), HOME);
    assert_eq!(
        client.capability(),
        CalendarCapability::CalDav {
            read: true,
            write: true
        }
    );

    // Native stubs report Unavailable *now* (FFI deferred) — the loud, honest
    // answer that is the opposite of a silent downgrade.
    assert_eq!(
        EventKitAdapter.capability(),
        CalendarCapability::Unavailable
    );
    assert_eq!(EdsAdapter.capability(), CalendarCapability::Unavailable);
    assert_eq!(
        AppointmentManagerAdapter.capability(),
        CalendarCapability::Unavailable
    );

    // Their *planned* per-platform capability differs meaningfully.
    assert_eq!(
        EventKitAdapter.planned_capability(),
        CalendarCapability::Native {
            read: true,
            write: true
        }
    );
    assert_eq!(
        EdsAdapter.planned_capability(),
        CalendarCapability::Native {
            read: true,
            write: true
        }
    );
    assert_eq!(
        AppointmentManagerAdapter.planned_capability(),
        CalendarCapability::Native {
            read: true,
            write: false
        },
        "Windows is honestly read-only, not silently two-way"
    );

    // Helper accessors.
    let c = CalendarCapability::CalDav {
        read: true,
        write: false,
    };
    assert!(c.can_read() && !c.can_write());
    assert_eq!(c.tier(), "caldav");
    assert!(!CalendarCapability::Unavailable.can_read());
    assert_eq!(CalendarCapability::IcsOnly.tier(), "ics");
}

#[tokio::test]
async fn native_stub_ops_error_rather_than_downgrade() {
    let err = EventKitAdapter
        .pull(&CalId::from("x"), &SyncToken::initial())
        .await
        .expect_err("native op must fail honestly");
    let msg = err.to_string();
    assert!(
        msg.contains("not yet implemented") || msg.contains("not supported"),
        "{msg}"
    );
}

// ---------------------------------------------------------------------------
// 5. Full pull -> local-edit -> push round-trip over MockSyncAdapter.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn full_pull_edit_push_roundtrip() {
    let adapter = MockSyncAdapter::caldav();
    let cal = CalId::from(WORK);

    // Scripted incremental pull delivers one event.
    let mut incoming = event("evt-7", "Kickoff");
    incoming.etag = Some("\"srv-1\"".to_string());
    incoming.sequence = 1;
    adapter.script_pull(ChangeSet {
        changed: vec![RemoteEvent {
            href: "https://dav.example.com/calendars/alice/work/evt-7.ics".to_string(),
            event: incoming,
        }],
        deleted: vec![],
        next_token: SyncToken::some("s1"),
    });

    let mut state = LocalCalendarState::new();
    let cs = adapter.pull(&cal, &SyncToken::initial()).await.unwrap();
    state.apply_pull(cs);
    assert_eq!(state.len(), 1);
    assert_eq!(state.sync_token, SyncToken::some("s1"));

    // Local edit -> becomes dirty; sequence bumped by the editing code.
    let mut edited = state.get("evt-7").cloned().unwrap();
    edited.title = "Kickoff (moved)".to_string();
    edited.sequence = 2;
    edited.last_modified = Some(Timestamp::from_millis(5000));
    state.local_edit(edited);
    assert_eq!(state.dirty_uids(), vec!["evt-7".to_string()]);

    // Pending ops carry the known ETag for the If-Match precondition.
    let ops = state.pending_ops();
    assert_eq!(ops.len(), 1);
    match &ops[0] {
        EventOp::Update { etag, event, .. } => {
            assert_eq!(etag.as_deref(), Some("\"srv-1\""));
            assert_eq!(event.title, "Kickoff (moved)");
        }
        other => panic!("expected Update, got {other:?}"),
    }

    // Push succeeds; state clears dirty and records the new etag/href.
    let results = adapter.push(&cal, &ops).await.unwrap();
    assert!(matches!(results[0].outcome, PushOutcome::Written { .. }));
    let conflicts = state.apply_push_results(&results);
    assert!(conflicts.is_empty());
    assert!(state.dirty_uids().is_empty());
    assert_eq!(
        state.get("evt-7").and_then(|e| e.etag.as_deref()),
        Some("\"etag-2\"")
    );

    // The adapter recorded exactly the pushed update.
    let pushed = adapter.pushed_ops();
    assert_eq!(pushed.len(), 1);
    assert_eq!(pushed[0].uid(), "evt-7");
}

// ---------------------------------------------------------------------------
// 6. PROPFIND calendar discovery.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn propfind_lists_calendars() {
    let body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"
  xmlns:cs="http://calendarserver.org/ns/" xmlns:ic="http://apple.com/ns/ical/">
<d:response>
<d:href>/calendars/alice/</d:href>
<d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop>
<d:status>HTTP/1.1 200 OK</d:status></d:propstat>
</d:response>
<d:response>
<d:href>/calendars/alice/work/</d:href>
<d:propstat><d:prop>
<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>
<d:displayname>Work</d:displayname>
<cs:getctag>ctag-123</cs:getctag>
<ic:calendar-color>#ff8800</ic:calendar-color>
</d:prop><d:status>HTTP/1.1 200 OK</d:status></d:propstat>
</d:response>
</d:multistatus>"#;
    let transport = MockTransport::with_responses([HttpResponse::new(207, body)]);
    let client = CalDavClient::new(transport, HOME);

    let cals = client.list_calendars().await.expect("propfind ok");
    // Only the calendar collection is returned (the plain collection is skipped).
    assert_eq!(cals.len(), 1);
    let c = &cals[0];
    assert_eq!(c.name, "Work");
    assert_eq!(c.ctag.as_deref(), Some("ctag-123"));
    assert_eq!(c.color.as_deref(), Some("#ff8800"));
    assert!(c.writable);
    assert!(c.id.as_str().ends_with("/calendars/alice/work/"));

    let req = client.transport().last_request().unwrap();
    assert_eq!(req.method, "PROPFIND");
    assert_eq!(req.header_value("Depth"), Some("1"));
}

// ---------------------------------------------------------------------------
// 7. MockTransport behavior.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_transport_records_and_errors_when_empty() {
    let transport = MockTransport::new();
    transport.push_response(HttpResponse::new(207, sync_collection_body("e", "T")));
    let client = CalDavClient::new(transport, HOME);

    client
        .pull(&CalId::from(WORK), &SyncToken::initial())
        .await
        .unwrap();
    assert_eq!(client.transport().request_count(), 1);

    // No more canned responses -> the next request fails loudly, never hangs.
    let err = client
        .pull(&CalId::from(WORK), &SyncToken::initial())
        .await
        .expect_err("should error with no queued response");
    assert!(err.to_string().contains("no canned response"));
}

#[tokio::test]
async fn full_resync_uses_empty_sync_token() {
    let transport =
        MockTransport::with_responses([HttpResponse::new(207, sync_collection_body("e1", "A"))]);
    let client = CalDavClient::new(transport, HOME);
    client
        .pull(&CalId::from(WORK), &SyncToken::initial())
        .await
        .unwrap();
    let req = client.transport().last_request().unwrap();
    // Initial token serializes to an empty <d:sync-token/> body element.
    assert!(req.body.contains("<d:sync-token></d:sync-token>"));
}

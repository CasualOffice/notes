//! End-to-end checks that the SQL `search` *builds* actually *runs* and ranks
//! correctly against a real (in-memory) FTS5 store. This is the executable proof
//! for the BM25 query builder (Data Model §10) and the filter→SQL compiler
//! (Feature Specs §7.2) — the search crate itself never opens a connection, so
//! these live here with `rusqlite` as a dev-dependency.

use app_domain::{Day, EntityKind, Id};
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection};
use search::build_match_expr;
use search::{
    build_fts_query, DateSpec, Filters, FtsSource, IsFilter, MatchMode, SearchHit, SqlParam,
};

/// Map our connection-free [`SqlParam`] onto a rusqlite bind value.
fn bind(p: &SqlParam) -> Value {
    match p {
        SqlParam::Text(s) => Value::Text(s.clone()),
        SqlParam::Int(i) => Value::Integer(*i),
    }
}

/// Run a compiled FTS query and collect hits.
fn run(conn: &Connection, source: FtsSource, sql: &str, params: &[SqlParam]) -> Vec<SearchHit> {
    let mut stmt = conn.prepare(sql).expect("prepare");
    let vals: Vec<Value> = params.iter().map(bind).collect();
    let rows = stmt
        .query_map(params_from_iter(vals), |row| {
            let id_hex: String = row.get(0)?;
            let kind: String = row.get(1)?;
            let title: Option<String> = row.get(2)?;
            let rank: f64 = row.get(3)?;
            Ok((id_hex, kind, title, rank))
        })
        .expect("query_map");

    rows.map(|r| {
        let (id_hex, kind, title, rank) = r.expect("row");
        // The contentless FTS index yields no snippet column; storage builds the
        // excerpt Rust-side (search::make_snippet). Left empty here.
        SearchHit::from_row_parts(&id_hex, &kind, title, rank, String::new(), source).expect("hit")
    })
    .collect()
}

/// Build the entity spine + note/task detail + FTS5 tables and their side maps.
fn schema(conn: &Connection) {
    conn.execute_batch(
        r#"
        CREATE TABLE entity (
          id BLOB PRIMARY KEY, kind TEXT NOT NULL, title TEXT,
          daily_date TEXT, created_at INTEGER, updated_at INTEGER,
          hlc TEXT, deleted_at INTEGER
        );
        CREATE TABLE note (
          entity_id BLOB PRIMARY KEY, daily_date TEXT, is_pinned INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE task (
          entity_id BLOB PRIMARY KEY, status TEXT NOT NULL DEFAULT 'open',
          start_on TEXT, deadline_on TEXT
        );
        CREATE TABLE tag (entity_id BLOB PRIMARY KEY, name TEXT NOT NULL, display TEXT);
        CREATE TABLE link (
          id BLOB PRIMARY KEY, src_entity BLOB NOT NULL, dst_entity BLOB NOT NULL,
          rel TEXT NOT NULL, deleted_at INTEGER
        );
        CREATE VIRTUAL TABLE fts_note USING fts5(title, body, content='',
          tokenize='unicode61 remove_diacritics 2');
        CREATE TABLE fts_note_map (rowid INTEGER PRIMARY KEY, entity_id BLOB);
        CREATE VIRTUAL TABLE fts_task USING fts5(title, notes_md, content='',
          tokenize='unicode61 remove_diacritics 2');
        CREATE TABLE fts_task_map (rowid INTEGER PRIMARY KEY, entity_id BLOB);
        "#,
    )
    .expect("schema");
}

fn add_entity(conn: &Connection, id: Id, kind: EntityKind, title: &str) {
    conn.execute(
        "INSERT INTO entity(id, kind, title, created_at, updated_at, hlc, deleted_at) \
         VALUES (?, ?, ?, 0, 0, '0', NULL)",
        rusqlite::params![id.as_bytes().to_vec(), kind.as_str(), title],
    )
    .expect("insert entity");
}

fn add_note(conn: &Connection, id: Id, title: &str, body: &str) {
    add_entity(conn, id, EntityKind::Note, title);
    conn.execute(
        "INSERT INTO note(entity_id, is_pinned) VALUES (?, 0)",
        rusqlite::params![id.as_bytes().to_vec()],
    )
    .unwrap();
    let rowid: i64 = {
        conn.execute(
            "INSERT INTO fts_note(title, body) VALUES (?, ?)",
            rusqlite::params![title, body],
        )
        .unwrap();
        conn.last_insert_rowid()
    };
    conn.execute(
        "INSERT INTO fts_note_map(rowid, entity_id) VALUES (?, ?)",
        rusqlite::params![rowid, id.as_bytes().to_vec()],
    )
    .unwrap();
}

fn add_task(
    conn: &Connection,
    id: Id,
    title: &str,
    notes: &str,
    status: &str,
    deadline: Option<&str>,
) {
    add_entity(conn, id, EntityKind::Task, title);
    conn.execute(
        "INSERT INTO task(entity_id, status, deadline_on) VALUES (?, ?, ?)",
        rusqlite::params![id.as_bytes().to_vec(), status, deadline],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO fts_task(title, notes_md) VALUES (?, ?)",
        rusqlite::params![title, notes],
    )
    .unwrap();
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO fts_task_map(rowid, entity_id) VALUES (?, ?)",
        rusqlite::params![rowid, id.as_bytes().to_vec()],
    )
    .unwrap();
}

fn tag_entity(conn: &Connection, src: Id, tag_name: &str) {
    let tag_id = Id::new();
    add_entity(conn, tag_id, EntityKind::Tag, tag_name);
    conn.execute(
        "INSERT INTO tag(entity_id, name, display) VALUES (?, ?, ?)",
        rusqlite::params![
            tag_id.as_bytes().to_vec(),
            tag_name.to_lowercase(),
            tag_name
        ],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO link(id, src_entity, dst_entity, rel, deleted_at) VALUES (?, ?, ?, 'tagged', NULL)",
        rusqlite::params![
            Id::new().as_bytes().to_vec(),
            src.as_bytes().to_vec(),
            tag_id.as_bytes().to_vec()
        ],
    )
    .unwrap();
}

#[test]
fn bm25_ranks_stronger_match_first_and_resolves_entity_refs() {
    let conn = Connection::open_in_memory().unwrap();
    schema(&conn);

    let strong = Id::new();
    let weak = Id::new();
    // `strong` mentions "planning" twice → better BM25 than `weak` (once).
    add_note(
        &conn,
        strong,
        "Quarterly planning",
        "planning planning agenda",
    );
    add_note(&conn, weak, "Random note", "some planning aside");
    add_note(&conn, Id::new(), "Unrelated", "nothing to see");

    let expr = build_match_expr("planning", MatchMode::Exact).unwrap();
    let q = build_fts_query(FtsSource::Note, &expr, &search::WhereClause::empty(), 10);
    let hits = run(&conn, FtsSource::Note, &q.sql, &q.params);

    assert_eq!(hits.len(), 2, "only the two matching notes come back");
    assert_eq!(hits[0].entity.id, strong, "stronger match ranks first");
    assert_eq!(hits[1].entity.id, weak);
    assert_eq!(hits[0].entity.kind, EntityKind::Note);
    // bm25 is negative; better (first) hit is more negative.
    assert!(hits[0].bm25 <= hits[1].bm25);
}

#[test]
fn task_filter_is_open_and_tag_restricts_results() {
    let conn = Connection::open_in_memory().unwrap();
    schema(&conn);

    let open_work = Id::new();
    let done_work = Id::new();
    let open_untagged = Id::new();
    add_task(
        &conn,
        open_work,
        "Ship report",
        "quarterly report",
        "open",
        None,
    );
    add_task(
        &conn,
        done_work,
        "Ship report",
        "quarterly report",
        "completed",
        None,
    );
    add_task(
        &conn,
        open_untagged,
        "Ship report",
        "quarterly report",
        "open",
        None,
    );
    tag_entity(&conn, open_work, "Work");
    tag_entity(&conn, done_work, "Work");
    // open_untagged has no tag.

    let filters = Filters {
        is: vec![IsFilter::Open],
        tags: vec!["Work".to_string()],
        ..Default::default()
    };
    let today = "2026-07-23".parse::<Day>().unwrap();
    let where_c = filters.compile_for(FtsSource::Task, today);

    let expr = build_match_expr("report", MatchMode::Exact).unwrap();
    let q = build_fts_query(FtsSource::Task, &expr, &where_c, 10);
    let hits = run(&conn, FtsSource::Task, &q.sql, &q.params);

    // Only the open, Work-tagged task survives (done_work excluded by is:open;
    // open_untagged excluded by tag:Work).
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, open_work);
}

#[test]
fn task_date_overdue_filter_executes() {
    let conn = Connection::open_in_memory().unwrap();
    schema(&conn);

    let overdue = Id::new();
    let future = Id::new();
    add_task(
        &conn,
        overdue,
        "Pay invoice",
        "invoice due",
        "open",
        Some("2026-07-01"),
    );
    add_task(
        &conn,
        future,
        "Pay invoice",
        "invoice due",
        "open",
        Some("2026-12-01"),
    );

    let filters = Filters {
        date: Some(DateSpec::Overdue),
        ..Default::default()
    };
    let today = "2026-07-23".parse::<Day>().unwrap();
    let where_c = filters.compile_for(FtsSource::Task, today);
    let expr = build_match_expr("invoice", MatchMode::Exact).unwrap();
    let q = build_fts_query(FtsSource::Task, &expr, &where_c, 10);
    let hits = run(&conn, FtsSource::Task, &q.sql, &q.params);

    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].entity.id, overdue);
}

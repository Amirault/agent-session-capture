//! §9.19 WarpConversationReader.readSessions — enumeration, filters, skip-unchanged.

use super::fixtures::TempDir;
use super::fixtures::fake_source::fake_source;
use super::fixtures::fixture_db::{
    BlockSeed, QuerySeed, create_fixture, seed_block, seed_conversation, seed_query, seed_task,
};
use super::fixtures::protobuf::encode_string;
use crate::adapters::warp_conversation_reader::WarpConversationReader;
use crate::domain::models::{BundleSource, EventKind, SessionHeader};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{ReadSessionsOptions, SessionReader, SessionsRead};

fn stored_header(conversation_id: &str, last_modified: Option<&str>) -> SessionHeader {
    SessionHeader::fresh(
        conversation_id,
        BundleSource::Warp,
        last_modified,
        "2026-07-01T00:00:00.000Z",
    )
}

/// A temp dir holding a fresh fixture DB `f.db` (the `beforeEach`).
fn setup() -> (TempDir, String, rusqlite::Connection) {
    let tmp = TempDir::new("wa-919-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    (tmp, db_path, db)
}

fn read_sessions(db_path: &str, options: ReadSessionsOptions) -> SessionsRead {
    WarpConversationReader::new(fake_source(db_path))
        .read_sessions(&options)
        .expect("read sessions")
}

fn session_ids(read: &SessionsRead) -> Vec<&str> {
    read.sessions.iter().map(|s| s.conversation_id.as_str()).collect()
}

fn query(conversation_id: &'static str, start_ts: &'static str, text: &'static str) -> QuerySeed<'static> {
    QuerySeed {
        conversation_id,
        start_ts,
        text,
        ..QuerySeed::default()
    }
}

#[test]
fn enumeration_unions_both_durable_tables_and_takes_the_max_stamp() {
    // Given — c1 in both tables (tasks stamp later), c2 conversations-only, c3 tasks-only
    let (_tmp, db_path, db) = setup();
    seed_conversation(&db, "c1", Some("2026-06-30 10:00:00.000000"));
    seed_task(
        &db,
        "c1",
        &encode_string(1, "c1 task"),
        None,
        Some("2026-06-30 10:30:00.000000"),
    );
    seed_conversation(&db, "c2", Some("2026-07-01 09:00:00.000000"));
    seed_task(
        &db,
        "c3",
        &encode_string(1, "c3 task"),
        None,
        Some("2026-07-02 08:00:00.000000"),
    );

    // When
    let read = read_sessions(&db_path, ReadSessionsOptions::default());

    // Then
    assert_eq!(read.source, BundleSource::Warp);
    let stamp = |cid: &str| {
        read.sessions
            .iter()
            .find(|s| s.conversation_id == cid)
            .and_then(|s| s.last_modified.clone())
    };
    assert_eq!(stamp("c1").as_deref(), Some("2026-06-30 10:30:00.000000")); // MAX across both tables
    assert_eq!(stamp("c2").as_deref(), Some("2026-07-01 09:00:00.000000"));
    assert_eq!(stamp("c3").as_deref(), Some("2026-07-02 08:00:00.000000")); // listed without an agent_conversations row
}

#[test]
fn since_enumerates_only_conversations_at_or_after_the_cutoff() {
    // Given
    let (_tmp, db_path, db) = setup();
    seed_conversation(&db, "c1", Some("2026-06-30 10:00:00.000000"));
    seed_conversation(&db, "c2", Some("2026-07-15 09:00:00.000000"));
    let since = |cutoff: &str| ReadSessionsOptions {
        since: Some(cutoff.to_string()),
        ..ReadSessionsOptions::default()
    };

    // When
    let after_july = read_sessions(&db_path, since("2026-07-01"));
    let at_boundary = read_sessions(&db_path, since("2026-07-15T09:00:00Z"));
    let after_june = read_sessions(&db_path, since("2026-06-01"));

    // Then
    assert_eq!(session_ids(&after_july), vec!["c2"]);
    assert_eq!(session_ids(&at_boundary), vec!["c2"]); // at/after, inclusive
    assert_eq!(session_ids(&after_june), vec!["c1", "c2"]);
}

#[test]
fn gather_collects_queries_blocks_and_tasks_for_each_cid_only_without_any_marker() {
    // Given — c1 with all three event kinds, its task row tagged like a subagent task; c2 separate
    let (_tmp, db_path, db) = setup();
    seed_conversation(&db, "c1", Some("2026-06-30 10:00:00.000000"));
    seed_query(&db, query("c1", "2026-06-30 10:10:00.000000", "fix the login"));
    seed_block(
        &db,
        BlockSeed {
            conversation_id: "c1",
            start_ts: "2026-06-30 10:20:00.000000",
            command: Some("npm test"),
            ..BlockSeed::default()
        },
    );
    seed_task(
        &db,
        "c1",
        &encode_string(1, "subagent message"),
        Some("sub-1"),
        Some("2026-06-30 10:30:00.000000"),
    );
    seed_conversation(&db, "c2", Some("2026-07-01 09:00:00.000000"));
    seed_query(&db, query("c2", "2026-07-01 09:10:00.000000", "other conversation"));

    // When
    let read = read_sessions(&db_path, ReadSessionsOptions::default());

    // Then — no marker rows exist anywhere; both conversations still enumerate and gather
    let marker_rows: i64 = db
        .query_row(
            "SELECT COUNT(*) AS n FROM commands WHERE command LIKE '%CAPTURE_MARKER%'",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(marker_rows, 0);

    let c1 = read.reads.get(&"c1".to_string()).expect("c1");
    let kinds: Vec<EventKind> = c1.drafts.iter().map(|d| d.kind).collect();
    assert_eq!(
        kinds,
        vec![EventKind::Query, EventKind::Command, EventKind::AgentMessage]
    ); // fixed reader order
    assert!(c1.drafts.iter().all(|d| d.conversation_id == "c1")); // only c1's rows
    assert_eq!(c1.drafts[2].meta.get_str("task_id"), Some("sub-1")); // the subagent task rolls up via the shared cid
    assert_eq!(c1.last_modified.as_deref(), Some("2026-06-30 10:30:00.000000"));

    let c2 = read.reads.get(&"c2".to_string()).expect("c2");
    let rows: Vec<(&str, EventKind, &str)> = c2
        .drafts
        .iter()
        .map(|d| (d.conversation_id.as_str(), d.kind, d.content.as_str()))
        .collect();
    assert_eq!(rows, vec![("c2", EventKind::Query, "other conversation")]);
    assert_eq!(read.reads.len(), 2);
}

#[test]
fn only_changed_or_new_conversations_are_gathered_and_conversation_filter_forces_a_gather() {
    // Given — c1 stored with an equal stamp, c2 stored with an older stamp, c3 not stored
    let (_tmp, db_path, db) = setup();
    seed_conversation(&db, "c1", Some("2026-06-30 10:00:00.000000"));
    seed_query(&db, query("c1", "2026-06-30 10:10:00.000000", "same"));
    seed_conversation(&db, "c2", Some("2026-07-01 09:00:00.000000"));
    seed_query(&db, query("c2", "2026-07-01 09:10:00.000000", "changed"));
    seed_conversation(&db, "c3", Some("2026-07-02 09:00:00.000000"));
    seed_query(&db, query("c3", "2026-07-02 09:10:00.000000", "new"));
    let mut stored_by_cid = OrderedMap::new();
    stored_by_cid.insert(
        "c1".to_string(),
        stored_header("c1", Some("2026-06-30 10:00:00.000000")),
    );
    stored_by_cid.insert(
        "c2".to_string(),
        stored_header("c2", Some("2026-06-30 08:00:00.000000")),
    );

    // When
    let read = read_sessions(
        &db_path,
        ReadSessionsOptions {
            stored_by_cid: stored_by_cid.clone(),
            ..ReadSessionsOptions::default()
        },
    );

    // Then — equal stamp skipped, different and new gathered
    assert!(!read.reads.contains_key(&"c1".to_string()));
    assert!(read.reads.contains_key(&"c2".to_string()));
    assert!(read.reads.contains_key(&"c3".to_string()));

    // And — the --conversation debug filter always gathers its conversation
    let forced = read_sessions(
        &db_path,
        ReadSessionsOptions {
            conversation: Some("c1".to_string()),
            stored_by_cid,
            ..ReadSessionsOptions::default()
        },
    );
    assert_eq!(session_ids(&forced), vec!["c1"]);
    assert!(forced.reads.contains_key(&"c1".to_string()));
}

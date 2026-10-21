//! §9.5 queryReader & blockReader.

use super::fixtures::TempDir;
use super::fixtures::fixture_db::{BlockSeed, QuerySeed, create_fixture, seed_block, seed_query};
use crate::adapters::readers::block_reader::read_blocks;
use crate::adapters::readers::query_reader::read_queries;
use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::models::{EventKind, Role};
use crate::js::Value;

const CID: &str = "c-1";

/// A temp dir holding a fresh fixture DB `f.db` (the `beforeEach`).
fn setup() -> (TempDir, String, rusqlite::Connection) {
    let tmp = TempDir::new("wa-95-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    (tmp, db_path, db)
}

fn readable(db_path: &str) -> ReadableDb {
    ReadableDb::new(rusqlite::Connection::open(db_path).expect("open"))
}

fn ids() -> Vec<String> {
    vec![CID.to_string()]
}

#[test]
fn query_text_yields_a_user_query_event_with_cwd_git_and_model_metadata() {
    // Given — a block before the query provides git_branch context
    let (_tmp, db_path, db) = setup();
    seed_block(
        &db,
        BlockSeed {
            conversation_id: CID,
            start_ts: "2026-06-30 10:00:00.000000",
            command: Some("git status"),
            git_branch: Some("feature-x"),
            ..BlockSeed::default()
        },
    );
    seed_query(
        &db,
        QuerySeed {
            conversation_id: CID,
            start_ts: "2026-06-30 10:05:00.000000",
            text: "fix the bug",
            working_directory: Some("/repo"),
            model_id: Some("claude-x"),
            ..QuerySeed::default()
        },
    );

    // When
    let result = read_queries(&readable(&db_path), &ids()).expect("read");

    // Then
    assert!(result.skipped.is_empty());
    assert_eq!(result.drafts.len(), 1);
    let draft = &result.drafts[0];
    assert_eq!(draft.role, Role::User);
    assert_eq!(draft.kind, EventKind::Query);
    assert_eq!(draft.conversation_id, CID);
    assert_eq!(draft.content, "fix the bug");
    assert_eq!(draft.meta.get_str("cwd"), Some("/repo"));
    assert_eq!(draft.meta.get_str("model"), Some("claude-x"));
    assert_eq!(draft.meta.get_str("git_branch"), Some("feature-x"));
    let expected_exchange = format!("{CID}-ex");
    assert_eq!(draft.meta.get_str("exchange_id"), Some(expected_exchange.as_str()));
}

#[test]
fn query_input_failing_the_schema_is_skipped_and_extraction_continues() {
    // Given — a malformed input row plus a valid one
    let (_tmp, db_path, db) = setup();
    db.execute(
        "INSERT INTO ai_queries (exchange_id, conversation_id, start_ts, input, working_directory, output_status, model_id)
       VALUES (?, ?, ?, ?, ?, '', ?)",
        rusqlite::params!["ex-bad", CID, "2026-06-30 10:00:00.000000", "[{\"NotQuery\":{}}]", "/repo", "m"],
    )
    .expect("insert");
    seed_query(
        &db,
        QuerySeed {
            conversation_id: CID,
            start_ts: "2026-06-30 10:05:00.000000",
            text: "good query",
            ..QuerySeed::default()
        },
    );

    // When
    let result = read_queries(&readable(&db_path), &ids()).expect("read");

    // Then
    assert_eq!(result.drafts.len(), 1);
    assert_eq!(result.drafts[0].content, "good query");
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].table, "ai_queries");
}

#[test]
fn ansi_block_output_is_stripped_into_a_clean_command_tool_event() {
    // Given
    let (_tmp, db_path, db) = setup();
    seed_block(
        &db,
        BlockSeed {
            conversation_id: CID,
            start_ts: "2026-06-30 10:00:00.000000",
            command: Some("git pull origin main"),
            output: Some("Already up to date.\nDone"),
            git_branch: Some("main"),
            ..BlockSeed::default()
        },
    );

    // When
    let result = read_blocks(&readable(&db_path), &ids()).expect("read");

    // Then
    assert!(result.skipped.is_empty());
    assert_eq!(result.drafts.len(), 1);
    let draft = &result.drafts[0];
    assert_eq!(draft.role, Role::Tool);
    assert_eq!(draft.kind, EventKind::Command);
    assert_eq!(draft.conversation_id, CID);
    assert_eq!(draft.content, "git pull origin main");
    assert_eq!(draft.meta.get_str("output"), Some("Already up to date.\nDone"));
    assert_eq!(draft.meta.get("block_id"), Some(&Value::Number(1.0)));
}

#[test]
fn no_conversation_ids_return_empty_results() {
    // Given
    let (_tmp, db_path, _db) = setup();
    let db = readable(&db_path);

    // When
    let queries = read_queries(&db, &[]).expect("queries");
    let blocks = read_blocks(&db, &[]).expect("blocks");

    // Then
    assert!(queries.drafts.is_empty() && queries.skipped.is_empty());
    assert!(blocks.drafts.is_empty() && blocks.skipped.is_empty());
}

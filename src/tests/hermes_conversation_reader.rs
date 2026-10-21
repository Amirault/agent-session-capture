//! Port of `hermesConversationReader.test.ts` (in-process cases; the CLI cases
//! live in `tests/hermes_conversation_reader_cli.rs`).

use rusqlite::Connection;

use super::fixtures::TempDir;
use super::hermes_fixture::{
    MessageOptions, SPEC, SessionOptions, calls, command_args, create_hermes_db, insert_message, marker, seed_marker,
    seed_session, seed_text, seed_tool_calls, tool_call,
};
use crate::adapters::hermes_conversation_reader::HermesConversationReader;
use crate::domain::models::{BundleSource, EventDraft, EventKind, Role};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{CaptureRead, ConversationReader};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value};
use crate::usecases::extract_bundle::{ExtractOptions, ExtractResult, extract_bundle};

struct Fixture {
    _temp: TempDir,
    db_path: String,
}

fn fixture() -> Fixture {
    let temp = TempDir::new("capture-hermes-test-");
    let db_path = temp.join("state.db");
    Fixture { _temp: temp, db_path }
}

fn extract(db_path: &str) -> Result<ExtractResult, ReadError> {
    extract_bundle(
        &HermesConversationReader::new(Some(db_path.to_string())),
        SPEC,
        ExtractOptions::default(),
    )
}

fn content(event: &Object) -> &str {
    event.get_str("content").unwrap_or_default()
}

fn meta(event: &Object) -> &Object {
    event.get("meta").and_then(Value::as_object).expect("meta object")
}

fn implement_count(result: &ExtractResult) -> Option<f64> {
    result
        .bundle
        .header
        .get("conversations_per_label")
        .and_then(Value::as_object)
        .and_then(|counts| counts.get("implement"))
        .and_then(Value::as_f64)
}

fn assert_binds_marker_inside_multiline_call(name: &str) {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    seed_session(&db, "multiline-session", SessionOptions::default());
    let command = format!("cd /tmp\n{}\npwd", marker("implement"));
    seed_tool_calls(
        &db,
        "multiline-session",
        1_784_000_000.0,
        &calls(&[tool_call("marker-call", name, &command_args(&command))]),
        None,
    );
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    assert_eq!(implement_count(&result), Some(1.0));
}

#[test]
fn given_an_exact_marker_inside_a_multiline_assistant_terminal_call_when_extracting_should_bind() {
    assert_binds_marker_inside_multiline_call("terminal");
}

#[test]
fn given_an_exact_marker_inside_a_multiline_assistant_run_shell_command_call_when_extracting_should_bind() {
    assert_binds_marker_inside_multiline_call("run_shell_command");
}

/// (session id, role for a plain message, tool calls JSON, content)
type NoiseCase<'a> = (&'a str, Option<&'a str>, Option<String>, Option<String>);

#[test]
fn given_marker_shaped_noise_when_extracting_should_reject_every_non_canonical_execution() {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    let exact = marker("implement");
    let terminal = |command: String| Some(calls(&[tool_call("x", "terminal", &command_args(&command))]));
    let cases: Vec<NoiseCase> = vec![
        ("user-text", Some("user"), None, Some(exact.clone())),
        ("assistant-prose", Some("assistant"), None, Some(exact.clone())),
        ("tool-result", Some("tool"), None, Some(exact.clone())),
        (
            "other-tool",
            None,
            Some(calls(&[tool_call("x", "read_file", &command_args(&exact))])),
            None,
        ),
        ("comment", None, terminal(format!("# {exact}")), None),
        ("echo", None, terminal(format!("echo '{exact}'")), None),
        ("quoted", None, terminal(format!("'{exact}'")), None),
        ("prefix", None, terminal(format!("prefix {exact}")), None),
        ("suffix", None, terminal(format!("{exact} suffix")), None),
        (
            "wrong-order",
            None,
            terminal(format!(": CAPTURE_MARKER id={SPEC} v=1 label=implement")),
            None,
        ),
        ("wrong-version", None, terminal(exact.replacen("v=1", "v=2", 1)), None),
    ];
    for (index, (id, role, tool_calls, content)) in cases.iter().enumerate() {
        seed_session(&db, id, SessionOptions::default());
        let timestamp = 1_784_000_000.0 + index as f64;
        match tool_calls {
            Some(tool_calls) => seed_tool_calls(&db, id, timestamp, tool_calls, content.as_deref()),
            None => insert_message(
                &db,
                MessageOptions {
                    content: content.as_deref(),
                    ..MessageOptions::new(id, role.expect("role"), timestamp)
                },
            ),
        }
    }
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    assert!(result.bundle.events.is_empty());
    assert_eq!(result.summary.conversations, 0);
}

#[test]
fn given_parallel_and_malformed_tool_calls_when_extracting_should_keep_valid_events_and_report_skips() {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    seed_session(&db, "tool-session", SessionOptions::default());
    seed_tool_calls(
        &db,
        "tool-session",
        1_784_000_000.0,
        &calls(&[
            tool_call("marker-call", "terminal", &command_args(&marker("implement"))),
            r#"{"id":"invalid-sibling","type":"function","function":{"name":42,"arguments":"{}"}}"#.to_string(),
            tool_call("read-call", "read_file", r#"{"path":"README.md"}"#),
            tool_call("search-call", "search_files", r#"{"pattern":"Hermes"}"#),
        ]),
        Some("I will inspect both sources."),
    );
    seed_tool_calls(
        &db,
        "tool-session",
        1_784_000_001.0,
        "{not-json",
        Some("Text survives malformed calls."),
    );
    insert_message(
        &db,
        MessageOptions {
            content: Some("README contents"),
            tool_call_id: Some("read-call"),
            tool_name: Some("read_file"),
            ..MessageOptions::new("tool-session", "tool", 1_784_000_002.0)
        },
    );
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    let events = &result.bundle.events;
    let role_kinds: Vec<(&str, &str)> = events
        .iter()
        .map(|event| {
            (
                event.get_str("role").unwrap_or_default(),
                event.get_str("kind").unwrap_or_default(),
            )
        })
        .collect();
    for expected in [
        ("assistant", "agent_message"),
        ("assistant", "tool_call"),
        ("tool", "tool_result"),
    ] {
        assert!(role_kinds.contains(&expected), "missing {expected:?} in {role_kinds:?}");
    }
    let tool_call_ids: Vec<&str> = events
        .iter()
        .filter(|event| event.get_str("kind") == Some("tool_call"))
        .filter_map(|event| meta(event).get_str("tool_call_id"))
        .collect();
    for expected in ["marker-call", "read-call", "search-call"] {
        assert!(
            tool_call_ids.contains(&expected),
            "missing {expected} in {tool_call_ids:?}"
        );
    }
    assert!(
        events
            .iter()
            .any(|event| content(event) == "Text survives malformed calls.")
    );
    assert_eq!(result.summary.skipped_rows.len(), 2);
}

#[test]
fn given_workflow_and_non_workflow_children_when_extracting_should_apply_nearest_marked_lineage() {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    seed_session(
        &db,
        "root",
        SessionOptions {
            end_reason: Some("compression"),
            ..Default::default()
        },
    );
    seed_marker(&db, "root", "implement", 1_784_000_000.0);
    seed_marker(&db, "root", "implement", 1_784_000_001.0);
    let child = |id: &str, parent: &str, source: Option<&str>, model_config: Option<&str>| {
        seed_session(
            &db,
            id,
            SessionOptions {
                parent: Some(parent),
                source,
                model_config,
                ..Default::default()
            },
        );
    };
    child("compression-child", "root", None, None);
    child("delegate-child", "root", Some("subagent"), None);
    child(
        "branch-child",
        "delegate-child",
        None,
        Some(r#"{"_branched_from":"delegate-child"}"#),
    );
    child("generic-child", "delegate-child", None, None);
    child(
        "null-delegate-child",
        "delegate-child",
        None,
        Some(r#"{"_delegate_from":null}"#),
    );
    child("tool-child", "delegate-child", Some("tool"), None);
    child(
        "explicit-branch",
        "delegate-child",
        None,
        Some(r#"{"_branched_from":"delegate-child"}"#),
    );
    seed_marker(&db, "explicit-branch", "review", 1_784_000_002.0);
    let ids = [
        "compression-child",
        "delegate-child",
        "branch-child",
        "generic-child",
        "null-delegate-child",
        "tool-child",
        "explicit-branch",
    ];
    for (index, id) in ids.iter().enumerate() {
        seed_text(&db, id, id, 1_784_000_010.0 + index as f64);
    }
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    let mut labels: Vec<(String, String)> = Vec::new();
    for event in &result.bundle.events {
        if ids.contains(&content(event)) {
            let label = event.get_str("label").unwrap_or_default().to_string();
            match labels.iter_mut().find(|(c, _)| c == content(event)) {
                Some(entry) => entry.1 = label,
                None => labels.push((content(event).to_string(), label)),
            }
        }
    }
    labels.sort();
    assert_eq!(
        labels,
        vec![
            ("compression-child".to_string(), "implement".to_string()),
            ("delegate-child".to_string(), "implement".to_string()),
            ("explicit-branch".to_string(), "review".to_string()),
        ]
    );
    assert_eq!(implement_count(&result), Some(3.0));
}

#[test]
fn given_a_conflicting_marker_session_when_resolving_descendants_should_block_inheritance_and_report_collision() {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    seed_session(
        &db,
        "root",
        SessionOptions {
            end_reason: Some("compression"),
            ..Default::default()
        },
    );
    seed_marker(&db, "root", "specify", 1_784_000_000.0);
    let subagent = |id: &str, parent: &str| {
        seed_session(
            &db,
            id,
            SessionOptions {
                parent: Some(parent),
                source: Some("subagent"),
                ..Default::default()
            },
        );
    };
    subagent("conflict", "root");
    seed_marker(&db, "conflict", "specify", 1_784_000_001.0);
    seed_marker(&db, "conflict", "implement", 1_784_000_002.0);
    subagent("blocked", "conflict");
    seed_text(&db, "blocked", "blocked", 1_784_000_003.0);
    subagent("restarted", "conflict");
    seed_marker(&db, "restarted", "review", 1_784_000_004.0);
    seed_text(&db, "restarted", "restarted", 1_784_000_005.0);
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    let events = &result.bundle.events;
    assert!(!events.iter().any(|event| content(event) == "blocked"));
    assert_eq!(
        events
            .iter()
            .find(|event| content(event) == "restarted")
            .and_then(|event| event.get_str("label")),
        Some("review")
    );
    assert!(
        result.summary.skipped_rows.iter().any(|row| row.table == "messages"
            && row.reason == "same session has markers for multiple labels"
            && row.detail == "conflict: specify, implement"),
        "skipped rows: {:?}",
        result.summary.skipped_rows
    );
}

#[test]
fn given_active_compacted_and_parallel_hermes_messages_when_extracting_should_preserve_history_and_identifiers() {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    seed_session(&db, "history-session", SessionOptions::default());
    seed_marker(&db, "history-session", "implement", 1_784_000_000.0);
    insert_message(
        &db,
        MessageOptions {
            content: Some("inactive original"),
            active: 0,
            compacted: 1,
            ..MessageOptions::new("history-session", "user", 1_784_000_001.0)
        },
    );
    insert_message(
        &db,
        MessageOptions {
            content: Some("active summary"),
            active: 1,
            compacted: 0,
            ..MessageOptions::new("history-session", "user", 1_784_000_002.0)
        },
    );
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    let events = &result.bundle.events;
    let original = meta(
        events
            .iter()
            .find(|e| content(e) == "inactive original")
            .expect("original"),
    );
    let summary = meta(events.iter().find(|e| content(e) == "active summary").expect("summary"));
    assert_eq!(original.get("active"), Some(&Value::Number(0.0)));
    assert_eq!(original.get("compacted"), Some(&Value::Number(1.0)));
    assert_eq!(original.get_str("session_source"), Some("cli"));
    assert_eq!(summary.get("active"), Some(&Value::Number(1.0)));
    assert_eq!(summary.get("compacted"), Some(&Value::Number(0.0)));
    assert_eq!(summary.get_str("session_source"), Some("cli"));
    assert!(matches!(original.get("message_id"), Some(Value::Number(_))));
}

#[test]
fn given_a_clock_regression_within_one_conversation_when_extracting_should_keep_tool_call_before_result() {
    // Given
    let f = fixture();
    let db = create_hermes_db(&f.db_path, "delete");
    seed_session(&db, "clock-session", SessionOptions::default());
    seed_marker(&db, "clock-session", "implement", 1_784_000_000.0);
    seed_tool_calls(
        &db,
        "clock-session",
        1_784_000_020.0,
        &calls(&[tool_call("clock-call", "read_file", r#"{"path":"README.md"}"#)]),
        None,
    );
    insert_message(
        &db,
        MessageOptions {
            content: Some("clock-result"),
            tool_call_id: Some("clock-call"),
            tool_name: Some("read_file"),
            ..MessageOptions::new("clock-session", "tool", 1_784_000_010.0)
        },
    );
    drop(db);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    let kinds: Vec<&str> = result
        .bundle
        .events
        .iter()
        .filter(|event| meta(event).get_str("tool_call_id") == Some("clock-call"))
        .filter_map(|event| event.get_str("kind"))
        .collect();
    assert_eq!(kinds, vec!["tool_call", "tool_result"]);
}

struct FixedReader(CaptureRead);

impl ConversationReader for FixedReader {
    fn read_capture(&self, _capture_id: &str) -> Result<CaptureRead, ReadError> {
        Ok(self.0.clone())
    }
}

fn draft(conversation_id: &str, ts: &str, role: Role, kind: EventKind, content: &str, message_id: i64) -> EventDraft {
    let mut meta = Object::new();
    meta.insert("message_id", message_id);
    meta.insert("message_event_index", 0i64);
    EventDraft {
        conversation_id: conversation_id.to_string(),
        ts: ts.to_string(),
        role,
        kind,
        content: content.to_string(),
        meta,
    }
}

#[test]
fn given_causal_drafts_across_conversations_when_extracting_should_use_a_transitive_global_order() {
    // Given
    let mut label_by_cid = OrderedMap::new();
    for cid in ["a", "b", "c"] {
        label_by_cid.insert(cid.to_string(), "implement".to_string());
    }
    let reader = FixedReader(CaptureRead {
        source: BundleSource::Hermes,
        label_by_cid,
        drafts: vec![
            draft(
                "a",
                "2026-07-15T00:00:20.000Z",
                Role::Assistant,
                EventKind::ToolCall,
                "call-a",
                1,
            ),
            draft(
                "a",
                "2026-07-15T00:00:10.000Z",
                Role::Tool,
                EventKind::ToolResult,
                "result-a",
                2,
            ),
            draft(
                "b",
                "2026-07-15T00:00:15.000Z",
                Role::User,
                EventKind::Query,
                "conversation-b",
                3,
            ),
            draft(
                "c",
                "2026-07-15T00:00:12.000Z",
                Role::User,
                EventKind::Query,
                "conversation-c",
                4,
            ),
        ],
        skipped: vec![],
        unbindable: vec![],
        collisions: vec![],
        heuristic_bindings: vec![],
    });

    // When
    let result = extract_bundle(&reader, SPEC, ExtractOptions::default()).expect("extract");

    // Then
    let contents: Vec<&str> = result.bundle.events.iter().map(content).collect();
    assert_eq!(contents, vec!["conversation-c", "conversation-b", "call-a", "result-a"]);
}

#[test]
fn given_committed_wal_frames_when_reading_the_live_db_should_see_them_without_writing() {
    // Given
    let f = fixture();
    let writer = create_hermes_db(&f.db_path, "wal");
    writer
        .pragma_update(None, "wal_autocheckpoint", 0)
        .expect("autocheckpoint");
    writer
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .expect("checkpoint");
    seed_session(&writer, "wal-session", SessionOptions::default());
    seed_marker(&writer, "wal-session", "specify", 1_784_000_000.0);
    seed_text(&writer, "wal-session", "committed in WAL", 1_784_000_001.0);

    // When
    let result = extract(&f.db_path).expect("extract");

    // Then
    assert!(
        result
            .bundle
            .events
            .iter()
            .any(|event| content(event) == "committed in WAL")
    );
    let query_only: i64 = writer
        .query_row("PRAGMA query_only", [], |row| row.get(0))
        .expect("query_only");
    assert_eq!(query_only, 0);
    drop(writer);
}

fn assert_incompatible_db_names_path_and_schema(sql: &str, expected: &str) {
    // Given
    let f = fixture();
    let db = Connection::open(&f.db_path).expect("open");
    db.execute_batch(sql).expect("schema");
    drop(db);

    // When / Then
    let first = extract(&f.db_path).err().expect("first extraction fails");
    assert!(first.message().contains(&f.db_path), "{}", first.message());
    let second = extract(&f.db_path).err().expect("second extraction fails");
    assert!(second.message().contains(expected), "{}", second.message());
}

#[test]
fn given_an_incompatible_db_with_missing_sessions_table_when_extracting_should_name_path_and_schema() {
    assert_incompatible_db_names_path_and_schema("CREATE TABLE messages (id INTEGER)", "table sessions");
}

#[test]
fn given_an_incompatible_db_with_missing_messages_tool_calls_column_when_extracting_should_name_path_and_schema() {
    assert_incompatible_db_names_path_and_schema(
        "CREATE TABLE sessions (id TEXT, source TEXT, parent_session_id TEXT); CREATE TABLE messages (id INTEGER, session_id TEXT, role TEXT, content TEXT, timestamp REAL)",
        "messages.tool_calls",
    );
}

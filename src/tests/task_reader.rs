//! §9.6 taskReader.

use super::fixtures::TempDir;
use super::fixtures::fixture_db::{create_fixture, seed_task};
use super::fixtures::protobuf::{encode_message, encode_string, encode_tag, encode_varint};
use crate::adapters::readers::task_reader::read_tasks;
use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::models::{EventDraft, EventKind, Role};
use crate::js::Value;

const CID: &str = "c-1";

fn context_envelope_task(turn: u32, rule_content: &str) -> Vec<u8> {
    let rule_file = [encode_string(1, "/repo/AGENTS.md"), encode_string(2, rule_content)].concat();
    let project_rules = encode_message(2, &rule_file);
    let skills = [
        encode_message(
            1,
            &[encode_string(1, "skills/plan/SKILL.md"), encode_string(2, "plan")].concat(),
        ),
        encode_message(
            1,
            &[encode_string(1, "skills/review/SKILL.md"), encode_string(2, "review")].concat(),
        ),
    ]
    .concat();
    let input_context = [encode_message(10, &project_rules), encode_message(12, &skills)].concat();
    let envelope = encode_message(
        5,
        &encode_message(
            5,
            &[
                encode_string(1, &format!("context-{turn}")),
                encode_message(11, &input_context),
            ]
            .concat(),
        ),
    );
    let mixed_envelope = encode_message(
        5,
        &encode_message(
            5,
            &[
                encode_string(1, &format!("mixed-{turn}")),
                encode_message(2, &encode_string(1, &format!("output {turn}"))),
                encode_message(11, &input_context),
            ]
            .concat(),
        ),
    );
    let user_query = encode_message(5, &encode_message(2, &encode_string(1, &format!("query {turn}"))));
    let agent_reasoning = encode_message(5, &encode_message(15, &encode_string(1, &format!("reasoning {turn}"))));
    let update_todos = encode_message(
        5,
        &encode_message(
            10,
            &encode_message(1, &encode_message(1, &encode_string(2, &format!("todo {turn}")))),
        ),
    );
    let received_message = encode_message(
        5,
        &encode_message(
            24,
            &encode_message(1, &encode_string(4, &format!("agent update {turn}"))),
        ),
    );
    let tool_call = encode_message(
        5,
        &encode_message(
            4,
            &[
                encode_string(1, &format!("command-{turn}")),
                encode_message(2, &encode_string(1, &format!("echo turn-{turn}"))),
            ]
            .concat(),
        ),
    );
    [
        envelope,
        mixed_envelope,
        user_query,
        agent_reasoning,
        update_todos,
        received_message,
        tool_call,
    ]
    .concat()
}

const DEFAULT_RULES: &str = "# Shared project rules\nKeep tests focused.";

/// A temp dir holding a fresh fixture DB `f.db` (the `beforeEach`).
fn setup() -> (TempDir, String, rusqlite::Connection) {
    let tmp = TempDir::new("wa-96-");
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

fn field_keys(draft: &EventDraft) -> Vec<String> {
    draft
        .meta
        .get("fields")
        .and_then(Value::as_object)
        .map(|f| f.keys().cloned().collect())
        .unwrap_or_default()
}

#[test]
fn task_blob_with_description_dependencies_summary_yields_agent_messages_with_semantic_paths() {
    // Given — Task: description(1), dependencies.parent_task_id(3.1), summary(6)
    let (_tmp, db_path, db) = setup();
    let blob = [
        encode_string(1, "Task title"),
        encode_message(3, &encode_string(1, "parent-task-id")),
        encode_string(6, "command output"),
    ]
    .concat();
    seed_task(&db, CID, &blob, None, Some("2026-06-30 12:00:00.000000"));

    // When
    let result = read_tasks(&readable(&db_path), &ids()).expect("read");

    // Then
    assert!(result.skipped.is_empty());
    let drafts = &result.drafts;
    let contents: Vec<&str> = drafts.iter().map(|d| d.content.as_str()).collect();
    assert_eq!(contents, vec!["Task title", "parent-task-id", "command output"]);
    assert_eq!(drafts[0].conversation_id, CID);
    assert_eq!(drafts[0].role, Role::Assistant);
    assert_eq!(drafts[0].kind, EventKind::AgentMessage);
    assert_eq!(drafts[0].ts, "2026-06-30 12:00:00.000000");
    // field 1 = Task.id, field 3 = Task.dependencies, field 6 = Task.summary
    assert_eq!(drafts[0].meta.get_str("field_path"), Some("id"));
    assert_eq!(
        drafts[1].meta.get_str("field_path"),
        Some("dependencies.parent_task_id")
    );
    let task_ids: Vec<Option<&str>> = drafts.iter().map(|d| d.meta.get_str("task_id")).collect();
    let expected_task_id = format!("{CID}-task");
    assert_eq!(task_ids, vec![Some(expected_task_id.as_str()); 3]);
    let indexes: Vec<Option<f64>> = drafts
        .iter()
        .map(|d| d.meta.get("task_event_index").and_then(Value::as_f64))
        .collect();
    assert_eq!(indexes, vec![Some(0.0), Some(1.0), Some(2.0)]);
    assert!(!drafts[0].meta.contains_key("confidence"));
}

#[test]
fn repeated_static_context_envelopes_appear_once_and_every_signal_event_survives() {
    // Given
    let (_tmp, db_path, db) = setup();
    for turn in 1..=3 {
        seed_task(
            &db,
            CID,
            &context_envelope_task(turn, DEFAULT_RULES),
            Some(&format!("task-{turn}")),
            Some(&format!("2026-06-30 12:0{turn}:00.000000")),
        );
    }

    // When
    let drafts = read_tasks(&readable(&db_path), &ids()).expect("read").drafts;

    // Then
    assert_eq!(
        drafts
            .iter()
            .filter(|d| d.meta.get("skills").and_then(Value::as_array).is_some())
            .count(),
        1
    );
    assert_eq!(
        drafts
            .iter()
            .filter(|d| field_keys(d).iter().any(|f| f.starts_with("context.project_rules.")))
            .count(),
        1
    );
    for message_kind in [
        "user_query",
        "agent_reasoning",
        "update_todos",
        "messages_received_from_agents",
    ] {
        assert_eq!(
            drafts
                .iter()
                .filter(|d| d.meta.get_str("message_kind") == Some(message_kind))
                .count(),
            3
        );
    }
    assert_eq!(drafts.len(), 19);
    assert_eq!(
        drafts
            .iter()
            .filter(|d| d.meta.get_str("message_kind") == Some("tool_call")
                && d.meta.get_str("tool") == Some("run_shell_command"))
            .count(),
        3
    );
    let command_outputs: Vec<&Value> = drafts
        .iter()
        .filter_map(|d| {
            d.meta
                .get("fields")
                .and_then(Value::as_object)
                .and_then(|f| f.get("run_shell_command.output"))
        })
        .collect();
    assert_eq!(command_outputs.len(), 3);
    assert_eq!(
        command_outputs,
        vec![
            &Value::from("output 1"),
            &Value::from("output 2"),
            &Value::from("output 3")
        ]
    );
    let contents: Vec<&str> = drafts.iter().map(|d| d.content.as_str()).collect();
    for expected in [
        "query 1",
        "query 2",
        "query 3",
        "reasoning 1",
        "reasoning 2",
        "reasoning 3",
        "todo 1",
        "todo 2",
        "todo 3",
        "agent update 1",
        "agent update 2",
        "agent update 3",
    ] {
        assert!(contents.contains(&expected), "missing {expected}");
    }
}

#[test]
fn project_rules_changing_between_task_rows_keep_both_rule_versions() {
    // Given
    let (_tmp, db_path, db) = setup();
    seed_task(
        &db,
        CID,
        &context_envelope_task(1, "rules version one"),
        Some("task-1"),
        Some("2026-06-30 12:01:00.000000"),
    );
    seed_task(
        &db,
        CID,
        &context_envelope_task(2, "rules version two"),
        Some("task-2"),
        Some("2026-06-30 12:02:00.000000"),
    );

    // When
    let drafts = read_tasks(&readable(&db_path), &ids()).expect("read").drafts;

    // Then
    let rule_contents: Vec<&Value> = drafts
        .iter()
        .filter_map(|d| {
            d.meta
                .get("fields")
                .and_then(Value::as_object)
                .and_then(|f| f.get("context.project_rules.active_rule_files.content"))
        })
        .collect();
    assert_eq!(
        rule_contents,
        vec![&Value::from("rules version one"), &Value::from("rules version two")]
    );
}

#[test]
fn malformed_task_blob_returns_partial_results_marked_heuristic() {
    // Given — a recoverable string then a truncated length-delimited field
    let (_tmp, db_path, db) = setup();
    let blob = [
        encode_string(1, "recovered text"),
        encode_tag(2, 2),
        encode_varint(100),
        b"abc".to_vec(),
    ]
    .concat();
    seed_task(&db, CID, &blob, None, None);

    // When
    let drafts = read_tasks(&readable(&db_path), &ids()).expect("read").drafts;

    // Then
    let ev = drafts
        .iter()
        .find(|d| d.content == "recovered text")
        .expect("recovered");
    assert_eq!(ev.meta.get_str("field_path"), Some("id"));
    assert_eq!(ev.meta.get_str("confidence"), Some("heuristic"));
}

#[test]
fn empty_task_blob_is_skipped_and_the_run_continues() {
    // Given
    let (_tmp, db_path, db) = setup();
    seed_task(&db, CID, &[], None, None);

    // When
    let result = read_tasks(&readable(&db_path), &ids()).expect("read");

    // Then
    assert!(result.drafts.is_empty());
    assert_eq!(result.skipped.len(), 1);
    assert_eq!(result.skipped[0].table, "agent_tasks");
    assert_eq!(result.skipped[0].reason, "empty task");
}

#[test]
fn no_conversation_ids_return_empty_results() {
    // Given
    let (_tmp, db_path, _db) = setup();

    // When
    let result = read_tasks(&readable(&db_path), &[]).expect("read");

    // Then
    assert!(result.drafts.is_empty());
    assert!(result.skipped.is_empty());
}

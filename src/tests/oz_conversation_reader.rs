//! Headless Warp `oz agent run` sessions, read from Warp's server through `oz`.

use std::cell::RefCell;
use std::rc::Rc;

use super::fixtures::TempDir;
use super::fixtures::oz_runs::{
    CONVERSATION_ID, MODEL_ID, RUN_ID, action_result, command_result, conversation, conversation_json,
    marked_conversation, message, run, run_command, run_json, step, text,
};
use crate::adapters::oz_conversation_reader::{OzConversationReader, drafts_from_run};
use crate::adapters::oz_run_store::{OzCommand, OzRunStore, OzRuns};
use crate::domain::models::{EventDraft, EventKind, Role, SessionHeader};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{ConversationReader, ReadSessionsOptions, SessionReader};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value};

const MARKER: &str = ": CAPTURE_MARKER v=1 id=greeting-fix label=build";

/// In-memory store: fixed runs and their conversations.
struct FakeStore {
    runs: Vec<(Object, Object)>,
    fetched: Rc<RefCell<Vec<String>>>,
}

impl OzRunStore for FakeStore {
    fn runs(&self) -> Result<Vec<Object>, ReadError> {
        Ok(self.runs.iter().map(|(r, _)| r.clone()).collect())
    }

    fn conversation(&self, run: &Object) -> Result<Object, ReadError> {
        let run_id = run.get_str("run_id").unwrap_or_default().to_string();
        self.fetched.borrow_mut().push(run_id.clone());
        Ok(self
            .runs
            .iter()
            .find(|(r, _)| r.get_str("run_id") == Some(run_id.as_str()))
            .map(|(_, c)| c.clone())
            .expect("known run"))
    }
}

fn reader(runs: Vec<(Object, Object)>) -> (OzConversationReader<FakeStore>, Rc<RefCell<Vec<String>>>) {
    let fetched = Rc::new(RefCell::new(Vec::new()));
    (
        OzConversationReader::new(FakeStore {
            runs,
            fetched: fetched.clone(),
        }),
        fetched,
    )
}

fn marked_run(run_id: &str, cid: &str, marker: &str) -> (Object, Object) {
    (
        run(run_id, cid, "2026-10-20T09:10:00.000000Z"),
        marked_conversation(cid, marker),
    )
}

fn meta_str<'a>(draft: &'a EventDraft, key: &str) -> Option<&'a str> {
    draft.meta.get_str(key)
}

#[test]
fn given_a_run_when_mapped_should_emit_query_message_command_and_tool_events_in_order() {
    // Given
    let (run, conversation) = marked_run(RUN_ID, CONVERSATION_ID, MARKER);

    // When
    let drafts = drafts_from_run(&run, &conversation);

    // Then
    let shape: Vec<(Role, EventKind, &str)> = drafts.iter().map(|d| (d.role, d.kind, d.content.as_str())).collect();
    assert_eq!(
        shape,
        vec![
            (Role::User, EventKind::Query, "Make the greeting say hello"),
            (Role::Assistant, EventKind::AgentMessage, "Tagging the session first."),
            (Role::Assistant, EventKind::Command, MARKER),
            (Role::Tool, EventKind::ToolResult, ""),
            (
                Role::Assistant,
                EventKind::ToolCall,
                r#"{"files":[{"name":"src/greeting.txt"}]}"#
            ),
            (
                Role::Tool,
                EventKind::ToolResult,
                r#"{"success":{"files":[{"file":{"path":"src/greeting.txt","content":"hi"}}]}}"#
            ),
        ]
    );
    assert!(drafts.iter().all(|d| d.conversation_id == CONVERSATION_ID));
    for d in &drafts {
        let expected = (d.role == Role::Assistant).then_some(MODEL_ID);
        assert_eq!(meta_str(d, "model"), expected, "{:?}", d.kind);
    }
    assert!(drafts.iter().all(|d| meta_str(d, "run_id") == Some(RUN_ID)));
    assert_eq!(meta_str(&drafts[0], "exchange_id"), Some("req-1"));
    assert_eq!(meta_str(&drafts[4], "exchange_id"), Some("req-2"));
}

#[test]
fn given_actions_and_results_when_mapped_should_carry_tool_ids_fields_and_exit_codes() {
    // Given
    let (run, conversation) = marked_run(RUN_ID, CONVERSATION_ID, MARKER);

    // When
    let drafts = drafts_from_run(&run, &conversation);

    // Then
    let command = &drafts[2];
    assert_eq!(meta_str(command, "tool"), Some("run_command"));
    assert_eq!(meta_str(command, "tool_call_id"), Some("call-1"));
    let fields = command.meta.get("fields").and_then(Value::as_object).expect("fields");
    assert_eq!(fields.get_str("command"), Some(MARKER));
    let result = &drafts[3];
    assert_eq!(meta_str(result, "tool_call_id"), Some("call-1"));
    assert_eq!(result.meta.get("exit_code"), Some(&Value::Number(0.0)));
    let read = &drafts[4];
    assert_eq!(meta_str(read, "tool"), Some("read_files"));
    assert_eq!(meta_str(read, "tool_call_id"), Some("call-2"));
    assert!(read.meta.get("fields").and_then(Value::as_object).is_some());
    assert_eq!(meta_str(&drafts[5], "tool_call_id"), Some("call-2"));
    assert_eq!(drafts[5].meta.get("exit_code"), None);
}

#[test]
fn given_a_run_with_usage_when_mapped_should_attach_it_once_on_the_first_event() {
    // Given
    let (run, conversation) = marked_run(RUN_ID, CONVERSATION_ID, MARKER);

    // When
    let drafts = drafts_from_run(&run, &conversation);

    // Then
    let usage = drafts[0].meta.get("usage").and_then(Value::as_object).expect("usage");
    assert_eq!(usage.get("inference_cost_usd"), Some(&Value::Number(0.25)));
    assert_eq!(usage.get("total_tokens"), Some(&Value::Number(1500.0)));
    assert!(drafts[1..].iter().all(|d| d.meta.get("usage").is_none()));
}

#[test]
fn given_nanosecond_timestamps_when_mapped_should_pad_them_to_a_sortable_ts() {
    // Given
    let (run, conversation) = marked_run(RUN_ID, CONVERSATION_ID, MARKER);

    // When
    let drafts = drafts_from_run(&run, &conversation);

    // Then
    let ts: Vec<&str> = drafts.iter().map(|d| d.ts.as_str()).collect();
    assert_eq!(
        ts,
        vec![
            "2026-10-20T09:00:01.500000000Z",
            "2026-10-20T09:00:02.123456789Z",
            "2026-10-20T09:00:02.123456789Z",
            "2026-10-20T09:00:03.000000000Z",
            "2026-10-20T09:00:04.250000000Z",
            "2026-10-20T09:00:05.000000000Z",
        ]
    );
}

#[test]
fn given_nested_steps_when_mapped_should_interleave_their_messages_by_time() {
    // Given
    let nested = step(
        &[message(
            "assistant",
            "2026-10-20T09:00:02Z",
            "req-1",
            &[text("from the nested step")],
        )],
        &[],
    );
    let conversation = conversation(
        CONVERSATION_ID,
        &[step(
            &[
                message("user", "2026-10-20T09:00:01Z", "req-1", &[text("first")]),
                message("assistant", "2026-10-20T09:00:03Z", "req-1", &[text("last")]),
            ],
            &[nested],
        )],
    );

    // When
    let drafts = drafts_from_run(&run(RUN_ID, CONVERSATION_ID, "2026-10-20T09:10:00Z"), &conversation);

    // Then
    let contents: Vec<&str> = drafts.iter().map(|d| d.content.as_str()).collect();
    assert_eq!(contents, vec!["first", "from the nested step", "last"]);
}

#[test]
fn given_a_long_command_output_when_mapped_should_truncate_it_with_the_shared_rule() {
    // Given
    let output = "x".repeat(2500);
    let conversation = conversation(
        CONVERSATION_ID,
        &[step(
            &[
                message(
                    "assistant",
                    "2026-10-20T09:00:01Z",
                    "req-1",
                    &[run_command("c", "cat big.log")],
                ),
                message(
                    "tool",
                    "2026-10-20T09:00:02Z",
                    "req-1",
                    &[command_result("c", "cat big.log", &output, 1)],
                ),
            ],
            &[],
        )],
    );

    // When
    let drafts = drafts_from_run(&run(RUN_ID, CONVERSATION_ID, "2026-10-20T09:10:00Z"), &conversation);

    // Then
    let result = &drafts[1];
    assert_eq!(
        result.content,
        format!("{}…[truncated len=2500]…{}", "x".repeat(1000), "x".repeat(500))
    );
    assert_eq!(result.meta.get("truncated"), Some(&Value::Bool(true)));
    assert_eq!(result.meta.get("original_len"), Some(&Value::Number(2500.0)));
    assert_eq!(result.meta.get("exit_code"), Some(&Value::Number(1.0)));
}

#[test]
fn given_unmapped_content_when_mapped_should_ignore_it() {
    // Given
    let event = r#"{"type":"event","name":"create_todo_list","data":{"initial_todos":[]}}"#.to_string();
    let conversation = conversation(
        CONVERSATION_ID,
        &[step(
            &[
                message("system", "2026-10-20T09:00:01Z", "req-1", &[event]),
                message("user", "2026-10-20T09:00:02Z", "req-1", &[text("go")]),
                message(
                    "tool",
                    "2026-10-20T09:00:03Z",
                    "req-1",
                    &[action_result("a", "read_skill", r#"{"error":{"message":"no skill"}}"#)],
                ),
            ],
            &[],
        )],
    );

    // When
    let drafts = drafts_from_run(&run(RUN_ID, CONVERSATION_ID, "2026-10-20T09:10:00Z"), &conversation);

    // Then
    let kinds: Vec<EventKind> = drafts.iter().map(|d| d.kind).collect();
    assert_eq!(kinds, vec![EventKind::Query, EventKind::ToolResult]);
    assert_eq!(drafts[1].content, r#"{"error":{"message":"no skill"}}"#);
}

#[test]
fn given_runs_when_reading_a_capture_should_bind_only_the_conversations_that_ran_its_marker() {
    // Given
    let (reader, _) = reader(vec![
        marked_run("run-a", "conv-a", MARKER),
        marked_run("run-b", "conv-b", ": CAPTURE_MARKER v=1 id=other-task"),
        marked_run("run-c", "conv-c", "echo ': CAPTURE_MARKER v=1 id=greeting-fix'"),
    ]);

    // When
    let read = reader.read_capture("greeting-fix").expect("read");

    // Then
    assert_eq!(read.source.as_str(), "oz");
    let bound: Vec<(&String, &String)> = read.label_by_cid.iter().collect();
    assert_eq!(bound, vec![(&"conv-a".to_string(), &"build".to_string())]);
    assert!(read.drafts.iter().all(|d| d.conversation_id == "conv-a"));
    assert_eq!(read.drafts.len(), 6);
    assert!(read.skipped.is_empty());
}

#[test]
fn given_a_conversation_marked_with_two_labels_when_reading_a_capture_should_skip_it_with_a_warning() {
    // Given
    let conversation = conversation(
        "conv-a",
        &[step(
            &[message(
                "assistant",
                "2026-10-20T09:00:01Z",
                "req-1",
                &[
                    run_command("c1", ": CAPTURE_MARKER v=1 id=greeting-fix label=plan"),
                    run_command("c2", ": CAPTURE_MARKER v=1 id=greeting-fix label=build"),
                ],
            )],
            &[],
        )],
    );
    let (reader, _) = reader(vec![(run("run-a", "conv-a", "2026-10-20T09:10:00Z"), conversation)]);

    // When
    let read = reader.read_capture("greeting-fix").expect("read");

    // Then
    assert!(read.label_by_cid.is_empty());
    assert_eq!(read.skipped.len(), 1);
    assert_eq!(read.skipped[0].detail, "conv-a: plan, build");
}

fn stored(cid: &str, last_modified: &str) -> OrderedMap<String, SessionHeader> {
    let mut map = OrderedMap::new();
    map.insert(
        cid.to_string(),
        SessionHeader::fresh(cid, crate::domain::models::BundleSource::Oz, Some(last_modified), "x"),
    );
    map
}

#[test]
fn given_runs_when_reading_sessions_should_stamp_each_conversation_and_skip_unchanged_ones() {
    // Given
    let (reader, fetched) = reader(vec![
        (
            run("run-b", "conv-b", "2026-10-21T08:00:00.000000Z"),
            marked_conversation("conv-b", MARKER),
        ),
        (
            run("run-a", "conv-a", "2026-10-20T08:00:00.000000Z"),
            marked_conversation("conv-a", MARKER),
        ),
    ]);
    let options = ReadSessionsOptions {
        stored_by_cid: stored("conv-a", "2026-10-20T08:00:00.000000Z"),
        ..Default::default()
    };

    // When
    let read = reader.read_sessions(&options).expect("read");

    // Then
    let sessions: Vec<(&str, Option<&str>)> = read
        .sessions
        .iter()
        .map(|s| (s.conversation_id.as_str(), s.last_modified.as_deref()))
        .collect();
    assert_eq!(
        sessions,
        vec![
            ("conv-a", Some("2026-10-20T08:00:00.000000Z")),
            ("conv-b", Some("2026-10-21T08:00:00.000000Z")),
        ]
    );
    assert_eq!(
        read.reads.keys().cloned().collect::<Vec<_>>(),
        vec!["conv-b".to_string()]
    );
    assert_eq!(*fetched.borrow(), vec!["run-b".to_string()]);
}

#[test]
fn given_since_when_reading_sessions_should_keep_only_runs_updated_after_it() {
    // Given
    let (reader, _) = reader(vec![
        marked_run("run-a", "conv-a", MARKER),
        (
            run("run-b", "conv-b", "2026-10-25T08:00:00.000000Z"),
            marked_conversation("conv-b", MARKER),
        ),
    ]);
    let options = ReadSessionsOptions {
        since: Some("2026-10-22".to_string()),
        ..Default::default()
    };

    // When
    let read = reader.read_sessions(&options).expect("read");

    // Then
    let cids: Vec<&str> = read.sessions.iter().map(|s| s.conversation_id.as_str()).collect();
    assert_eq!(cids, vec!["conv-b"]);
}

/// Records every `oz` invocation and answers from a script.
struct FakeOz {
    calls: Rc<RefCell<Vec<Vec<String>>>>,
    fail: bool,
}

impl OzCommand for FakeOz {
    fn output(&self, args: &[&str]) -> Result<String, ReadError> {
        self.calls
            .borrow_mut()
            .push(args.iter().map(|a| a.to_string()).collect());
        if self.fail {
            return Err(ReadError::SourceAccess("oz run get failed: not logged in".to_string()));
        }
        Ok(match args {
            ["run", "get", id, ..] => run_json(id, CONVERSATION_ID, "2026-10-20T09:10:00Z"),
            ["run", "conversation", "get", cid, ..] => conversation_json(cid, &[]),
            _ => panic!("unexpected oz call {args:?}"),
        })
    }
}

#[test]
fn given_run_ids_when_listing_runs_should_shell_out_to_oz_for_the_run_and_its_conversation() {
    // Given
    let calls = Rc::new(RefCell::new(Vec::new()));
    let store = OzRuns::with_command(
        vec![RUN_ID.to_string()],
        None,
        FakeOz {
            calls: calls.clone(),
            fail: false,
        },
    );

    // When
    let runs = store.runs().expect("runs");
    let conversation = store.conversation(&runs[0]).expect("conversation");

    // Then
    assert_eq!(runs[0].get_str("run_id"), Some(RUN_ID));
    assert_eq!(conversation.get_str("conversation_id"), Some(CONVERSATION_ID));
    assert_eq!(
        *calls.borrow(),
        vec![
            vec!["run", "get", RUN_ID, "--output-format", "json"],
            vec!["run", "conversation", "get", CONVERSATION_ID, "--output-format", "json"],
        ]
    );
}

#[test]
fn given_oz_fails_when_listing_runs_should_raise_a_source_access_error() {
    // Given
    let store = OzRuns::with_command(
        vec![RUN_ID.to_string()],
        None,
        FakeOz {
            calls: Rc::new(RefCell::new(Vec::new())),
            fail: true,
        },
    );

    // When
    let error = store.runs().expect_err("oz failed");

    // Then
    assert!(error.is_source_access());
    assert_eq!(error.exit_code(), 1);
}

#[test]
fn given_a_run_id_shaped_like_a_flag_when_listing_runs_should_refuse_it() {
    // Given
    let store = OzRuns::with_command(
        vec!["--help".to_string()],
        None,
        FakeOz {
            calls: Rc::new(RefCell::new(Vec::new())),
            fail: false,
        },
    );

    // When
    let error = store.runs().expect_err("refused");

    // Then
    assert_eq!(error.exit_code(), 2);
    assert!(error.message().contains("--help"), "{}", error.message());
}

#[test]
fn given_a_directory_of_saved_runs_when_listing_runs_should_read_each_run_and_conversation() {
    // Given
    let temp = TempDir::new("oz-saved-runs-");
    let saved = format!(
        r#"{{"run":{},"conversation":{}}}"#,
        run_json("run-a", "conv-a", "2026-10-20T09:10:00Z"),
        conversation_json(
            "conv-a",
            &[step(
                &[message("user", "2026-10-20T09:00:01Z", "r", &[text("hi")])],
                &[]
            )]
        )
    );
    std::fs::write(temp.join("20261020-conv-a.json"), saved).expect("write");
    std::fs::write(temp.join("notes.txt"), "not a run").expect("write");
    let store = OzRuns::with_command(
        vec![],
        Some(temp.path()),
        FakeOz {
            calls: Rc::new(RefCell::new(Vec::new())),
            fail: true,
        },
    );

    // When
    let runs = store.runs().expect("runs");
    let conversation = store.conversation(&runs[0]).expect("conversation");

    // Then
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].get_str("run_id"), Some("run-a"));
    assert_eq!(conversation.get_str("conversation_id"), Some("conv-a"));
}

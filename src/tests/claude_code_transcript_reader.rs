//! Port of `claudeCodeTranscriptReader.test.ts` (in-process cases; the CLI case
//! lives in `tests/claude_code_transcript_reader_cli.rs`).

use super::fixtures::TempDir;
use crate::adapters::claude_code_transcript_reader::ClaudeCodeTranscriptReader;
use crate::domain::models::SkippedRow;
use crate::domain::ports::ConversationReader;
use crate::js::{Object, Value};
use crate::usecases::extract_bundle::{ExtractOptions, ExtractResult, extract_bundle};

const SPEC: &str = "2026-06-30-multiquote-limit-5";

fn obj(pairs: Vec<(&str, Value)>) -> Object {
    let mut object = Object::new();
    for (key, value) in pairs {
        object.insert(key, value);
    }
    object
}

fn s(text: &str) -> Value {
    Value::from(text)
}

fn arr(items: Vec<Object>) -> Value {
    Value::from(items.into_iter().map(Value::from).collect::<Vec<_>>())
}

fn write_lines(path: &std::path::Path, entries: &[Object]) {
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let body: String = entries.iter().map(|e| format!("{}\n", e.to_json())).collect();
    std::fs::write(path, body).expect("write transcript");
}

fn write_transcript(root: &str, session_id: &str, entries: &[Object]) {
    let path = std::path::Path::new(root)
        .join("-Users-dev-acme-app")
        .join(format!("{session_id}.jsonl"));
    write_lines(&path, entries);
}

fn write_subagent_transcript(root: &str, session_id: &str, agent_id: &str, entries: &[Object]) {
    let path = std::path::Path::new(root)
        .join("-Users-dev-acme-app")
        .join(session_id)
        .join("subagents")
        .join(format!("agent-{agent_id}.jsonl"));
    write_lines(&path, entries);
}

fn subagent_entry(agent_id: &str, mut entry: Object) -> Object {
    entry.insert("agentId", agent_id);
    entry.insert("isSidechain", true);
    entry
}

fn user_entry(session_id: &str, uuid: &str, timestamp: &str, content: Value) -> Object {
    obj(vec![
        ("type", s("user")),
        ("uuid", s(uuid)),
        ("parentUuid", Value::Null),
        ("sessionId", s(session_id)),
        ("cwd", s("/Users/dev/acme/app")),
        ("gitBranch", s("main")),
        ("version", s("2.1.202")),
        (
            "message",
            Value::from(obj(vec![("role", s("user")), ("content", content)])),
        ),
        ("timestamp", s(timestamp)),
    ])
}

fn assistant_entry(session_id: &str, uuid: &str, timestamp: &str, content: Vec<Object>) -> Object {
    obj(vec![
        ("type", s("assistant")),
        ("uuid", s(uuid)),
        ("parentUuid", Value::Null),
        ("sessionId", s(session_id)),
        ("cwd", s("/Users/dev/acme/app")),
        ("gitBranch", s("main")),
        ("version", s("2.1.202")),
        (
            "message",
            Value::from(obj(vec![
                ("id", s(&format!("msg_{uuid}"))),
                ("type", s("message")),
                ("role", s("assistant")),
                ("model", s("claude-sonnet-4-5-20250929")),
                ("content", arr(content)),
                (
                    "usage",
                    Value::from(obj(vec![
                        ("input_tokens", Value::from(42usize)),
                        ("output_tokens", Value::from(7usize)),
                    ])),
                ),
            ])),
        ),
        ("timestamp", s(timestamp)),
    ])
}

fn tool_result_entry(session_id: &str, uuid: &str, timestamp: &str, tool_use_id: &str, content: &str) -> Object {
    user_entry(
        session_id,
        uuid,
        timestamp,
        arr(vec![obj(vec![
            ("type", s("tool_result")),
            ("tool_use_id", s(tool_use_id)),
            ("content", s(content)),
        ])]),
    )
}

fn text_block(text: &str) -> Object {
    obj(vec![("type", s("text")), ("text", s(text))])
}

fn tool_use(id: &str, name: &str, input: Object) -> Object {
    obj(vec![
        ("type", s("tool_use")),
        ("id", s(id)),
        ("name", s(name)),
        ("input", Value::from(input)),
    ])
}

fn bash(id: &str, command: &str) -> Object {
    tool_use(id, "Bash", obj(vec![("command", s(command))]))
}

fn marker_command(label: &str) -> String {
    format!(": CAPTURE_MARKER v=1 id={SPEC} label={label}")
}

struct Env {
    _tmp: TempDir,
    claude_root: String,
}

fn env() -> Env {
    let tmp = TempDir::new("wa-claude-");
    let claude_root = tmp.join(".claude/projects");
    Env { _tmp: tmp, claude_root }
}

fn extract(claude_root: &str) -> ExtractResult {
    extract_bundle(
        &ClaudeCodeTranscriptReader::new(Some(claude_root.to_string())),
        SPEC,
        ExtractOptions::default(),
    )
    .expect("extract bundle")
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

/// `expect(events).toContainEqual(expect.objectContaining({...fields, meta: objectContaining(meta)}))`.
fn contains_event(events: &[Object], fields: &[(&str, &str)], meta: &[(&str, &str)]) -> bool {
    events.iter().any(|event| {
        fields.iter().all(|(k, v)| event.get_str(k) == Some(*v))
            && (meta.is_empty()
                || event
                    .get("meta")
                    .and_then(Value::as_object)
                    .is_some_and(|m| meta.iter().all(|(k, v)| m.get_str(k) == Some(*v))))
    })
}

fn reasons(rows: &[SkippedRow]) -> Vec<&str> {
    rows.iter().map(|r| r.reason.as_str()).collect()
}

#[test]
fn transcripts_with_spec_markers_are_grouped_by_label_with_claude_events_preserved() {
    // Given — one Claude Code session per label, each bound by the existing CAPTURE_MARKER no-op command.
    let Env { _tmp, claude_root } = env();
    write_transcript(
        &claude_root,
        "session-specify",
        &[
            user_entry(
                "session-specify",
                "u1",
                "2026-06-30T09:00:00.000Z",
                s(&format!("please specify {SPEC}")),
            ),
            assistant_entry(
                "session-specify",
                "a1",
                "2026-06-30T09:00:01.000Z",
                vec![
                    text_block("I will write the spec."),
                    bash("toolu_spec", &marker_command("specify")),
                ],
            ),
            tool_result_entry(
                "session-specify",
                "r1",
                "2026-06-30T09:00:02.000Z",
                "toolu_spec",
                "done",
            ),
        ],
    );
    write_transcript(
        &claude_root,
        "session-implement",
        &[
            user_entry(
                "session-implement",
                "u2",
                "2026-06-30T10:00:00.000Z",
                s(&format!("implement {SPEC}")),
            ),
            assistant_entry(
                "session-implement",
                "a2",
                "2026-06-30T10:00:01.000Z",
                vec![
                    bash("toolu_impl", &marker_command("implement")),
                    bash("toolu_test", "npm test"),
                ],
            ),
            tool_result_entry(
                "session-implement",
                "r2",
                "2026-06-30T10:00:02.000Z",
                "toolu_test",
                "all tests passed",
            ),
        ],
    );
    write_transcript(
        &claude_root,
        "session-gate",
        &[
            user_entry(
                "session-gate",
                "u3",
                "2026-06-30T11:00:00.000Z",
                s(&format!("validate {SPEC}")),
            ),
            assistant_entry(
                "session-gate",
                "a3",
                "2026-06-30T11:00:01.000Z",
                vec![
                    text_block("Gate is green."),
                    bash("toolu_gate", &marker_command("review")),
                ],
            ),
        ],
    );

    // When
    let ExtractResult { bundle, summary } = extract(&claude_root);

    // Then
    assert_eq!(bundle.header.get_str("source"), Some("claude-code"));
    assert_eq!(
        strings(bundle.header.get("conversation_ids")),
        vec!["session-specify", "session-implement", "session-gate"]
    );
    assert_eq!(
        bundle
            .header
            .get("conversations_per_label")
            .map(Value::to_json)
            .as_deref(),
        Some(r#"{"specify":1,"implement":1,"review":1}"#)
    );
    assert_eq!(summary.skipped_rows, vec![]);

    let events = &bundle.events;
    let seqs: Vec<f64> = events
        .iter()
        .map(|e| e.get("seq").and_then(Value::as_f64).expect("seq"))
        .collect();
    let expected_seqs: Vec<f64> = (1..=events.len()).map(|i| i as f64).collect();
    assert_eq!(seqs, expected_seqs);
    let ts: Vec<&str> = events.iter().map(|e| e.get_str("ts").expect("ts")).collect();
    let mut sorted_ts = ts.clone();
    sorted_ts.sort_unstable();
    assert_eq!(ts, sorted_ts);

    assert!(contains_event(
        events,
        &[
            ("conversation_id", "session-specify"),
            ("label", "specify"),
            ("role", "user"),
            ("kind", "query"),
            ("content", &format!("please specify {SPEC}")),
        ],
        &[("cwd", "/Users/dev/acme/app"), ("git_branch", "main")],
    ));
    assert!(contains_event(
        events,
        &[
            ("conversation_id", "session-implement"),
            ("label", "implement"),
            ("role", "assistant"),
            ("kind", "tool_call"),
            ("content", "npm test"),
        ],
        &[("tool", "Bash"), ("tool_use_id", "toolu_test")],
    ));
    assert!(contains_event(
        events,
        &[
            ("conversation_id", "session-implement"),
            ("label", "implement"),
            ("role", "tool"),
            ("kind", "tool_result"),
            ("content", "all tests passed"),
        ],
        &[("tool_use_id", "toolu_test")],
    ));
}

#[test]
fn marker_only_mentioned_in_a_prompt_is_not_bound_as_a_captured_label() {
    // Given — Claude Code records failed unauthenticated prompts too; a text mention
    // must not count as the label marker unless Claude actually emitted the Bash tool_use.
    let Env { _tmp, claude_root } = env();
    write_transcript(
        &claude_root,
        "session-mention-only",
        &[
            user_entry(
                "session-mention-only",
                "u1",
                "2026-06-30T09:00:00.000Z",
                s(&marker_command("specify")),
            ),
            assistant_entry(
                "session-mention-only",
                "a1",
                "2026-06-30T09:00:01.000Z",
                vec![text_block("Not logged in · Please run /login")],
            ),
        ],
    );

    // When
    let ExtractResult { bundle, summary } = extract(&claude_root);

    // Then
    assert_eq!(bundle.header.get_str("source"), Some("claude-code"));
    assert_eq!(strings(bundle.header.get("conversation_ids")), Vec::<String>::new());
    assert_eq!(summary.conversations, 0);
    assert_eq!(summary.events, 0);
}

#[test]
fn marker_shaped_command_on_a_non_bash_tool_use_is_not_bound() {
    // Given — CAPTURE_MARKER is a shell no-op; only Bash tool_use blocks prove it was emitted.
    let Env { _tmp, claude_root } = env();
    write_transcript(
        &claude_root,
        "session-non-bash",
        &[assistant_entry(
            "session-non-bash",
            "a1",
            "2026-06-30T09:00:00.000Z",
            vec![tool_use(
                "toolu_write",
                "Write",
                obj(vec![
                    ("command", s(&marker_command("specify"))),
                    ("file_path", s("notes.md")),
                ]),
            )],
        )],
    );

    // When
    let ExtractResult { summary, .. } = extract(&claude_root);

    // Then
    assert_eq!(summary.conversations, 0);
    assert_eq!(summary.events, 0);
}

#[test]
fn marker_shaped_bash_tool_use_outside_an_assistant_turn_is_not_bound() {
    // Given — Claude emits tool_use blocks from assistant turns; user/tool-result turns must not seed labels.
    let Env { _tmp, claude_root } = env();
    write_transcript(
        &claude_root,
        "session-user-tool-use",
        &[user_entry(
            "session-user-tool-use",
            "u1",
            "2026-06-30T09:00:00.000Z",
            arr(vec![bash("toolu_user", &marker_command("specify"))]),
        )],
    );

    // When
    let ExtractResult { summary, .. } = extract(&claude_root);

    // Then
    assert_eq!(summary.conversations, 0);
    assert_eq!(summary.events, 0);
}

#[test]
fn timestamp_less_metadata_records_are_not_skipped_only_message_records_without_timestamp_are() {
    // Given — Claude Code appends session metadata (custom-title, last-prompt,
    // file-history-snapshot, …) without a timestamp; they are not events.
    let Env { _tmp, claude_root } = env();
    let session_id = "session-metadata";
    let mut timestamp_less_prompt = user_entry(session_id, "u2", "unused", s("a prompt with no timestamp"));
    timestamp_less_prompt.remove("timestamp"); // `timestamp: undefined` is dropped by JSON.stringify
    write_transcript(
        &claude_root,
        session_id,
        &[
            assistant_entry(
                session_id,
                "a1",
                "2026-06-30T09:00:00.000Z",
                vec![bash("toolu_spec", &marker_command("specify"))],
            ),
            obj(vec![
                ("type", s("custom-title")),
                ("sessionId", s(session_id)),
                ("customTitle", s("Specify multiquote limit")),
            ]),
            obj(vec![
                ("type", s("last-prompt")),
                ("sessionId", s(session_id)),
                ("lastPrompt", s("/specify multiquote")),
            ]),
            obj(vec![
                ("type", s("file-history-snapshot")),
                ("messageId", s("m1")),
                ("snapshot", Value::from(Object::new())),
                ("isSnapshotUpdate", Value::from(false)),
            ]),
            timestamp_less_prompt,
        ],
    );

    // When
    let ExtractResult { summary, .. } = extract(&claude_root);

    // Then
    assert_eq!(summary.conversations, 1);
    assert_eq!(reasons(&summary.skipped_rows), vec!["missing timestamp"]);
}

#[test]
fn implement_and_review_subagents_with_their_own_markers_bind_each_subagent_transcript_to_its_label() {
    // Given — an autonomous-workflow run: both subagents share the parent sessionId
    // and differ only by their agentId.
    let Env { _tmp, claude_root } = env();
    let session_id = "session-autonomous";
    write_transcript(
        &claude_root,
        session_id,
        &[user_entry(
            session_id,
            "u1",
            "2026-06-30T10:00:00.000Z",
            s(&format!("run the workflow on {SPEC}")),
        )],
    );
    write_subagent_transcript(
        &claude_root,
        session_id,
        "impl",
        &[subagent_entry(
            "impl",
            assistant_entry(
                session_id,
                "a1",
                "2026-06-30T10:00:01.000Z",
                vec![bash("toolu_impl", &marker_command("implement"))],
            ),
        )],
    );
    write_subagent_transcript(
        &claude_root,
        session_id,
        "rev",
        &[subagent_entry(
            "rev",
            assistant_entry(
                session_id,
                "a2",
                "2026-06-30T11:00:01.000Z",
                vec![bash("toolu_rev", &marker_command("review"))],
            ),
        )],
    );

    // When
    let ExtractResult { bundle, summary } = extract(&claude_root);

    // Then
    assert_eq!(
        bundle
            .header
            .get("conversations_per_label")
            .map(Value::to_json)
            .as_deref(),
        Some(r#"{"implement":1,"review":1}"#)
    );
    assert_eq!(
        strings(bundle.header.get("conversation_ids")),
        vec![format!("{session_id}/agent-impl"), format!("{session_id}/agent-rev")]
    );
    assert!(!reasons(&summary.skipped_rows).contains(&"same session has markers for multiple labels"));
}

#[test]
fn single_label_session_with_a_marker_less_subagent_binds_the_subagent_under_the_plain_session_id() {
    // Given — a manual implement session delegating a refactoring pass to a subagent.
    let Env { _tmp, claude_root } = env();
    let session_id = "session-manual-implement";
    write_transcript(
        &claude_root,
        session_id,
        &[assistant_entry(
            session_id,
            "a1",
            "2026-06-30T10:00:00.000Z",
            vec![bash("toolu_impl", &marker_command("implement"))],
        )],
    );
    write_subagent_transcript(
        &claude_root,
        session_id,
        "refactor",
        &[subagent_entry(
            "refactor",
            assistant_entry(
                session_id,
                "a2",
                "2026-06-30T10:30:00.000Z",
                vec![text_block("Refactoring pass done.")],
            ),
        )],
    );

    // When
    let ExtractResult { bundle, .. } = extract(&claude_root);

    // Then
    assert_eq!(strings(bundle.header.get("conversation_ids")), vec![session_id]);
    assert!(contains_event(
        &bundle.events,
        &[
            ("conversation_id", session_id),
            ("label", "implement"),
            ("content", "Refactoring pass done."),
        ],
        &[],
    ));
}

#[test]
fn missing_transcript_root_reports_a_skipped_row_naming_the_directory() {
    // Given
    let reader = ClaudeCodeTranscriptReader::new(Some("/nonexistent/claude-root".to_string()));

    // When
    let read = reader.read_capture("any").expect("read capture");

    // Then
    assert_eq!(read.label_by_cid.len(), 0);
    assert_eq!(
        read.skipped,
        vec![SkippedRow::new(
            "claude-root",
            "transcript root not found",
            "/nonexistent/claude-root"
        )]
    );
}

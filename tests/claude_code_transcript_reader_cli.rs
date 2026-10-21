//! Port of the CLI case of `claudeCodeTranscriptReader.test.ts`.

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;

use fixtures::TempDir;
use js::{Object, Value};

const SPEC: &str = "2026-06-30-multiquote-limit-5";

fn base_entry(kind: &str, session_id: &str, uuid: &str, timestamp: &str, message: Object) -> Object {
    let mut entry = Object::new();
    entry.insert("type", kind);
    entry.insert("uuid", uuid);
    entry.insert("parentUuid", Value::Null);
    entry.insert("sessionId", session_id);
    entry.insert("cwd", "/Users/dev/acme/app");
    entry.insert("gitBranch", "main");
    entry.insert("version", "2.1.202");
    entry.insert("message", message);
    entry.insert("timestamp", timestamp);
    entry
}

fn user_entry(session_id: &str, uuid: &str, timestamp: &str, content: &str) -> Object {
    let mut message = Object::new();
    message.insert("role", "user");
    message.insert("content", content);
    base_entry("user", session_id, uuid, timestamp, message)
}

fn assistant_bash_entry(session_id: &str, uuid: &str, timestamp: &str, tool_use_id: &str, command: &str) -> Object {
    let mut input = Object::new();
    input.insert("command", command);
    let mut tool_use = Object::new();
    tool_use.insert("type", "tool_use");
    tool_use.insert("id", tool_use_id);
    tool_use.insert("name", "Bash");
    tool_use.insert("input", input);
    let mut usage = Object::new();
    usage.insert("input_tokens", 42usize);
    usage.insert("output_tokens", 7usize);
    let mut message = Object::new();
    message.insert("id", format!("msg_{uuid}"));
    message.insert("type", "message");
    message.insert("role", "assistant");
    message.insert("model", "claude-sonnet-4-5-20250929");
    message.insert("content", vec![Value::from(tool_use)]);
    message.insert("usage", usage);
    base_entry("assistant", session_id, uuid, timestamp, message)
}

fn write_transcript(root: &str, session_id: &str, entries: &[Object]) {
    let dir = std::path::Path::new(root).join("-Users-dev-acme-app");
    std::fs::create_dir_all(&dir).expect("mkdir");
    let body: String = entries.iter().map(|e| format!("{}\n", e.to_json())).collect();
    std::fs::write(dir.join(format!("{session_id}.jsonl")), body).expect("write transcript");
}

#[test]
fn cli_with_claude_code_source_writes_the_same_jsonl_bundle_format_with_source_claude_code() {
    // Given
    let tmp = TempDir::new("wa-claude-");
    let claude_root = tmp.join(".claude/projects");
    let out_dir = tmp.join("out");
    write_transcript(
        &claude_root,
        "session-specify",
        &[
            user_entry(
                "session-specify",
                "u1",
                "2026-06-30T09:00:00.000Z",
                &format!("specify {SPEC}"),
            ),
            assistant_bash_entry(
                "session-specify",
                "a1",
                "2026-06-30T09:00:01.000Z",
                "toolu_spec",
                &format!(": CAPTURE_MARKER v=1 id={SPEC} label=specify"),
            ),
        ],
    );

    // When
    let result = common::run_cli(
        &[
            "--source",
            "claude-code",
            "--claude-root",
            &claude_root,
            "--id",
            SPEC,
            "--out",
            &out_dir,
        ],
        &tmp.path(),
        &[("HOME", &tmp.path())],
    );

    // Then
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    let raw = std::fs::read_to_string(format!("{out_dir}/{SPEC}.jsonl")).expect("read bundle");
    let mut lines = raw.trim_end().split('\n');
    let header = js::json::parse(lines.next().expect("header line")).expect("header json");
    let header = header.as_object().expect("header object");
    assert!(result.stdout.contains("wrote"));
    assert_eq!(header.get_str("type"), Some("bundle_header"));
    assert_eq!(header.get_str("capture_id"), Some(SPEC));
    assert_eq!(header.get_str("source"), Some("claude-code"));
    assert_eq!(
        header.get("labels").map(Value::to_json).as_deref(),
        Some(r#"["specify"]"#)
    );
    assert!(lines.any(|line| {
        js::json::parse(line)
            .expect("event json")
            .as_object()
            .and_then(|e| e.get_str("kind"))
            == Some("tool_call")
    }));
}

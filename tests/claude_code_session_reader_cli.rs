//! Port of the CLI case of `claudeCodeSessionReader.test.ts`.

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;

use fixtures::TempDir;
use js::{Object, Value};

const PROJECT: &str = "-Users-dev-acme-app";

/// One user record whose string content maps to exactly one `query` event.
fn user_record(session_id: &str, index: usize) -> Object {
    let mut record = Object::new();
    record.insert("type", "user");
    record.insert("uuid", format!("{session_id}-u{index}"));
    record.insert("parentUuid", Value::Null);
    record.insert("sessionId", session_id);
    record.insert("cwd", "/Users/dev/acme/app");
    let mut message = Object::new();
    message.insert("role", "user");
    message.insert("content", format!("message {index} of {session_id}"));
    record.insert("message", message);
    record.insert(
        "timestamp",
        format!("2026-10-15T09:{:02}:{:02}.000Z", index / 60, index % 60),
    );
    record
}

fn records(session_id: &str, count: usize) -> Vec<Object> {
    (1..=count).map(|i| user_record(session_id, i)).collect()
}

fn write_jsonl(file_path: &str, entries: &[Object], mtime: &str) {
    let path = std::path::Path::new(file_path);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let body: String = entries.iter().map(|e| format!("{}\n", e.to_json())).collect();
    std::fs::write(path, body).expect("write transcript");
    let ms = js::date::parse(mtime);
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(at))
        .expect("set mtime");
}

/// Example 2: `a1b2c3.jsonl` (42 records) with `a1b2c3/subagents/agent-77.jsonl`
/// (18 records), and `d4e5f6.jsonl` (9 records).
fn seed_example2(claude_root: &str) {
    let project = format!("{claude_root}/{PROJECT}");
    write_jsonl(
        &format!("{project}/a1b2c3.jsonl"),
        &records("a1b2c3", 42),
        "2026-10-15T10:00:00.000Z",
    );
    let subagent: Vec<Object> = records("a1b2c3", 18)
        .into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            r.insert("agentId", "77");
            r.insert("isSidechain", true);
            r.insert("uuid", format!("agent-77-u{}", i + 1));
            r
        })
        .collect();
    write_jsonl(
        &format!("{project}/a1b2c3/subagents/agent-77.jsonl"),
        &subagent,
        "2026-10-16T08:30:00.000Z",
    );
    write_jsonl(
        &format!("{project}/d4e5f6.jsonl"),
        &records("d4e5f6", 9),
        "2026-10-17T12:00:00.000Z",
    );
}

#[test]
fn cli_rerun_of_sessions_mode_exits_0_twice_and_reports_both_sessions_unchanged() {
    // Given
    let tmp = TempDir::new("wa-cc-sessions-");
    let claude_root = tmp.join(".claude/projects");
    let out_dir = tmp.join("out");
    seed_example2(&claude_root);
    let home = tmp.path();
    let run = || {
        common::run_cli(
            &[
                "--sessions",
                "--source",
                "claude-code",
                "--since",
                "2026-10-01",
                "--claude-root",
                &claude_root,
                "--out",
                &out_dir,
            ],
            &tmp.path(),
            &[("HOME", &home)],
        )
    };
    let first = run();

    // When
    let second = run();

    // Then
    assert_eq!(first.exit_code, 0, "stderr: {}", first.stderr);
    assert!(std::path::Path::new(&format!("{out_dir}/sessions/a1b2c3.jsonl")).exists());
    assert!(std::path::Path::new(&format!("{out_dir}/sessions/d4e5f6.jsonl")).exists());
    assert_eq!(second.exit_code, 0, "stderr: {}", second.stderr);
    assert!(format!("{}{}", second.stdout, second.stderr).contains("2 sessions: 0 written, 2 unchanged, 0 untouched"));
}

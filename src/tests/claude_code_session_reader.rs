//! Port of `claudeCodeSessionReader.test.ts` — readSessions, one session per
//! sessionId with subagents joined (in-process cases; the CLI case lives in
//! `tests/claude_code_session_reader_cli.rs`).

use super::fixtures::TempDir;
use crate::adapters::claude_code_transcript_reader::ClaudeCodeTranscriptReader;
use crate::domain::models::{BundleSource, SessionHeader, SessionInfo};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{ReadSessionsOptions, SessionReader};
use crate::js::{Object, Value, date, json};
use crate::usecases::extract_sessions::{ExtractSessionsOptions, extract_sessions};

const PROJECT: &str = "-Users-dev-acme-app";

/// One user record whose string content maps to exactly one `query` event.
fn user_record(session_id: &str, index: usize, extra: &[(&str, &str)]) -> Object {
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
    for (key, value) in extra {
        record.insert(*key, *value);
    }
    record
}

fn records(session_id: &str, count: usize, extra: &[(&str, &str)]) -> Vec<Object> {
    (1..=count).map(|i| user_record(session_id, i, extra)).collect()
}

fn write_jsonl(file_path: &str, entries: &[Object], mtime: &str) {
    let path = std::path::Path::new(file_path);
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    let body: String = entries.iter().map(|e| format!("{}\n", e.to_json())).collect();
    std::fs::write(path, body).expect("write transcript");
    let ms = date::parse(mtime);
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(ms as u64);
    std::fs::File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(at))
        .expect("set mtime");
}

fn project_file(claude_root: &str, rel: &str) -> String {
    format!("{claude_root}/{PROJECT}/{rel}")
}

/// Example 2: `a1b2c3.jsonl` (42 records) with `a1b2c3/subagents/agent-77.jsonl`
/// (18 records), and `d4e5f6.jsonl` (9 records).
fn seed_example2(claude_root: &str) {
    write_jsonl(
        &project_file(claude_root, "a1b2c3.jsonl"),
        &records("a1b2c3", 42, &[]),
        "2026-10-15T10:00:00.000Z",
    );
    let subagent: Vec<Object> = records("a1b2c3", 18, &[("agentId", "77")])
        .into_iter()
        .enumerate()
        .map(|(i, mut r)| {
            r.insert("isSidechain", true);
            r.insert("uuid", format!("agent-77-u{}", i + 1));
            r
        })
        .collect();
    write_jsonl(
        &project_file(claude_root, "a1b2c3/subagents/agent-77.jsonl"),
        &subagent,
        "2026-10-16T08:30:00.000Z",
    );
    write_jsonl(
        &project_file(claude_root, "d4e5f6.jsonl"),
        &records("d4e5f6", 9, &[]),
        "2026-10-17T12:00:00.000Z",
    );
}

struct Env {
    _tmp: TempDir,
    claude_root: String,
    out_dir: String,
}

fn env() -> Env {
    let tmp = TempDir::new("wa-cc-sessions-");
    let claude_root = tmp.join(".claude/projects");
    let out_dir = tmp.join("out");
    Env {
        _tmp: tmp,
        claude_root,
        out_dir,
    }
}

fn info(conversation_id: &str, last_modified: &str) -> SessionInfo {
    SessionInfo {
        conversation_id: conversation_id.to_string(),
        last_modified: Some(last_modified.to_string()),
    }
}

fn session_options(out_dir: &str) -> ExtractSessionsOptions {
    ExtractSessionsOptions {
        out_dir: out_dir.to_string(),
        source: BundleSource::ClaudeCode,
        since: Some("2026-10-01".to_string()),
        conversation: None,
        no_merge: false,
    }
}

#[test]
fn subagent_parent_and_plain_session_enumerate_one_session_per_session_id_with_latest_mtime() {
    // Given
    let Env { _tmp, claude_root, .. } = env();
    seed_example2(&claude_root);

    // When
    let read = ClaudeCodeTranscriptReader::new(Some(claude_root))
        .read_sessions(&ReadSessionsOptions::default())
        .expect("read sessions");

    // Then
    assert_eq!(read.source, BundleSource::ClaudeCode);
    assert_eq!(
        read.sessions,
        vec![
            info("a1b2c3", "2026-10-16T08:30:00.000Z"), // subagent file is the latest
            info("d4e5f6", "2026-10-17T12:00:00.000Z"),
        ]
    );
    let a = read.reads.get(&"a1b2c3".to_string()).expect("a1b2c3 gathered");
    assert_eq!(a.drafts.len(), 60);
    assert_eq!(a.last_modified.as_deref(), Some("2026-10-16T08:30:00.000Z"));
    assert_eq!(
        read.reads
            .get(&"d4e5f6".to_string())
            .expect("d4e5f6 gathered")
            .drafts
            .len(),
        9
    );
}

#[test]
fn example2_rerun_with_no_transcript_change_writes_both_files_then_reports_both_unchanged() {
    // Given
    let Env {
        _tmp,
        claude_root,
        out_dir,
    } = env();
    seed_example2(&claude_root);
    let reader = ClaudeCodeTranscriptReader::new(Some(claude_root));
    let first = extract_sessions(&reader, session_options(&out_dir)).expect("first run");
    let a_path = format!("{out_dir}/sessions/a1b2c3.jsonl");
    let first_bytes = std::fs::read_to_string(&a_path).expect("read a1b2c3");

    // When
    let second = extract_sessions(&reader, session_options(&out_dir)).expect("second run");

    // Then
    let written: Vec<(&str, usize)> = first
        .written
        .iter()
        .map(|w| (crate::js::path::basename(&w.path), w.events))
        .collect();
    assert_eq!(written, vec![("a1b2c3.jsonl", 60), ("d4e5f6.jsonl", 9)]);
    let lines: Vec<Object> = first_bytes
        .trim()
        .split('\n')
        .map(|line| json::parse(line).expect("json").as_object().expect("object").clone())
        .collect();
    assert_eq!(lines[0].get_str("type"), Some("session_header"));
    assert_eq!(lines[0].get_str("conversation_id"), Some("a1b2c3"));
    assert_eq!(lines[0].get_str("source"), Some("claude-code"));
    assert_eq!(lines[0].get_str("last_modified"), Some("2026-10-16T08:30:00.000Z"));
    let events = &lines[1..];
    assert!(events.iter().all(|e| e.get_str("conversation_id") == Some("a1b2c3")));
    let subagent_source = format!("{PROJECT}/a1b2c3/subagents/agent-77.jsonl");
    let from_subagent = events
        .iter()
        .filter(|e| {
            e.get("meta")
                .and_then(Value::as_object)
                .and_then(|m| m.get_str("source_file"))
                == Some(subagent_source.as_str())
        })
        .count();
    assert_eq!(from_subagent, 18);

    assert_eq!(second.enumerated, 2);
    assert_eq!(second.written, vec![]);
    assert_eq!(second.unchanged, vec!["a1b2c3", "d4e5f6"]);
    assert_eq!(std::fs::read_to_string(&a_path).expect("reread a1b2c3"), first_bytes);
}

#[test]
fn since_cutoff_enumerates_only_sessions_active_at_or_after_it() {
    // Given
    let Env { _tmp, claude_root, .. } = env();
    write_jsonl(
        &project_file(&claude_root, "old-0412.jsonl"),
        &records("old-0412", 2, &[]),
        "2026-09-28T16:00:00.000Z",
    );
    write_jsonl(
        &project_file(&claude_root, "new-0419.jsonl"),
        &records("new-0419", 3, &[]),
        "2026-10-17T09:00:00.000Z",
    );
    write_jsonl(
        &project_file(&claude_root, "edge.jsonl"),
        &records("edge", 1, &[]),
        "2026-10-01T00:00:00.000Z",
    );
    let reader = ClaudeCodeTranscriptReader::new(Some(claude_root));

    // When
    let read = reader
        .read_sessions(&ReadSessionsOptions {
            since: Some("2026-10-01".to_string()),
            ..ReadSessionsOptions::default()
        })
        .expect("read sessions");

    // Then
    let enumerated: Vec<&str> = read.sessions.iter().map(|s| s.conversation_id.as_str()).collect();
    assert_eq!(enumerated, vec!["edge", "new-0419"]); // at/after, inclusive
    let gathered: Vec<&str> = read.reads.keys().map(String::as_str).collect();
    assert_eq!(gathered, vec!["edge", "new-0419"]);
}

#[test]
fn conversation_filter_enumerates_and_gathers_exactly_that_session_even_when_unchanged() {
    // Given
    let Env { _tmp, claude_root, .. } = env();
    write_jsonl(
        &project_file(&claude_root, "old-0412.jsonl"),
        &records("old-0412", 2, &[]),
        "2026-09-28T16:00:00.000Z",
    );
    write_jsonl(
        &project_file(&claude_root, "new-0419.jsonl"),
        &records("new-0419", 3, &[]),
        "2026-10-17T09:00:00.000Z",
    );
    let mut stored_by_cid = OrderedMap::new();
    stored_by_cid.insert(
        "old-0412".to_string(),
        SessionHeader::fresh(
            "old-0412",
            BundleSource::ClaudeCode,
            Some("2026-09-28T16:00:00.000Z"),
            "2026-10-01T00:00:00.000Z",
        ),
    );

    // When
    let read = ClaudeCodeTranscriptReader::new(Some(claude_root))
        .read_sessions(&ReadSessionsOptions {
            conversation: Some("old-0412".to_string()),
            stored_by_cid,
            ..ReadSessionsOptions::default()
        })
        .expect("read sessions");

    // Then
    let enumerated: Vec<&str> = read.sessions.iter().map(|s| s.conversation_id.as_str()).collect();
    assert_eq!(enumerated, vec!["old-0412"]);
    let gathered: Vec<&str> = read.reads.keys().map(String::as_str).collect();
    assert_eq!(gathered, vec!["old-0412"]);
    assert_eq!(
        read.reads.get(&"old-0412".to_string()).expect("gathered").drafts.len(),
        2
    );
}

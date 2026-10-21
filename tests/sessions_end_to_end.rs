//! Port of `sessionsEndToEnd.test.ts`: the real CLI in `--sessions` mode.

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;

use std::fs;
use std::path::Path;

use common::{CliResult, run_cli};
use fixtures::TempDir;
use fixtures::fixture_db::{
    BlockSeed, QuerySeed, create_fixture, seed_block, seed_conversation, seed_query, seed_task,
};
use fixtures::protobuf::encode_string;
use js::Object;

const ROOT: &str = env!("CARGO_MANIFEST_DIR");

/// c1: conversation + query + block + task; c2: conversation + query; c3: tasks only.
fn seed_three_conversations(db: &rusqlite::Connection) {
    seed_conversation(db, "c1", Some("2026-06-30 10:00:00.000000"));
    seed_query(
        db,
        QuerySeed {
            conversation_id: "c1",
            start_ts: "2026-06-30 10:10:00.000000",
            text: "fix the login",
            ..QuerySeed::default()
        },
    );
    seed_block(
        db,
        BlockSeed {
            conversation_id: "c1",
            start_ts: "2026-06-30 10:20:00.000000",
            command: Some("npm test"),
            ..BlockSeed::default()
        },
    );
    seed_task(
        db,
        "c1",
        &encode_string(1, "task message"),
        None,
        Some("2026-06-30 10:30:00.000000"),
    );
    seed_conversation(db, "c2", Some("2026-07-01 09:00:00.000000"));
    seed_query(
        db,
        QuerySeed {
            conversation_id: "c2",
            start_ts: "2026-07-01 09:10:00.000000",
            text: "second session",
            ..QuerySeed::default()
        },
    );
    seed_task(
        db,
        "c3",
        &encode_string(1, "task only"),
        None,
        Some("2026-07-02 08:00:00.000000"),
    );
}

/// The per-test setup (`beforeEach`): a seeded fixture DB and an output dir.
struct Setup {
    tmp: TempDir,
    db_path: String,
    out_dir: String,
}

impl Setup {
    fn new() -> Self {
        let tmp = TempDir::new("wa-923-");
        let db_path = tmp.join("warp.sqlite");
        let out_dir = tmp.join("out");
        let db = create_fixture(&db_path, false);
        seed_three_conversations(&db);
        drop(db);
        Self { tmp, db_path, out_dir }
    }

    fn run_cli(&self, args: &[&str]) -> CliResult {
        run_cli(args, ROOT, &[("HOME", &self.tmp.path())])
    }

    fn session_args(&self) -> Vec<&str> {
        vec![
            "--sessions",
            "--source",
            "warp",
            "--db-path",
            &self.db_path,
            "--out",
            &self.out_dir,
        ]
    }

    fn sessions_dir(&self) -> String {
        format!("{}/sessions", self.out_dir)
    }
}

fn sorted_dir(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

// --- §9.23 end-to-end --sessions (CLI -> snapshot adapter -> use-case -> sink) ---

#[test]
fn sessions_run_twice_writes_files_then_reports_everything_unchanged() {
    // Given
    let s = Setup::new();

    // When — first run
    let first = s.run_cli(&s.session_args());

    // Then — three files under out/sessions/ plus the tally report
    assert_eq!(first.exit_code, 0, "{}", first.stderr);
    let sessions_dir = s.sessions_dir();
    assert_eq!(sorted_dir(&sessions_dir), vec!["c1.jsonl", "c2.jsonl", "c3.jsonl"]);
    assert!(
        first.stdout.contains(&format!("wrote {sessions_dir}/c1.jsonl")),
        "{}",
        first.stdout
    );
    assert!(first.stdout.contains(&format!("wrote {sessions_dir}/c2.jsonl")));
    assert!(first.stdout.contains(&format!("wrote {sessions_dir}/c3.jsonl")));
    assert!(first.stdout.contains("3 sessions: 3 written, 0 unchanged, 0 untouched"));

    // strict NDJSON: trailing newline, every physical line parses
    let raw = fs::read_to_string(format!("{sessions_dir}/c1.jsonl")).expect("read");
    assert!(raw.ends_with('\n'));
    let mut lines: Vec<&str> = raw.split('\n').collect();
    lines.pop();
    let parsed: Vec<Object> = lines
        .iter()
        .map(|l| {
            js::json::parse(l)
                .expect("valid JSON")
                .as_object()
                .expect("object")
                .clone()
        })
        .collect();
    for line in &lines {
        assert!(!line.contains('\n'));
    }

    // line 1 is the session_header; events carry no capture_id/label
    assert_eq!(parsed[0].get_str("type"), Some("session_header"));
    assert_eq!(parsed[0].get_str("conversation_id"), Some("c1"));
    let events = &parsed[1..];
    let kinds: Vec<&str> = events.iter().map(|e| e.get_str("kind").expect("kind")).collect();
    assert_eq!(kinds, vec!["query", "command", "agent_message"]);
    for e in events {
        assert!(!e.contains_key("capture_id"));
        assert!(!e.contains_key("label"));
    }
    let ts: Vec<&str> = events.iter().map(|e| e.get_str("ts").expect("ts")).collect();
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(sorted, ts);

    // When — second run, nothing changed
    let second = s.run_cli(&s.session_args());

    // Then — everything skipped as unchanged, exit 0, no wrote lines
    assert_eq!(second.exit_code, 0);
    assert_eq!(second.stdout, "3 sessions: 0 written, 3 unchanged, 0 untouched\n");
}

#[test]
fn since_cutoff_between_conversations_writes_only_sessions_at_or_after_it() {
    // Given
    let s = Setup::new();

    // When — cutoff after c1's last activity, before c2/c3
    let mut args = s.session_args();
    args.extend(["--since", "2026-07-01"]);
    let result = s.run_cli(&args);

    // Then
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert_eq!(sorted_dir(&s.sessions_dir()), vec!["c2.jsonl", "c3.jsonl"]);
    assert!(
        result
            .stdout
            .contains("2 sessions: 2 written, 0 unchanged, 0 untouched")
    );
}

#[test]
fn id_combined_with_sessions_exits_2_before_any_io() {
    // Given
    let s = Setup::new();

    // When
    let result = s.run_cli(&[
        "--sessions",
        "--id",
        "some-capture",
        "--source",
        "warp",
        "--db-path",
        &s.db_path,
        "--out",
        &s.out_dir,
    ]);

    // Then
    assert_eq!(result.exit_code, 2);
    assert!(
        result.stderr.contains("error: --id cannot be combined with --sessions"),
        "{}",
        result.stderr
    );
    assert!(!Path::new(&s.sessions_dir()).exists());
    assert!(!Path::new(&s.out_dir).exists());
}

#[test]
fn since_or_conversation_without_sessions_exits_2_before_any_io() {
    // Given
    let s = Setup::new();

    // When — a marker-mode invocation carrying a session-only flag
    let result = s.run_cli(&[
        "--id",
        "some-capture",
        "--source",
        "warp",
        "--db-path",
        &s.db_path,
        "--out",
        &s.out_dir,
        "--since",
        "2026-07-01",
    ]);

    // Then
    assert_eq!(result.exit_code, 2);
    assert!(
        result
            .stderr
            .contains("error: --since and --conversation are only valid together with --sessions"),
        "{}",
        result.stderr
    );
    assert!(!Path::new(&s.out_dir).exists());
}

#[test]
fn since_cutoff_no_conversation_reaches_exits_1_and_writes_nothing() {
    // Given
    let s = Setup::new();

    // When
    let mut args = s.session_args();
    args.extend(["--since", "2100-01-01"]);
    let result = s.run_cli(&args);

    // Then
    assert_eq!(result.exit_code, 1);
    assert!(result.stderr.contains("no sessions enumerated"), "{}", result.stderr);
    assert!(
        result.stderr.contains("--since 2100-01-01 too recent"),
        "{}",
        result.stderr
    );
    assert!(!Path::new(&s.sessions_dir()).exists());
    assert!(!Path::new(&s.out_dir).exists());
}

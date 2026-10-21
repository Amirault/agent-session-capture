//! Port of `endToEnd.test.ts`: the real CLI against fixture Warp DBs.

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;

use std::collections::HashSet;
use std::fs;

use common::{CliResult, run_cli};
use fixtures::TempDir;
use fixtures::fixture_db::{
    BlockSeed, MarkerSeed, QuerySeed, create_fixture, seed_block, seed_marker, seed_query, seed_task,
};
use fixtures::protobuf::encode_string;
use js::{Object, Value};

const SPEC: &str = "add-feature-x";
const ROOT: &str = env!("CARGO_MANIFEST_DIR");

fn marker(db: &rusqlite::Connection, label: &str, conversation_id: &str, start_ts: &str) {
    seed_marker(
        db,
        MarkerSeed {
            capture_id: SPEC,
            label,
            conversation_id,
            start_ts,
        },
    );
}

fn query(db: &rusqlite::Connection, conversation_id: &str, start_ts: &str, text: &str) {
    seed_query(
        db,
        QuerySeed {
            conversation_id,
            start_ts,
            text,
            ..QuerySeed::default()
        },
    );
}

/// Run the CLI from the crate root with HOME pointed at the test's temp dir.
fn cli(args: &[&str], tmp: &TempDir) -> CliResult {
    run_cli(args, ROOT, &[("HOME", &tmp.path())])
}

fn parse_lines(raw: &str) -> (Vec<&str>, Vec<Object>) {
    let mut lines: Vec<&str> = raw.split('\n').collect();
    lines.pop(); // drop trailing "" from final newline
    let parsed = lines
        .iter()
        .map(|l| {
            js::json::parse(l)
                .expect("valid JSON")
                .as_object()
                .expect("object")
                .clone()
        })
        .collect();
    (lines, parsed)
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(*v)).collect())
}

// --- CLI source selection ---

#[test]
fn spec_extraction_without_source_fails_before_reading_or_writing() {
    // Given
    let tmp = TempDir::new("capture-source-required-");
    let out_dir = tmp.join("out");
    let out_path = format!("{out_dir}/{SPEC}.jsonl");
    let sentinel = "existing bundle must not be read or overwritten\n";
    fs::create_dir_all(&out_dir).expect("mkdir");
    fs::write(&out_path, sentinel).expect("write");

    // When
    let result = cli(&["--id", SPEC, "--out", &out_dir], &tmp);

    // Then
    assert_eq!(result.exit_code, 2);
    assert!(
        result
            .stderr
            .contains(r#"error: --source is required for extraction ("warp", "claude-code", "hermes", or "langfuse")"#),
        "{}",
        result.stderr
    );
    assert_eq!(fs::read_to_string(&out_path).expect("read"), sentinel);
}

// --- §9.10 end-to-end (CLI -> snapshot adapter -> use-case -> sink) ---

#[test]
fn spec_across_three_labels_with_subagent_and_rerun_gate_writes_a_complete_ordered_bundle() {
    // Given — a file-backed fixture DB (the real adapter will VACUUM INTO it)
    let tmp = TempDir::new("wa-910-");
    let db_path = tmp.join("warp.sqlite");
    let out_dir = tmp.join("out");
    let db = create_fixture(&db_path, false);
    // specify (c1)
    marker(&db, "specify", "c1", "2026-06-30 09:00:00.000000");
    query(&db, "c1", "2026-06-30 09:10:00.000000", "spec it");
    // implement (c2) with a subagent block + subagent task sharing the parent conversation_id
    marker(&db, "implement", "c2", "2026-06-30 10:00:00.000000");
    query(&db, "c2", "2026-06-30 10:10:00.000000", "implement it");
    seed_block(
        &db,
        BlockSeed {
            conversation_id: "c2",
            start_ts: "2026-06-30 10:20:00.000000",
            command: Some("npm test"),
            subagent_task_id: Some("sub-1"),
            ..BlockSeed::default()
        },
    );
    seed_task(
        &db,
        "c2",
        &encode_string(1, "subagent message"),
        None,
        Some("2026-06-30 10:30:00.000000"),
    );
    // review (c3) + a re-run gate (c4)
    marker(&db, "review", "c3", "2026-06-30 11:00:00.000000");
    query(&db, "c3", "2026-06-30 11:10:00.000000", "gate run 1");
    marker(&db, "review", "c4", "2026-06-30 12:00:00.000000");
    query(&db, "c4", "2026-06-30 12:10:00.000000", "gate run 2");
    drop(db);

    // When — run the actual CLI against the fixture DB
    let result = cli(
        &[
            "--id",
            SPEC,
            "--source",
            "warp",
            "--db-path",
            &db_path,
            "--out",
            &out_dir,
        ],
        &tmp,
    );
    assert_eq!(result.exit_code, 0, "{}", result.stderr);

    // Then — the bundle file was written
    let out_path = format!("{out_dir}/{SPEC}.jsonl");
    assert!(std::path::Path::new(&out_path).exists());
    assert!(result.stdout.contains("wrote"));

    // JSONL validity: every line parses, no raw newline leaks into a value
    let raw = fs::read_to_string(&out_path).expect("read");
    let (lines, parsed) = parse_lines(&raw);
    for line in &lines {
        assert!(!line.contains('\n'));
    }

    // line 1 is the header — complete, all 3 labels, gate has 2 conversations
    let header = &parsed[0];
    assert_eq!(header.get_str("type"), Some("bundle_header"));
    assert_eq!(header.get_str("capture_id"), Some(SPEC));
    assert_eq!(header.get_str("source"), Some("warp"));
    assert_eq!(
        header.get("labels"),
        Some(&strings(&["specify", "implement", "review"]))
    );
    let per_label = header
        .get("conversations_per_label")
        .and_then(Value::as_object)
        .expect("tally");
    assert_eq!(per_label.get("review"), Some(&Value::Number(2.0)));

    let events = &parsed[1..];

    // subagent rows are included: the subagent block carries subagent_task_id,
    // and the subagent task's walked string is present
    assert!(events.iter().any(|e| {
        e.get("meta")
            .and_then(Value::as_object)
            .and_then(|m| m.get_str("subagent_task_id"))
            == Some("sub-1")
    }));
    assert!(events.iter().any(|e| e.get_str("content") == Some("subagent message")));

    // all three labels appear among events
    let labels: HashSet<&str> = events.iter().filter_map(|e| e.get_str("label")).collect();
    assert_eq!(labels, HashSet::from(["specify", "implement", "review"]));

    // time-ordered with a monotonic 1..N sequence
    let ts: Vec<&str> = events.iter().map(|e| e.get_str("ts").expect("ts")).collect();
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(sorted, ts);
    let seq: Vec<f64> = events
        .iter()
        .map(|e| e.get("seq").and_then(Value::as_f64).expect("seq"))
        .collect();
    let expected: Vec<f64> = (1..=events.len()).map(|i| i as f64).collect();
    assert_eq!(seq, expected);
}

// --- §9.18 end-to-end merge + guards ---

/// The per-test setup (`beforeEach`): a temp dir and its output directory.
struct Setup {
    tmp: TempDir,
    out_dir: String,
}

impl Setup {
    fn new() -> Self {
        let tmp = TempDir::new("wa-918-");
        let out_dir = tmp.join("out");
        Self { tmp, out_dir }
    }

    fn fixture_db_with_label(&self, label: &str, cid: &str) -> String {
        let db_path = self.tmp.join(&format!("{label}.sqlite"));
        let db = create_fixture(&db_path, false);
        marker(&db, label, cid, "2026-06-30 10:00:00.000000");
        query(&db, cid, "2026-06-30 10:10:00.000000", &format!("{label} it"));
        db_path
    }

    fn run_cli(&self, db_path: &str, extra_args: &[&str]) -> CliResult {
        let mut args = vec![
            "--id",
            SPEC,
            "--source",
            "warp",
            "--db-path",
            db_path,
            "--out",
            &self.out_dir,
        ];
        args.extend_from_slice(extra_args);
        cli(&args, &self.tmp)
    }

    fn out_path(&self) -> String {
        format!("{}/{SPEC}.jsonl", self.out_dir)
    }

    fn read_bundle(&self) -> (Object, Vec<Object>) {
        let raw = fs::read_to_string(self.out_path()).expect("read bundle");
        let (_, parsed) = parse_lines(&raw);
        (parsed[0].clone(), parsed[1..].to_vec())
    }

    fn write_foreign_header(&self, header: &Object) -> String {
        fs::create_dir_all(&self.out_dir).expect("mkdir");
        let line = header.to_json();
        fs::write(self.out_path(), format!("{line}\n")).expect("write");
        line
    }
}

fn foreign_header(capture_id: &str, labels: &[&str], per_label: [f64; 3], cid: &str, at: &str, source: &str) -> Object {
    let mut tally = Object::new();
    tally.insert("specify", per_label[0]);
    tally.insert("implement", per_label[1]);
    tally.insert("review", per_label[2]);
    let mut header = Object::new();
    header.insert("type", "bundle_header");
    header.insert("capture_id", capture_id);
    header.insert("labels", strings(labels));
    header.insert("conversations_per_label", tally);
    header.insert("conversation_ids", strings(&[cid]));
    header.insert("extracted_at", at);
    header.insert("source", source);
    header
}

#[test]
fn id_escaping_the_output_directory_is_refused_with_exit_2() {
    // Given
    let s = Setup::new();
    let db_path = s.fixture_db_with_label("specify", "c1");

    // When
    let result = s.run_cli(&db_path, &[]).exit_code;
    let escaped = cli(
        &[
            "--id",
            "../esc",
            "--source",
            "warp",
            "--db-path",
            &db_path,
            "--out",
            &s.out_dir,
        ],
        &s.tmp,
    );

    // Then
    assert_eq!(result, 0);
    assert_eq!(escaped.exit_code, 2);
    assert!(escaped.stderr.contains("is not allowed"), "{}", escaped.stderr);
    assert!(!std::path::Path::new(&s.out_dir).join("../esc.jsonl").exists());
}

#[test]
fn two_captures_with_default_merge_keep_both_labels() {
    // Given — first run writes specify
    let s = Setup::new();
    s.run_cli(&s.fixture_db_with_label("specify", "c1"), &[]);
    // second run writes implement (merges with existing specify)
    s.run_cli(&s.fixture_db_with_label("implement", "c2"), &[]);

    // Then — merged bundle has both labels (specify from disk, implement from
    // live), specify not lost, and no duplicate events (dedup by natural key)
    let (header, events) = s.read_bundle();
    let mut labels: Vec<&str> = header
        .get("labels")
        .and_then(Value::as_array)
        .expect("labels")
        .iter()
        .filter_map(Value::as_str)
        .collect();
    labels.sort();
    assert_eq!(labels, vec!["implement", "specify"]);
    let event_labels: HashSet<&str> = events.iter().filter_map(|e| e.get_str("label")).collect();
    assert_eq!(event_labels, HashSet::from(["specify", "implement"]));
    assert!(events.iter().any(|e| e.get_str("label") == Some("specify")));
    let keys: HashSet<String> = events
        .iter()
        .map(|e| {
            format!(
                "{}:{}:{}:{}",
                e.get_str("conversation_id").unwrap_or_default(),
                e.get_str("ts").unwrap_or_default(),
                e.get_str("kind").unwrap_or_default(),
                e.get_str("content").unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(keys.len(), events.len());
}

#[test]
fn two_captures_of_one_task_row_merge_into_one_event_with_the_earlier_ts() {
    // Given
    let s = Setup::new();
    let task = encode_string(1, "same task event");
    let seed = |name: &str, last_modified_at: &str| {
        let db_path = s.tmp.join(name);
        let db = create_fixture(&db_path, false);
        marker(&db, "review", "c1", "2026-06-30 08:00:00.000000");
        seed_task(&db, "c1", &task, Some("stable-task"), Some(last_modified_at));
        db_path
    };
    let first_db_path = seed("task-first.sqlite", "2026-06-30 09:00:00.000000");
    let second_db_path = seed("task-second.sqlite", "2026-06-30 10:00:00.000000");
    s.run_cli(&first_db_path, &[]);

    // When
    s.run_cli(&second_db_path, &[]);

    // Then
    let (_, events) = s.read_bundle();
    let task_events: Vec<&Object> = events
        .iter()
        .filter(|e| e.get_str("content") == Some("same task event"))
        .collect();
    assert_eq!(task_events.len(), 1);
    assert_eq!(task_events[0].get_str("ts"), Some("2026-06-30 09:00:00.000000"));
}

#[test]
fn no_merge_replaces_the_existing_bundle_with_a_warning() {
    // Given — first run writes specify
    let s = Setup::new();
    s.run_cli(&s.fixture_db_with_label("specify", "c1"), &[]);
    // second run with --no-merge writes implement (replaces, no merge)
    let result = s.run_cli(&s.fixture_db_with_label("implement", "c2"), &["--no-merge"]);

    // Then — warning emitted, bundle has implement only (specify gone)
    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.contains("replacing existing bundle"), "{}", result.stderr);
    let (header, _) = s.read_bundle();
    assert_eq!(header.get("labels"), Some(&strings(&["implement"])));
}

#[test]
fn corrupt_existing_bundle_errors_loudly_and_is_not_overwritten() {
    // Given — write a corrupt bundle file
    let s = Setup::new();
    fs::create_dir_all(&s.out_dir).expect("mkdir");
    fs::write(s.out_path(), "not valid json\n").expect("write");

    // When
    let result = s.run_cli(&s.fixture_db_with_label("specify", "c1"), &[]);

    // Then — error, exit non-zero, file unchanged
    assert_ne!(result.exit_code, 0);
    assert!(result.stderr.contains("corrupt"), "{}", result.stderr);
    assert!(result.stderr.contains("refusing to overwrite"), "{}", result.stderr);
    assert_eq!(fs::read_to_string(s.out_path()).expect("read"), "not valid json\n");
}

#[test]
fn stored_bundle_and_failing_fresh_read_degrade_to_stored_only_with_a_warning() {
    // Given — first run writes specify
    let s = Setup::new();
    s.run_cli(&s.fixture_db_with_label("specify", "c1"), &[]);
    // second run with a non-existent DB → fresh read fails → stored-only fallback
    let result = s.run_cli(&s.tmp.join("nonexistent.sqlite"), &[]);

    // Then — does not abort (exit 0), warns about the fresh read, preserves stored
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(result.stderr.contains("fresh read failed"), "{}", result.stderr);
    let (header, _) = s.read_bundle();
    assert_eq!(header.get("labels"), Some(&strings(&["specify"])));
}

#[test]
fn existing_bundle_from_another_source_is_refused_and_not_overwritten() {
    // Given
    let s = Setup::new();
    let foreign = s.write_foreign_header(&foreign_header(
        SPEC,
        &["specify"],
        [1.0, 0.0, 0.0],
        "hermes-c1",
        "2026-07-29T00:00:00.000Z",
        "hermes",
    ));

    // When
    let result = s.run_cli(&s.fixture_db_with_label("specify", "warp-c1"), &[]);

    // Then
    assert_ne!(result.exit_code, 0);
    assert!(result.stderr.contains("bundle source mismatch"), "{}", result.stderr);
    assert!(result.stderr.contains("--no-merge"), "{}", result.stderr);
    assert_eq!(fs::read_to_string(s.out_path()).expect("read"), format!("{foreign}\n"));
}

#[test]
fn existing_bundle_from_another_source_with_no_merge_is_replaced() {
    // Given
    let s = Setup::new();
    let foreign = s.write_foreign_header(&foreign_header(
        SPEC,
        &["review"],
        [0.0, 0.0, 1.0],
        "hermes-c1",
        "2026-07-29T00:00:00.000Z",
        "hermes",
    ));

    // When
    let result = s.run_cli(&s.fixture_db_with_label("specify", "warp-c1"), &["--no-merge"]);

    // Then
    assert_eq!(result.exit_code, 0);
    assert!(result.stderr.contains("replacing existing bundle"), "{}", result.stderr);
    let (header, _) = s.read_bundle();
    assert_eq!(header.get_str("source"), Some("warp"));
    assert_eq!(header.get("labels"), Some(&strings(&["specify"])));
    assert_eq!(header.get("conversation_ids"), Some(&strings(&["warp-c1"])));
    assert_ne!(fs::read_to_string(s.out_path()).expect("read"), format!("{foreign}\n"));
}

#[test]
fn existing_bundle_with_mismatched_capture_id_errors_loudly() {
    // Given — write a valid bundle with a different capture_id in the header
    let s = Setup::new();
    s.write_foreign_header(&foreign_header(
        "different-spec",
        &["specify"],
        [1.0, 0.0, 0.0],
        "c1",
        "2026-07-14T00:00:00.000Z",
        "warp",
    ));

    // When
    let result = s.run_cli(&s.fixture_db_with_label("specify", "c1"), &[]);

    // Then — error, exit non-zero, file unchanged
    assert_ne!(result.exit_code, 0);
    assert!(result.stderr.contains("capture_id mismatch"), "{}", result.stderr);
    assert!(result.stderr.contains("refusing to overwrite"), "{}", result.stderr);
}

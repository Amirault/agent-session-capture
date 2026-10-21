//! Port of `extractSessions.test.ts`.

use std::collections::BTreeMap;
use std::fs;
use std::time::SystemTime;

use rusqlite::{Connection, params};

use super::fixtures::TempDir;
use super::fixtures::fake_source::{FakeSource, fake_source};
use super::fixtures::fixture_db::{
    BlockSeed, QuerySeed, create_fixture, seed_block, seed_conversation, seed_query, seed_task,
};
use super::fixtures::protobuf::encode_string;
use crate::adapters::session_file_name::session_file_name;
use crate::adapters::warp_conversation_reader::WarpConversationReader;
use crate::domain::models::{BundleSource, SessionRunSummary};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value, json, path};
use crate::usecases::extract_sessions::{ExtractSessionsOptions, extract_sessions};

/// c1: conversation + query + block + task; c2: conversation + query; c3: tasks only.
fn seed_three_conversations(db: &Connection) {
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
    db: Connection,
}

fn setup() -> Setup {
    let tmp = TempDir::new("wa-921-");
    let db_path = tmp.join("f.db");
    let out_dir = tmp.join("out");
    let db = create_fixture(&db_path, false);
    seed_three_conversations(&db);
    Setup {
        tmp,
        db_path,
        out_dir,
        db,
    }
}

fn reader(db_path: &str) -> WarpConversationReader<FakeSource> {
    WarpConversationReader::new(fake_source(db_path))
}

fn options(out_dir: &str) -> ExtractSessionsOptions {
    ExtractSessionsOptions {
        out_dir: out_dir.to_string(),
        source: BundleSource::Warp,
        since: None,
        conversation: None,
        no_merge: false,
    }
}

fn run(reader: &WarpConversationReader<FakeSource>, options: ExtractSessionsOptions) -> SessionRunSummary {
    extract_sessions(reader, options).expect("extract sessions")
}

fn sessions_dir_of(out_dir: &str) -> String {
    path::join(&[out_dir, "sessions"])
}

struct SessionFileRead {
    header: Object,
    events: Vec<Object>,
}

fn read_session_file(out_dir: &str, cid: &str) -> SessionFileRead {
    let file = path::join(&[&sessions_dir_of(out_dir), &format!("{}.jsonl", session_file_name(cid))]);
    let raw = fs::read_to_string(file).expect("read session file");
    let mut lines: Vec<&str> = raw.split('\n').collect();
    lines.pop(); // drop trailing "" from final newline
    let parsed: Vec<Object> = lines
        .iter()
        .map(|l| json::parse(l).expect("json").as_object().expect("object").clone())
        .collect();
    SessionFileRead {
        header: parsed[0].clone(),
        events: parsed[1..].to_vec(),
    }
}

/// {fileName -> (content, mtime)} — proves content equality, not just mtime.
fn snapshot_sessions_dir(sessions_dir: &str) -> BTreeMap<String, (String, SystemTime)> {
    let mut snapshot = BTreeMap::new();
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return snapshot;
    };
    for entry in entries {
        let entry = entry.expect("entry");
        let content = fs::read_to_string(entry.path()).expect("read");
        let mtime = entry.metadata().expect("metadata").modified().expect("mtime");
        snapshot.insert(entry.file_name().to_string_lossy().into_owned(), (content, mtime));
    }
    snapshot
}

fn sorted_dir(dir: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn written_ids(summary: &SessionRunSummary) -> Vec<String> {
    summary.written.iter().map(|w| w.conversation_id.clone()).collect()
}

fn sorted(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values
}

fn owned(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| v.to_string()).collect()
}

fn strs<'a>(events: &'a [Object], key: &str) -> Vec<&'a str> {
    events.iter().map(|e| e.get_str(key).expect("string field")).collect()
}

fn seqs(events: &[Object]) -> Vec<f64> {
    events
        .iter()
        .map(|e| e.get("seq").and_then(Value::as_f64).expect("seq"))
        .collect()
}

fn is_iso_timestamp(text: &str) -> bool {
    // ^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$
    let pattern = "dddd-dd-ddTdd:dd:dd.dddZ";
    text.len() == pattern.len()
        && text
            .chars()
            .zip(pattern.chars())
            .all(|(c, p)| if p == 'd' { c.is_ascii_digit() } else { c == p })
}

fn write_header_file(file: &str, header: &[(&str, &str)]) -> String {
    let mut object = Object::new();
    for (key, value) in header {
        object.insert(*key, *value);
    }
    let line = object.to_json();
    fs::write(file, format!("{line}\n")).expect("write");
    line
}

// --- §9.21 extractSessions use case — incremental, decay-safe session export ---

#[test]
fn fresh_out_dir_writes_one_label_free_chronological_file_per_conversation() {
    // Given — fixture seeded in setup; no CAPTURE_MARKER anywhere
    let s = setup();

    // When
    let summary = run(&reader(&s.db_path), options(&s.out_dir));

    // Then — accounting
    assert_eq!(summary.enumerated, 3);
    assert_eq!(sorted(written_ids(&summary)), owned(&["c1", "c2", "c3"]));
    assert!(summary.unchanged.is_empty());
    assert!(summary.untouched.is_empty());
    assert!(summary.warnings.is_empty());

    // one file per conversation, in the sessions/ subdir
    assert_eq!(
        sorted_dir(&sessions_dir_of(&s.out_dir)),
        owned(&["c1.jsonl", "c2.jsonl", "c3.jsonl"])
    );
    assert!(
        summary.written[0]
            .path
            .ends_with(&path::join(&["sessions", "c1.jsonl"]))
    );

    // line 1 is the session_header; events chronological, seq 1..N, no capture_id/label
    let c1 = read_session_file(&s.out_dir, "c1");
    assert_eq!(c1.header.get_str("type"), Some("session_header"));
    assert_eq!(c1.header.get_str("conversation_id"), Some("c1"));
    assert_eq!(c1.header.get_str("source"), Some("warp"));
    assert_eq!(c1.header.get_str("last_modified"), Some("2026-06-30 10:30:00.000000")); // MAX over conversations+tasks
    assert!(is_iso_timestamp(
        c1.header.get_str("extracted_at").expect("extracted_at")
    ));
    assert_eq!(strs(&c1.events, "kind"), vec!["query", "command", "agent_message"]);
    let ts = strs(&c1.events, "ts");
    let mut sorted_ts = ts.clone();
    sorted_ts.sort();
    assert_eq!(sorted_ts, ts);
    let expected_seq: Vec<f64> = (1..=c1.events.len()).map(|i| i as f64).collect();
    assert_eq!(seqs(&c1.events), expected_seq);
    for e in &c1.events {
        assert!(!e.contains_key("capture_id"));
        assert!(!e.contains_key("label"));
        assert_eq!(e.get_str("conversation_id"), Some("c1"));
    }

    // the tasks-only conversation is exported too
    let c3 = read_session_file(&s.out_dir, "c3");
    assert_eq!(c3.header.get_str("last_modified"), Some("2026-07-02 08:00:00.000000"));
    assert_eq!(strs(&c3.events, "kind"), vec!["agent_message"]);
}

#[test]
fn rerun_without_store_changes_rewrites_nothing_and_reports_all_unchanged() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    let before = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));

    // When
    let summary = run(&reader, options(&s.out_dir));

    // Then — no gather, no write, byte-identical files
    assert!(summary.written.is_empty());
    assert_eq!(sorted(summary.unchanged.clone()), owned(&["c1", "c2", "c3"]));
    assert!(summary.untouched.is_empty());
    assert_eq!(snapshot_sessions_dir(&sessions_dir_of(&s.out_dir)), before);
}

#[test]
fn bumped_stamp_rewrites_only_that_session_file() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    let before = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));

    // When — c1's task row is touched (mutable last_modified_at)
    s.db.execute(
        "UPDATE agent_tasks SET last_modified_at = ? WHERE conversation_id = ?",
        params!["2026-06-30 11:00:00.000000", "c1"],
    )
    .expect("update");
    let summary = run(&reader, options(&s.out_dir));

    // Then — exactly the changed session is rewritten
    assert_eq!(written_ids(&summary), owned(&["c1"]));
    assert_eq!(sorted(summary.unchanged.clone()), owned(&["c2", "c3"]));
    let after = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));
    assert!(after["c1.jsonl"].1 > before["c1.jsonl"].1);
    assert_eq!(after["c2.jsonl"], before["c2.jsonl"]);
    assert_eq!(after["c3.jsonl"], before["c3.jsonl"]);

    // the rewritten header carries the new stamp; the re-read task event dedups
    // against the stored one (mutable-ts reconciliation, earliest ts kept)
    let c1 = read_session_file(&s.out_dir, "c1");
    assert_eq!(c1.header.get_str("last_modified"), Some("2026-06-30 11:00:00.000000"));
    let task_events: Vec<&Object> = c1
        .events
        .iter()
        .filter(|e| e.get_str("kind") == Some("agent_message"))
        .collect();
    assert_eq!(task_events.len(), 1);
    assert_eq!(task_events[0].get_str("ts"), Some("2026-06-30 10:30:00.000000"));
    assert_eq!(c1.events.len(), 3);
}

#[test]
fn evicted_queries_survive_in_the_merged_file() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    assert_eq!(
        strs(&read_session_file(&s.out_dir, "c1").events, "kind"),
        vec!["query", "command", "agent_message"]
    );

    // When — Warp evicts c1's queries; the conversations stamp changes anyway
    s.db.execute("DELETE FROM ai_queries WHERE conversation_id = ?", params!["c1"])
        .expect("delete");
    s.db.execute(
        "UPDATE agent_conversations SET last_modified_at = ? WHERE conversation_id = ?",
        params!["2026-06-30 12:00:00.000000", "c1"],
    )
    .expect("update");
    let summary = run(&reader, options(&s.out_dir));

    // Then — c1 re-gathered and re-merged: the evicted query is recovered from
    // the stored file; the fresh block + task dedup; nothing duplicated
    assert_eq!(written_ids(&summary), owned(&["c1"]));
    let c1 = read_session_file(&s.out_dir, "c1");
    assert_eq!(c1.events.len(), 3);
    let has = |kind: &str, content: &str| {
        c1.events
            .iter()
            .any(|e| e.get_str("kind") == Some(kind) && e.get_str("content") == Some(content))
    };
    assert!(has("query", "fix the login"));
    assert!(has("command", "npm test"));
    assert!(has("agent_message", "task message"));
    assert_eq!(seqs(&c1.events), vec![1.0, 2.0, 3.0]);
}

#[test]
fn conversation_filter_force_rewrites_only_that_conversation() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    let before = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));

    // When
    let summary = run(
        &reader,
        ExtractSessionsOptions {
            conversation: Some("c2".to_string()),
            ..options(&s.out_dir)
        },
    );

    // Then — exactly one enumerated session, force-gathered despite the equal
    // stamp; the other stored files are kept untouched, never GC'd
    assert_eq!(summary.enumerated, 1);
    assert_eq!(written_ids(&summary), owned(&["c2"]));
    assert!(summary.unchanged.is_empty());
    let untouched: Vec<String> = summary
        .untouched
        .iter()
        .map(|p| path::basename(p).to_string())
        .collect();
    assert_eq!(sorted(untouched), owned(&["c1.jsonl", "c3.jsonl"]));
    let after = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));
    assert_eq!(after["c1.jsonl"], before["c1.jsonl"]);
    assert_eq!(after["c3.jsonl"], before["c3.jsonl"]);
    assert_eq!(read_session_file(&s.out_dir, "c2").events.len(), 1);
}

#[test]
fn corrupt_stored_session_file_with_default_merge_throws_and_overwrites_nothing() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    let before = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));
    let corrupt_path = path::join(&[&sessions_dir_of(&s.out_dir), "c2.jsonl"]);
    fs::write(&corrupt_path, "not valid json\n").expect("write");

    // When
    let error: ReadError = extract_sessions(&reader, options(&s.out_dir)).expect_err("must fail");

    // Then — the run refuses loudly, the corrupt file is left as-is
    assert!(error.message().contains("corrupt session file"), "{error}");
    assert_eq!(fs::read_to_string(&corrupt_path).expect("read"), "not valid json\n");
    let after = snapshot_sessions_dir(&sessions_dir_of(&s.out_dir));
    assert_eq!(after["c2.jsonl"].0, "not valid json\n");
    assert_eq!(after["c1.jsonl"], before["c1.jsonl"]);
}

#[test]
fn corrupt_stored_session_file_with_no_merge_is_replaced_and_warned_about() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    fs::write(
        path::join(&[&sessions_dir_of(&s.out_dir), "c2.jsonl"]),
        "not valid json\n",
    )
    .expect("write");

    // When
    let summary = run(
        &reader,
        ExtractSessionsOptions {
            no_merge: true,
            ..options(&s.out_dir)
        },
    );

    // Then — the corrupt entry is replaced by a fresh, valid write
    assert!(summary.warnings.join("\n").contains("corrupt session file"));
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.starts_with("replacing stored session (--no-merge): "))
    );
    assert_eq!(written_ids(&summary), owned(&["c2"]));
    assert_eq!(sorted(summary.unchanged.clone()), owned(&["c1", "c3"]));
    let c2 = read_session_file(&s.out_dir, "c2");
    assert_eq!(c2.header.get_str("type"), Some("session_header"));
    assert_eq!(strs(&c2.events, "kind"), vec!["query"]);
}

#[test]
fn stored_session_file_from_another_source_is_refused_then_replaced_with_no_merge() {
    // Given — a hermes-written file squatting in the warp sessions dir
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    let foreign_path = path::join(&[&sessions_dir_of(&s.out_dir), "c2.jsonl"]);
    let foreign_header = write_header_file(
        &foreign_path,
        &[
            ("type", "session_header"),
            ("conversation_id", "c2"),
            ("source", "hermes"),
            ("last_modified", "2026-07-01 09:00:00.000000"),
            ("extracted_at", "2026-07-29T00:00:00.000Z"),
        ],
    );

    // When / Then — default: refuse, leave the file untouched
    let error = extract_sessions(&reader, options(&s.out_dir)).expect_err("must fail");
    assert!(error.message().contains("source mismatch"), "{error}");
    assert_eq!(
        fs::read_to_string(&foreign_path).expect("read"),
        format!("{foreign_header}\n")
    );

    // And with --no-merge: warn, replace with a fresh warp file
    let summary = run(
        &reader,
        ExtractSessionsOptions {
            no_merge: true,
            ..options(&s.out_dir)
        },
    );
    assert!(summary.warnings.join("\n").contains("source mismatch"));
    assert_eq!(
        read_session_file(&s.out_dir, "c2").header.get_str("source"),
        Some("warp")
    );
}

#[test]
fn foreign_source_file_for_a_conversation_this_source_does_not_hold_is_left_untouched() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    fs::create_dir_all(sessions_dir_of(&s.out_dir)).expect("mkdir");
    let other_path = path::join(&[
        &sessions_dir_of(&s.out_dir),
        "0de8452a-214b-405a-b979-46981d093287.jsonl",
    ]);
    let other_header = write_header_file(
        &other_path,
        &[
            ("type", "session_header"),
            ("conversation_id", "0de8452a-214b-405a-b979-46981d093287"),
            ("source", "claude-code"),
            ("last_modified", "2026-10-20T19:11:00.759Z"),
            ("extracted_at", "2026-10-20T19:11:05.000Z"),
        ],
    );

    // When
    let summary = run(&reader, options(&s.out_dir));

    // Then
    assert_eq!(sorted(written_ids(&summary)), owned(&["c1", "c2", "c3"]));
    assert!(summary.untouched.is_empty());
    assert!(summary.warnings.is_empty());
    assert_eq!(
        fs::read_to_string(&other_path).expect("read"),
        format!("{other_header}\n")
    );
}

#[test]
fn header_conversation_id_not_matching_its_filename_is_refused() {
    // Given
    let s = setup();
    let reader = reader(&s.db_path);
    run(&reader, options(&s.out_dir));
    let mismatch_path = path::join(&[&sessions_dir_of(&s.out_dir), "not-c2.jsonl"]);
    let mismatch_header = write_header_file(
        &mismatch_path,
        &[
            ("type", "session_header"),
            ("conversation_id", "c2"),
            ("source", "warp"),
            ("last_modified", "2026-07-01 09:00:00.000000"),
            ("extracted_at", "2026-07-29T00:00:00.000Z"),
        ],
    );

    // When / Then
    let error = extract_sessions(&reader, options(&s.out_dir)).expect_err("must fail");
    assert!(error.message().contains("conversation mismatch"), "{error}");
    assert_eq!(
        fs::read_to_string(&mismatch_path).expect("read"),
        format!("{mismatch_header}\n")
    );
}

#[test]
fn conversation_id_with_path_separators_is_sanitized_into_sessions_dir() {
    // Given — an evil cid that would escape the outDir if used verbatim
    let s = setup();
    let evil_cid = "../../etc/escape";
    seed_conversation(&s.db, evil_cid, Some("2026-06-30 10:00:00.000000"));
    seed_query(
        &s.db,
        QuerySeed {
            conversation_id: evil_cid,
            start_ts: "2026-06-30 10:10:00.000000",
            text: "evil cid",
            ..QuerySeed::default()
        },
    );

    // When
    let summary = run(&reader(&s.db_path), options(&s.out_dir));

    // Then — the evil cid is written exactly once, as a flat, safe name directly
    // in sessions/ (alongside the seeded conversations, all flat)
    let sessions_dir = sessions_dir_of(&s.out_dir);
    let written_evil: Vec<_> = summary
        .written
        .iter()
        .filter(|w| w.conversation_id == evil_cid)
        .collect();
    assert_eq!(written_evil.len(), 1);
    let written_path = &written_evil[0].path;
    assert_eq!(path::dirname(written_path), sessions_dir);
    let base = path::basename(written_path);
    let stem = base.strip_suffix(".jsonl").expect(".jsonl suffix");
    assert!(
        !stem.is_empty()
            && stem
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
    );
    // digest suffix: -[0-9a-f]{8}.jsonl
    let digest = &stem[stem.len() - 9..];
    assert!(digest.starts_with('-') && digest[1..].chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
    let mut expected = owned(&["c1.jsonl", "c2.jsonl", "c3.jsonl", base]);
    expected.sort();
    assert_eq!(sorted_dir(&sessions_dir), expected);
    // nothing outside the outDir tree
    assert!(
        !std::path::Path::new(&s.tmp.path())
            .join("../../etc/escape.jsonl")
            .exists()
    );
    assert!(!std::path::Path::new(&s.out_dir).join("../escape.jsonl").exists());
    // the header preserves the original, unsanitized conversation id
    assert_eq!(
        read_session_file(&s.out_dir, evil_cid)
            .header
            .get_str("conversation_id"),
        Some(evil_cid)
    );
}

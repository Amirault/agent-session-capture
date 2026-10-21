//! Port of `hermesSessionReader.test.ts` (in-process cases; the CLI case
//! lives in `tests/hermes_session_reader_cli.rs`).
//! HermesConversationReader.readSessions — one session per lineage root.

use super::fixtures::TempDir;
use super::hermes_fixture::{
    BRANCH_ID, CONTINUATION_ID, ROOT_ID, SessionRow, create_session_hermes_db, read_jsonl, seed_example3,
    seed_session_row, seed_user_message,
};
use crate::adapters::hermes_conversation_reader::HermesConversationReader;
use crate::domain::models::{BundleSource, SessionHeader, SessionInfo};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{ReadSessionsOptions, SessionReader, SessionsRead};
use crate::js::path;
use crate::usecases::extract_sessions::{ExtractSessionsOptions, extract_sessions};

struct Fixture {
    _temp: TempDir,
    db_path: String,
    out_dir: String,
}

fn fixture() -> Fixture {
    let temp = TempDir::new("wa-hermes-sessions-");
    let db_path = temp.join("state.db");
    let out_dir = temp.join("out");
    Fixture {
        _temp: temp,
        db_path,
        out_dir,
    }
}

fn reader(db_path: &str) -> HermesConversationReader {
    HermesConversationReader::new(Some(db_path.to_string()))
}

fn session(conversation_id: &str, last_modified: &str) -> SessionInfo {
    SessionInfo {
        conversation_id: conversation_id.to_string(),
        last_modified: Some(last_modified.to_string()),
    }
}

fn draft_pairs(read: &SessionsRead, cid: &str) -> Option<Vec<(String, String)>> {
    read.reads.get(&cid.to_string()).map(|gather| {
        gather
            .drafts
            .iter()
            .map(|d| (d.conversation_id.clone(), d.content.clone()))
            .collect()
    })
}

fn draft_contents(read: &SessionsRead, cid: &str) -> Option<Vec<String>> {
    read.reads
        .get(&cid.to_string())
        .map(|gather| gather.drafts.iter().map(|d| d.content.clone()).collect())
}

fn session_ids(read: &SessionsRead) -> Vec<&str> {
    read.sessions.iter().map(|s| s.conversation_id.as_str()).collect()
}

fn read_keys(read: &SessionsRead) -> Vec<&str> {
    read.reads.keys().map(String::as_str).collect()
}

fn pairs(cid: &str, contents: &[&str]) -> Vec<(String, String)> {
    contents.iter().map(|c| (cid.to_string(), c.to_string())).collect()
}

#[test]
fn given_a_root_its_compression_continuation_and_a_branch_child_when_read_sessions_then_continuation_joins_root_and_branch_is_its_own()
 {
    // Given
    let f = fixture();
    seed_example3(&f.db_path);

    // When
    let read = reader(&f.db_path)
        .read_sessions(&ReadSessionsOptions::default())
        .expect("read sessions");

    // Then
    assert_eq!(read.source, BundleSource::Hermes);
    assert_eq!(
        read.sessions,
        vec![
            session(ROOT_ID, "2026-10-15T11:45:00.000Z"), // the continuation's latest message
            session(BRANCH_ID, "2026-10-15T12:00:01.000Z"),
        ]
    );
    assert_eq!(
        draft_pairs(&read, ROOT_ID),
        Some(pairs(
            ROOT_ID,
            &[
                "root question",
                "root follow-up",
                "continued after compression",
                "continuation follow-up"
            ]
        ))
    );
    assert_eq!(
        draft_pairs(&read, BRANCH_ID),
        Some(pairs(BRANCH_ID, &["explore an alternative"]))
    );
    assert!(!read.reads.contains_key(&CONTINUATION_ID.to_string()));
}

fn options(out_dir: &str) -> ExtractSessionsOptions {
    ExtractSessionsOptions {
        out_dir: out_dir.to_string(),
        source: BundleSource::Hermes,
        since: None,
        conversation: None,
        no_merge: false,
    }
}

#[test]
fn given_example3_when_extract_sessions_reruns_unchanged_then_first_writes_root_and_branch_files_and_second_reports_both_unchanged()
 {
    // Given
    let f = fixture();
    seed_example3(&f.db_path);
    let reader = reader(&f.db_path);
    let first = extract_sessions(&reader, options(&f.out_dir)).expect("first run");
    let sessions_dir = path::join(&[&f.out_dir, "sessions"]);
    let root_file = path::join(&[&sessions_dir, &format!("{ROOT_ID}.jsonl")]);
    let root_bytes = std::fs::read_to_string(&root_file).expect("root file");

    // When
    let second = extract_sessions(&reader, options(&f.out_dir)).expect("second run");

    // Then
    let written: Vec<(String, usize)> = first
        .written
        .iter()
        .map(|w| (path::basename(&w.path).to_string(), w.events))
        .collect();
    assert_eq!(
        written,
        vec![(format!("{ROOT_ID}.jsonl"), 4), (format!("{BRANCH_ID}.jsonl"), 1)]
    );
    let mut files: Vec<String> = std::fs::read_dir(&sessions_dir)
        .expect("sessions dir")
        .map(|entry| entry.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    files.sort();
    assert_eq!(files, vec![format!("{ROOT_ID}.jsonl"), format!("{BRANCH_ID}.jsonl")]);
    let lines = read_jsonl(&root_file);
    let (header, events) = lines.split_first().expect("header line");
    assert_eq!(header.get_str("type"), Some("session_header"));
    assert_eq!(header.get_str("conversation_id"), Some(ROOT_ID));
    assert_eq!(header.get_str("source"), Some("hermes"));
    assert_eq!(header.get_str("last_modified"), Some("2026-10-15T11:45:00.000Z"));
    let event_pairs: Vec<(String, String)> = events
        .iter()
        .map(|e| {
            (
                e.get_str("conversation_id").unwrap_or_default().to_string(),
                e.get_str("content").unwrap_or_default().to_string(),
            )
        })
        .collect();
    assert_eq!(
        event_pairs,
        pairs(
            ROOT_ID,
            &[
                "root question",
                "root follow-up",
                "continued after compression",
                "continuation follow-up"
            ]
        )
    );

    assert_eq!(second.enumerated, 2);
    assert!(second.written.is_empty());
    assert_eq!(second.unchanged, vec![ROOT_ID.to_string(), BRANCH_ID.to_string()]);
    assert_eq!(std::fs::read_to_string(&root_file).expect("root file"), root_bytes);
}

#[test]
fn given_delegate_generic_and_tool_children_of_a_root_when_read_sessions_then_delegate_joins_root_while_others_are_own_sessions()
 {
    // Given
    let f = fixture();
    let db = create_session_hermes_db(&f.db_path);
    seed_session_row(
        &db,
        SessionRow {
            id: "root",
            ..Default::default()
        },
        "2026-10-15T09:00:00Z",
    );
    seed_user_message(&db, "root", "root", "2026-10-15T09:00:01Z");
    seed_session_row(
        &db,
        SessionRow {
            id: "delegate",
            parent: Some("root"),
            source: Some("subagent"),
            ..Default::default()
        },
        "2026-10-15T09:01:00Z",
    );
    seed_user_message(&db, "delegate", "delegate", "2026-10-15T09:01:01Z");
    seed_session_row(
        &db,
        SessionRow {
            id: "delegate-by-config",
            parent: Some("root"),
            model_config: Some(r#"{"_delegate_from":"root"}"#),
            ..Default::default()
        },
        "2026-10-15T09:02:00Z",
    );
    seed_user_message(&db, "delegate-by-config", "delegate-by-config", "2026-10-15T09:02:01Z");
    seed_session_row(
        &db,
        SessionRow {
            id: "generic",
            parent: Some("root"),
            ..Default::default()
        },
        "2026-10-15T09:03:00Z",
    );
    seed_user_message(&db, "generic", "generic", "2026-10-15T09:03:01Z");
    seed_session_row(
        &db,
        SessionRow {
            id: "tool",
            parent: Some("root"),
            source: Some("tool"),
            ..Default::default()
        },
        "2026-10-15T09:04:00Z",
    );
    seed_user_message(&db, "tool", "tool", "2026-10-15T09:04:01Z");
    drop(db);

    // When
    let read = reader(&f.db_path)
        .read_sessions(&ReadSessionsOptions::default())
        .expect("read sessions");

    // Then
    assert_eq!(session_ids(&read), vec!["root", "generic", "tool"]);
    assert_eq!(
        draft_contents(&read, "root"),
        Some(vec![
            "root".to_string(),
            "delegate".to_string(),
            "delegate-by-config".to_string()
        ])
    );
    assert_eq!(draft_contents(&read, "generic"), Some(vec!["generic".to_string()]));
    assert_eq!(draft_contents(&read, "tool"), Some(vec!["tool".to_string()]));
}

#[test]
fn given_sessions_last_active_before_at_and_after_the_cutoff_when_read_sessions_with_since_then_only_those_at_or_after_are_enumerated()
 {
    // Given
    let f = fixture();
    let db = create_session_hermes_db(&f.db_path);
    let seed = |id: &str, started_at: &str, content: &str, timestamp: &str| {
        seed_session_row(
            &db,
            SessionRow {
                id,
                ..Default::default()
            },
            started_at,
        );
        seed_user_message(&db, id, content, timestamp);
    };
    seed("hermes-0412", "2026-09-28T15:00:00Z", "old", "2026-09-28T16:00:00Z");
    seed("hermes-0419", "2026-10-17T08:00:00Z", "new", "2026-10-17T09:00:00Z");
    seed("edge", "2026-09-30T23:00:00Z", "edge", "2026-10-01T00:00:00Z");
    drop(db);

    // When
    let read = reader(&f.db_path)
        .read_sessions(&ReadSessionsOptions {
            since: Some("2026-10-01".to_string()),
            ..Default::default()
        })
        .expect("read sessions");

    // Then
    assert_eq!(session_ids(&read), vec!["edge", "hermes-0419"]); // at/after, inclusive
    assert_eq!(read_keys(&read), vec!["edge", "hermes-0419"]);
}

#[test]
fn given_two_sessions_when_read_sessions_with_conversation_then_exactly_that_session_is_enumerated_and_gathered_even_when_unchanged()
 {
    // Given
    let f = fixture();
    let db = create_session_hermes_db(&f.db_path);
    let seed = |id: &str, started_at: &str, content: &str, timestamp: &str| {
        seed_session_row(
            &db,
            SessionRow {
                id,
                ..Default::default()
            },
            started_at,
        );
        seed_user_message(&db, id, content, timestamp);
    };
    seed("hermes-0412", "2026-09-28T15:00:00Z", "old", "2026-09-28T16:00:00Z");
    seed("hermes-0419", "2026-10-17T08:00:00Z", "new", "2026-10-17T09:00:00Z");
    drop(db);
    let mut stored_by_cid = OrderedMap::new();
    stored_by_cid.insert(
        "hermes-0412".to_string(),
        SessionHeader::fresh(
            "hermes-0412",
            BundleSource::Hermes,
            Some("2026-09-28T16:00:00.000Z"),
            "2026-10-01T00:00:00.000Z",
        ),
    );

    // When
    let read = reader(&f.db_path)
        .read_sessions(&ReadSessionsOptions {
            conversation: Some("hermes-0412".to_string()),
            stored_by_cid,
            ..Default::default()
        })
        .expect("read sessions");

    // Then
    assert_eq!(session_ids(&read), vec!["hermes-0412"]);
    assert_eq!(read_keys(&read), vec!["hermes-0412"]);
    assert_eq!(draft_contents(&read, "hermes-0412"), Some(vec!["old".to_string()]));
}

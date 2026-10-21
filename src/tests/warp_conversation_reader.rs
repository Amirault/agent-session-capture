//! §9.12 WarpConversationReader — spec binding + event gathering.

use super::fixtures::TempDir;
use super::fixtures::fake_source::fake_source;
use super::fixtures::fixture_db::{MarkerSeed, QuerySeed, create_fixture, seed_marker, seed_query};
use crate::adapters::warp_conversation_reader::WarpConversationReader;
use crate::domain::models::EventKind;
use crate::domain::ports::ConversationReader;

const SPEC: &str = "add-feature-x";

#[test]
fn bound_conversation_and_unbindable_marker_are_bound_gathered_and_surfaced() {
    // Given — one bound specify conversation (c1) with a query, plus an unbindable implement marker
    let tmp = TempDir::new("wa-912-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    seed_marker(
        &db,
        MarkerSeed {
            capture_id: SPEC,
            label: "specify",
            conversation_id: "c1",
            start_ts: "2026-06-30 10:00:00.000000",
        },
    );
    seed_query(
        &db,
        QuerySeed {
            conversation_id: "c1",
            start_ts: "2026-06-30 10:05:00.000000",
            text: "spec it",
            ..QuerySeed::default()
        },
    );
    db.execute(
        "INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)",
        rusqlite::params![
            format!(": CAPTURE_MARKER v=1 id={SPEC} label=implement"),
            "2026-06-30 11:00:00.000000"
        ],
    )
    .expect("insert");

    // When
    let read = WarpConversationReader::new(fake_source(&db_path))
        .read_capture(SPEC)
        .expect("read");

    // Then — c1 bound to specify; its query plus the marker's binding block are
    // gathered (queries before blocks), and the implement marker is unbindable.
    assert_eq!(
        read.label_by_cid.get(&"c1".to_string()).map(String::as_str),
        Some("specify")
    );
    assert_eq!(read.label_by_cid.len(), 1);
    assert_eq!(read.drafts.len(), 2);
    assert_eq!(read.drafts[0].conversation_id, "c1");
    assert_eq!(read.drafts[0].kind, EventKind::Query);
    assert_eq!(read.drafts[0].content, "spec it");
    assert_eq!(read.drafts[1].conversation_id, "c1");
    assert_eq!(read.drafts[1].kind, EventKind::Command);
    assert_eq!(
        read.drafts[1].content,
        ": CAPTURE_MARKER v=1 id=add-feature-x label=specify"
    );
    assert_eq!(read.unbindable.len(), 1);
    assert_eq!(read.unbindable[0].label, "implement");
    assert!(read.collisions.is_empty());
    assert!(read.skipped.is_empty());
}

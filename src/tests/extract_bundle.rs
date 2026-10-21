//! Port of `extractBundle.test.ts`.

use super::fixtures::TempDir;
use super::fixtures::fake_source::fake_source;
use super::fixtures::fixture_db::{
    BlockSeed, MarkerSeed, QuerySeed, create_fixture, seed_block, seed_marker, seed_query, seed_task,
    seed_unbindable_marker,
};
use super::fixtures::protobuf::encode_string;
use crate::adapters::warp_conversation_reader::WarpConversationReader;
use crate::domain::bundle_header::compute_bundle_header;
use crate::domain::models::{BundleSource, CaptureBundle, EventDraft, EventKind, Role, conversation_event};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{CaptureRead, ConversationReader};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value};
use crate::usecases::extract_bundle::{ExtractOptions, extract_bundle};

const SPEC: &str = "add-feature-x";

fn task_blob(text: &str) -> Vec<u8> {
    encode_string(1, text)
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(*v)).collect())
}

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

fn warp_reader(db_path: &str) -> WarpConversationReader<super::fixtures::fake_source::FakeSource> {
    WarpConversationReader::new(fake_source(db_path))
}

// --- §9.7 extractBundle tally ---

#[test]
fn spec_with_all_three_labels_tallies_all_three_labels() {
    // Given — one conversation per label, each with a marker + a query
    let tmp = TempDir::new("wa-97-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    marker(&db, "specify", "c1", "2026-06-30 10:00:00.000000");
    query(&db, "c1", "2026-06-30 10:05:00.000000", "spec it");
    marker(&db, "implement", "c2", "2026-06-30 11:00:00.000000");
    query(&db, "c2", "2026-06-30 11:05:00.000000", "implement it");
    marker(&db, "review", "c3", "2026-06-30 12:00:00.000000");
    query(&db, "c3", "2026-06-30 12:05:00.000000", "gate it");

    // When
    let result = extract_bundle(&warp_reader(&db_path), SPEC, ExtractOptions::default()).expect("extract");

    // Then
    let header = &result.bundle.header;
    assert_eq!(
        header.get("labels"),
        Some(&strings(&["specify", "implement", "review"]))
    );
    let mut per_label = Object::new();
    per_label.insert("specify", 1.0);
    per_label.insert("implement", 1.0);
    per_label.insert("review", 1.0);
    assert_eq!(header.get("conversations_per_label"), Some(&Value::Object(per_label)));
}

#[test]
fn spec_missing_a_label_plus_unbindable_marker_surfaces_the_warning() {
    // Given — specify + implement only, and one unbindable marker (no binding block)
    let tmp = TempDir::new("wa-97-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    marker(&db, "specify", "c1", "2026-06-30 10:00:00.000000");
    marker(&db, "implement", "c2", "2026-06-30 11:00:00.000000");
    seed_unbindable_marker(&db, SPEC, "review", "2026-06-30 12:00:00.000000");

    // When
    let result = extract_bundle(&warp_reader(&db_path), SPEC, ExtractOptions::default()).expect("extract");

    // Then — incomplete, missing label reported, unbindable marker surfaced as a warning
    assert_eq!(result.summary.unbindable.len(), 1);
    assert_eq!(result.summary.unbindable[0].label, "review");
}

#[test]
fn re_runs_of_a_label_keep_both_conversations() {
    // Given — two review conversations for the same spec
    let tmp = TempDir::new("wa-97-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    marker(&db, "specify", "c1", "2026-06-30 10:00:00.000000");
    marker(&db, "implement", "c2", "2026-06-30 11:00:00.000000");
    marker(&db, "review", "c3", "2026-06-30 12:00:00.000000");
    marker(&db, "review", "c4", "2026-06-30 13:00:00.000000");

    // When
    let result = extract_bundle(&warp_reader(&db_path), SPEC, ExtractOptions::default()).expect("extract");

    // Then — both re-run conversations kept, still complete
    let header = &result.bundle.header;
    let per_label = header
        .get("conversations_per_label")
        .and_then(Value::as_object)
        .expect("tally");
    assert_eq!(per_label.get("review"), Some(&Value::Number(2.0)));
    assert_eq!(
        header.get("conversation_ids"),
        Some(&strings(&["c1", "c2", "c3", "c4"]))
    );
    assert_eq!(result.summary.conversations, 4);
}

// --- §9.8 extractBundle ordering ---

#[test]
fn events_across_labels_are_ordered_by_ts_with_monotonic_seq_and_correct_label() {
    // Given — out-of-insertion-order timestamps across all three labels and all reader kinds
    let tmp = TempDir::new("wa-98-");
    let db_path = tmp.join("f.db");
    let db = create_fixture(&db_path, false);
    // specify conversation c1
    marker(&db, "specify", "c1", "2026-06-30 09:00:00.000000");
    seed_task(
        &db,
        "c1",
        &task_blob("spec task"),
        None,
        Some("2026-06-30 09:30:00.000000"),
    );
    query(&db, "c1", "2026-06-30 09:10:00.000000", "spec query");
    // implement conversation c2
    marker(&db, "implement", "c2", "2026-06-30 10:00:00.000000");
    seed_block(
        &db,
        BlockSeed {
            conversation_id: "c2",
            start_ts: "2026-06-30 10:20:00.000000",
            command: Some("npm test"),
            ..BlockSeed::default()
        },
    );
    query(&db, "c2", "2026-06-30 10:10:00.000000", "impl query");
    // gate conversation c3
    marker(&db, "review", "c3", "2026-06-30 11:00:00.000000");
    query(&db, "c3", "2026-06-30 11:10:00.000000", "gate query");

    // When
    let result = extract_bundle(&warp_reader(&db_path), SPEC, ExtractOptions::default()).expect("extract");

    // Then — non-decreasing timestamps and a 1..N monotonic sequence
    let events = &result.bundle.events;
    let ts: Vec<&str> = events.iter().map(|e| e.get_str("ts").expect("ts")).collect();
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(sorted, ts); // sorted ascending
    let seq: Vec<f64> = events
        .iter()
        .map(|e| e.get("seq").and_then(Value::as_f64).expect("seq"))
        .collect();
    let expected: Vec<f64> = (1..=events.len()).map(|i| i as f64).collect();
    assert_eq!(seq, expected); // 1..N (hence unique)
    // label matches the conversation that emitted each event
    for e in events {
        let expected_label = match e.get_str("conversation_id") {
            Some("c1") => "specify",
            Some("c2") => "implement",
            Some("c3") => "review",
            other => panic!("unexpected conversation {other:?}"),
        };
        assert_eq!(e.get_str("label"), Some(expected_label));
    }
}

// --- helpers for merge / fallback tests (no fixture DB needed) ---

fn query_drafts(conversation_id: &str, prompts: &[(&str, &str)]) -> Vec<EventDraft> {
    prompts
        .iter()
        .map(|(ts, content)| EventDraft {
            conversation_id: conversation_id.to_string(),
            ts: ts.to_string(),
            role: Role::User,
            kind: EventKind::Query,
            content: content.to_string(),
            meta: Object::new(),
        })
        .collect()
}

fn spec_read_with(label: &str, conversation_id: &str, prompts: &[(&str, &str)]) -> CaptureRead {
    let mut label_by_cid = OrderedMap::new();
    label_by_cid.insert(conversation_id.to_string(), label.to_string());
    CaptureRead {
        source: BundleSource::Warp,
        label_by_cid,
        drafts: query_drafts(conversation_id, prompts),
        skipped: vec![],
        unbindable: vec![],
        collisions: vec![],
        heuristic_bindings: vec![],
    }
}

struct FakeReader(CaptureRead);

impl ConversationReader for FakeReader {
    fn read_capture(&self, _capture_id: &str) -> Result<CaptureRead, ReadError> {
        Ok(self.0.clone())
    }
}

struct ThrowingReader(ReadError);

impl ConversationReader for ThrowingReader {
    fn read_capture(&self, _capture_id: &str) -> Result<CaptureRead, ReadError> {
        Err(self.0.clone())
    }
}

fn stored_bundle(label: &str, conversation_id: &str, prompts: &[(&str, &str)]) -> CaptureBundle {
    let mut label_by_cid = OrderedMap::new();
    label_by_cid.insert(conversation_id.to_string(), label.to_string());
    let events = query_drafts(conversation_id, prompts)
        .iter()
        .enumerate()
        .map(|(i, d)| conversation_event(SPEC, label, d, i + 1))
        .collect();
    CaptureBundle {
        header: compute_bundle_header(SPEC, BundleSource::Warp, &label_by_cid, "2026-07-14T00:00:00.000Z"),
        events,
    }
}

// --- §9.17 extractBundle merge + fresh-read-error fallback ---

#[test]
fn stored_specify_and_fresh_implement_merge_into_both_labels() {
    // Given — specify captured at close (stored); fresh read has implement only
    let existing = stored_bundle("specify", "c1", &[("2026-07-14 09:00:00.000000", "spec it")]);
    let reader = FakeReader(spec_read_with(
        "implement",
        "c2",
        &[("2026-07-14 10:00:00.000000", "implement it")],
    ));

    // When
    let result = extract_bundle(
        &reader,
        SPEC,
        ExtractOptions {
            existing_bundle: Some(existing),
            ..ExtractOptions::default()
        },
    )
    .expect("extract");

    // Then — both labels present (specify from disk, implement from live)
    assert_eq!(
        result.bundle.header.get("labels"),
        Some(&strings(&["specify", "implement"]))
    );
    assert_eq!(result.summary.fresh_read_error, None);
    assert_eq!(result.bundle.events.len(), 2);
}

#[test]
fn throwing_reader_with_stored_bundle_degrades_to_stored_only() {
    // Given — fresh read fails (DB locked); stored bundle has specify + implement
    let existing = stored_bundle("specify", "c1", &[("2026-07-14 09:00:00.000000", "spec it")]);
    let reader = ThrowingReader(ReadError::Other("database is locked".to_string()));

    // When
    let result = extract_bundle(
        &reader,
        SPEC,
        ExtractOptions {
            existing_bundle: Some(existing.clone()),
            ..ExtractOptions::default()
        },
    )
    .expect("extract");

    // Then — stored bundle preserved intact, fresh_read_error set
    assert!(
        result
            .summary
            .fresh_read_error
            .as_deref()
            .expect("fresh_read_error")
            .contains("database is locked")
    );
    assert_eq!(result.bundle.events.len(), existing.events.len());
    assert_eq!(result.bundle.header.get("labels"), existing.header.get("labels"));
}

#[test]
fn throwing_reader_without_stored_bundle_throws() {
    // Given — fresh read fails, nothing on disk to fall back on
    let reader = ThrowingReader(ReadError::Other("database is locked".to_string()));

    // When
    let error = extract_bundle(&reader, SPEC, ExtractOptions::default())
        .err()
        .expect("must fail");

    // Then
    assert!(error.message().contains("fresh read failed"));
}

#[test]
fn source_access_error_propagates_instead_of_degrading_to_stored_bundle() {
    // Given
    let existing = stored_bundle("specify", "c1", &[("2026-07-14 09:00:00.000000", "spec it")]);
    let reader = ThrowingReader(ReadError::SourceAccess(
        "Langfuse API 401 at https://cloud.langfuse.com".to_string(),
    ));

    // When
    let extraction = extract_bundle(
        &reader,
        SPEC,
        ExtractOptions {
            existing_bundle: Some(existing),
            ..ExtractOptions::default()
        },
    );

    // Then
    assert_eq!(
        extraction.err(),
        Some(ReadError::SourceAccess(
            "Langfuse API 401 at https://cloud.langfuse.com".to_string()
        ))
    );
}

#[test]
fn existing_bundle_with_no_merge_keeps_fresh_only() {
    // Given — stored specify; fresh read has implement; --no-merge replaces
    let existing = stored_bundle("specify", "c1", &[("2026-07-14 09:00:00.000000", "spec it")]);
    let reader = FakeReader(spec_read_with(
        "implement",
        "c2",
        &[("2026-07-14 10:00:00.000000", "implement it")],
    ));

    // When
    let result = extract_bundle(
        &reader,
        SPEC,
        ExtractOptions {
            existing_bundle: Some(existing),
            no_merge: true,
        },
    )
    .expect("extract");

    // Then — fresh only: specify NOT recovered (replaced, not merged)
    assert_eq!(result.bundle.header.get("labels"), Some(&strings(&["implement"])));
    assert_eq!(result.bundle.events.len(), 1);
}

//! Port of `mergeBundle.test.ts`.

use std::collections::HashSet;

use crate::domain::merge_bundle::merge_bundles;
use crate::domain::models::{CaptureBundle, EventRecord};
use crate::js::{Object, Value};

const SPEC: &str = "2026-07-14-foo";
const EXTRACTED_AT: &str = "2026-07-14T12:00:00.000Z";

fn event(conversation_id: &str, label: &str, ts: &str, content: &str, seq: usize) -> EventRecord {
    let mut event = Object::new();
    event.insert("capture_id", SPEC);
    event.insert("label", label);
    event.insert("conversation_id", conversation_id);
    event.insert("seq", seq);
    event.insert("ts", ts);
    event.insert("role", "user");
    event.insert("kind", "query");
    event.insert("content", content);
    event.insert("meta", Object::new());
    event
}

fn with_meta(mut event: EventRecord, entries: &[(&str, Value)]) -> EventRecord {
    let mut meta = Object::new();
    for (key, value) in entries {
        meta.insert(*key, value.clone());
    }
    event.insert("meta", meta);
    event
}

fn as_agent_message(mut event: EventRecord) -> EventRecord {
    event.insert("role", "assistant");
    event.insert("kind", "agent_message");
    event
}

/// Build a bundle whose header is structurally valid (merge recomputes it).
fn bundle(events: Vec<EventRecord>) -> CaptureBundle {
    let mut header = Object::new();
    header.insert("type", "bundle_header");
    header.insert("capture_id", SPEC);
    header.insert("labels", Vec::<Value>::new());
    header.insert("conversations_per_label", Object::new());
    header.insert("conversation_ids", Vec::<Value>::new());
    header.insert("extracted_at", "2026-07-14T00:00:00.000Z");
    header.insert("source", "warp");
    CaptureBundle { header, events }
}

fn strings(values: &[&str]) -> Value {
    Value::Array(values.iter().map(|v| Value::from(*v)).collect())
}

fn field<'a>(events: &'a [EventRecord], key: &str) -> Vec<&'a str> {
    events.iter().map(|e| e.get_str(key).expect("string field")).collect()
}

fn meta_of<'a>(event: &'a EventRecord, key: &str) -> Option<&'a Value> {
    event.get("meta").and_then(Value::as_object).and_then(|m| m.get(key))
}

// --- §9.16 mergeBundles decay-safe merge ---

#[test]
fn stored_specify_and_fresh_implement_keep_both_labels() {
    // Given — specify captured at close (stored); implement just closed fresh;
    // specify's marker decayed so it is absent from the fresh read.
    let stored = bundle(vec![event("c1", "specify", "2026-07-14 09:00:00.000000", "spec it", 1)]);
    let fresh = bundle(vec![event(
        "c2",
        "implement",
        "2026-07-14 10:00:00.000000",
        "implement it",
        1,
    )]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then — both labels survive; specify recovered from disk, implement from live
    assert_eq!(merged.header.get("labels"), Some(&strings(&["specify", "implement"])));
    assert_eq!(merged.events.len(), 2);
    assert_eq!(field(&merged.events, "label"), vec!["specify", "implement"]);
}

#[test]
fn overlapping_reread_events_dedup_by_natural_key() {
    // Given — both bundles carry the same specify + implement events (re-read,
    // not yet decayed). seq differs between the two copies (per-run).
    let stored = bundle(vec![
        event("c1", "specify", "2026-07-14 09:00:00.000000", "spec it", 1),
        event("c1", "specify", "2026-07-14 09:10:00.000000", "spec more", 2),
        event("c2", "implement", "2026-07-14 10:00:00.000000", "implement it", 1),
    ]);
    // fresh re-read: same content, different seq assignment
    let fresh = bundle(vec![
        event("c1", "specify", "2026-07-14 09:00:00.000000", "spec it", 1),
        event("c1", "specify", "2026-07-14 09:10:00.000000", "spec more", 2),
        event("c2", "implement", "2026-07-14 10:00:00.000000", "implement it", 3),
    ]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then — exactly 3 events (no duplicates), header recomputed from merged set
    assert_eq!(merged.events.len(), 3);
    let keys: HashSet<String> = merged
        .events
        .iter()
        .map(|e| {
            format!(
                "{}:{}:{}",
                e.get_str("conversation_id").unwrap_or_default(),
                e.get_str("ts").unwrap_or_default(),
                e.get_str("content").unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(keys.len(), 3);
    assert_eq!(merged.header.get("labels"), Some(&strings(&["specify", "implement"])));
    let mut per_label = Object::new();
    per_label.insert("specify", 1.0);
    per_label.insert("implement", 1.0);
    assert_eq!(
        merged.header.get("conversations_per_label"),
        Some(&Value::Object(per_label))
    );
}

#[test]
fn reread_with_later_mutable_timestamp_keeps_one_event_with_earlier_ts() {
    // Given
    let stored_event = with_meta(
        as_agent_message(event(
            "c1",
            "review",
            "2026-07-14 09:00:00.000000",
            "same task event",
            1,
        )),
        &[
            ("field_path", Value::from("messages.agent_output.text")),
            ("repeat", Value::from(1.0)),
        ],
    );
    let fresh_event = with_meta(
        as_agent_message(event(
            "c1",
            "review",
            "2026-07-14 10:00:00.000000",
            "same task event",
            1,
        )),
        &[
            ("field_path", Value::from("messages.agent_output.text")),
            ("repeat", Value::from(2.0)),
        ],
    );
    let stored = bundle(vec![stored_event]);
    let fresh = bundle(vec![fresh_event]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then
    assert_eq!(merged.events.len(), 1);
    assert_eq!(merged.events[0].get_str("ts"), Some("2026-07-14 09:00:00.000000"));
    assert_eq!(meta_of(&merged.events[0], "repeat"), Some(&Value::Number(2.0)));
}

#[test]
fn evicted_stored_query_and_distinct_fresh_query_with_same_content_keep_both() {
    // Given
    let query_meta = [("cwd", Value::from("/repo")), ("model", Value::from("model-a"))];
    let stored_event = with_meta(
        event("c1", "review", "2026-07-14 09:00:00.000000", "continue", 1),
        &query_meta,
    );
    let fresh_event = with_meta(
        event("c1", "review", "2026-07-14 10:00:00.000000", "continue", 1),
        &query_meta,
    );

    // When
    let merged = merge_bundles(&bundle(vec![stored_event]), &bundle(vec![fresh_event]), EXTRACTED_AT);

    // Then
    assert_eq!(merged.events.len(), 2);
    assert_eq!(
        field(&merged.events, "ts"),
        vec!["2026-07-14 09:00:00.000000", "2026-07-14 10:00:00.000000"]
    );
}

#[test]
fn legacy_stored_task_event_and_identified_fresh_reread_match_into_one_earliest_event() {
    // Given
    let stored_event = with_meta(
        as_agent_message(event(
            "c1",
            "review",
            "2026-07-14 09:00:00.000000",
            "same task event",
            1,
        )),
        &[("field_path", Value::from("messages.agent_output.text"))],
    );
    let fresh_event = with_meta(
        as_agent_message(event(
            "c1",
            "review",
            "2026-07-14 10:00:00.000000",
            "same task event",
            1,
        )),
        &[
            ("field_path", Value::from("messages.agent_output.text")),
            ("task_id", Value::from("task-1")),
            ("task_event_index", Value::from(4.0)),
        ],
    );

    // When
    let merged = merge_bundles(&bundle(vec![stored_event]), &bundle(vec![fresh_event]), EXTRACTED_AT);

    // Then
    assert_eq!(merged.events.len(), 1);
    assert_eq!(merged.events[0].get_str("ts"), Some("2026-07-14 09:00:00.000000"));
    assert_eq!(meta_of(&merged.events[0], "task_id"), Some(&Value::from("task-1")));
    assert_eq!(
        meta_of(&merged.events[0], "task_event_index"),
        Some(&Value::Number(4.0))
    );
}

#[test]
fn same_content_events_with_distinct_stable_surrogates_both_survive() {
    // Given
    let first = with_meta(
        event("c1", "review", "2026-07-14 09:00:00.000000", "continue", 1),
        &[
            ("message_id", Value::from(41.0)),
            ("message_event_index", Value::from(0.0)),
        ],
    );
    let second = with_meta(
        event("c1", "review", "2026-07-14 09:01:00.000000", "continue", 2),
        &[
            ("message_id", Value::from(42.0)),
            ("message_event_index", Value::from(0.0)),
        ],
    );

    // When
    let merged = merge_bundles(&bundle(vec![first]), &bundle(vec![second]), EXTRACTED_AT);

    // Then
    assert_eq!(merged.events.len(), 2);
    let ids: Vec<Option<&Value>> = merged.events.iter().map(|e| meta_of(e, "message_id")).collect();
    assert_eq!(ids, vec![Some(&Value::Number(41.0)), Some(&Value::Number(42.0))]);
}

#[test]
fn events_evicted_from_the_live_ring_buffer_are_recovered_from_the_stored_bundle() {
    // Given — specify captured at close with 3 prompts; since then 2 were
    // evicted from the live ring buffer, so the fresh read only has 1.
    let stored = bundle(vec![
        event("c1", "specify", "2026-07-14 09:00:00.000000", "prompt one", 1),
        event("c1", "specify", "2026-07-14 09:10:00.000000", "prompt two", 2),
        event("c1", "specify", "2026-07-14 09:20:00.000000", "prompt three", 3),
    ]);
    let fresh = bundle(vec![event(
        "c1",
        "specify",
        "2026-07-14 09:20:00.000000",
        "prompt three",
        1,
    )]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then — all 3 prompts present: the fresh one + the 2 evicted recovered
    assert_eq!(merged.events.len(), 3);
    assert_eq!(
        field(&merged.events, "content"),
        vec!["prompt one", "prompt two", "prompt three"]
    );
}

#[test]
fn merged_events_out_of_insertion_order_are_sorted_by_ts_with_monotonic_seq() {
    // Given — stored specify (later ts) + fresh implement (earlier ts)
    let stored = bundle(vec![event(
        "c1",
        "specify",
        "2026-07-14 11:00:00.000000",
        "late specify",
        1,
    )]);
    let fresh = bundle(vec![event(
        "c2",
        "implement",
        "2026-07-14 10:00:00.000000",
        "early implement",
        1,
    )]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then — time-ordered, monotonic seq
    let ts = field(&merged.events, "ts");
    let mut sorted = ts.clone();
    sorted.sort();
    assert_eq!(sorted, ts);
    let seq: Vec<f64> = merged
        .events
        .iter()
        .map(|e| e.get("seq").and_then(Value::as_f64).expect("seq"))
        .collect();
    assert_eq!(seq, vec![1.0, 2.0]);
}

#[test]
fn spec_split_across_stored_and_fresh_tallies_all_three_labels() {
    // Given — specify + implement stored (captured at close); gate just closed
    // fresh. All three labels now present in the merged set.
    let stored = bundle(vec![
        event("c1", "specify", "2026-07-14 09:00:00.000000", "spec", 1),
        event("c2", "implement", "2026-07-14 10:00:00.000000", "impl", 2),
    ]);
    let fresh = bundle(vec![event("c3", "review", "2026-07-14 11:00:00.000000", "gate", 1)]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then
    assert_eq!(
        merged.header.get("labels"),
        Some(&strings(&["specify", "implement", "review"]))
    );
    assert_eq!(
        merged.header.get("conversation_ids"),
        Some(&strings(&["c1", "c2", "c3"]))
    );
}

#[test]
fn empty_stored_and_non_empty_fresh_yields_the_fresh_events_reseqd() {
    // Given
    let stored = bundle(vec![]);
    let fresh = bundle(vec![event("c1", "specify", "2026-07-14 09:00:00.000000", "spec", 5)]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then — fresh events kept, seq reassigned from 1
    assert_eq!(merged.events.len(), 1);
    assert_eq!(merged.events[0].get_str("content"), Some("spec"));
    assert_eq!(merged.events[0].get("seq"), Some(&Value::Number(1.0)));
}

#[test]
fn non_empty_stored_and_empty_fresh_preserves_the_stored_events_reseqd() {
    // Given — fresh read bound nothing (e.g. all decayed), stored has the goods
    let stored = bundle(vec![event("c1", "specify", "2026-07-14 09:00:00.000000", "spec", 7)]);
    let fresh = bundle(vec![]);

    // When
    let merged = merge_bundles(&stored, &fresh, EXTRACTED_AT);

    // Then — stored events survive, seq reassigned from 1
    assert_eq!(merged.events.len(), 1);
    assert_eq!(merged.events[0].get_str("content"), Some("spec"));
    assert_eq!(merged.events[0].get("seq"), Some(&Value::Number(1.0)));
}

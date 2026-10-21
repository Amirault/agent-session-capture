//! Port of `mergeEventLists.test.ts`.

use crate::domain::merge_bundle::merge_event_lists;
use crate::domain::models::EventRecord;
use crate::js::{Object, Value};

fn meta(entries: &[(&str, Value)]) -> Object {
    let mut object = Object::new();
    for (key, value) in entries {
        object.insert(*key, value.clone());
    }
    object
}

/// A label-free session event (`kind`/`role` default to a user query).
fn session_event_with(ts: &str, content: &str, meta: Object, kind: &str, role: &str) -> EventRecord {
    let mut event = Object::new();
    event.insert("conversation_id", "c1");
    event.insert("seq", 1.0);
    event.insert("ts", ts);
    event.insert("role", role);
    event.insert("kind", kind);
    event.insert("content", content);
    event.insert("meta", meta);
    event
}

fn session_event(ts: &str, content: &str, meta: Object) -> EventRecord {
    session_event_with(ts, content, meta, "query", "user")
}

fn agent_message(ts: &str, content: &str, meta: Object) -> EventRecord {
    session_event_with(ts, content, meta, "agent_message", "assistant")
}

fn command(ts: &str, content: &str) -> EventRecord {
    session_event_with(ts, content, Object::new(), "command", "user")
}

fn strs<'a>(events: &'a [EventRecord], key: &str) -> Vec<&'a str> {
    events.iter().map(|e| e.get_str(key).expect("string field")).collect()
}

fn seqs(events: &[EventRecord]) -> Vec<f64> {
    events
        .iter()
        .map(|e| e.get("seq").and_then(Value::as_f64).expect("seq"))
        .collect()
}

// --- §9.20 mergeEventLists — decay-safe merge over label-free session events ---

#[test]
fn identical_stored_and_fresh_events_dedup_into_one_without_label() {
    // Given
    let m = || meta(&[("cwd", Value::from("/repo")), ("model", Value::from("m"))]);
    let stored = vec![session_event("2026-06-30 10:10:00.000000", "continue", m())];
    let fresh = vec![session_event("2026-06-30 10:10:00.000000", "continue", m())];

    // When
    let merged = merge_event_lists(&stored, &fresh);

    // Then
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].get_str("content"), Some("continue"));
    assert!(!merged[0].contains_key("label")); // session events stay label-free
}

#[test]
fn same_query_content_at_two_stable_timestamps_keeps_both() {
    // Given
    let stored = vec![session_event("2026-06-30 10:10:00.000000", "continue", Object::new())];
    let fresh = vec![
        session_event("2026-06-30 10:10:00.000000", "continue", Object::new()),
        session_event("2026-06-30 10:30:00.000000", "continue", Object::new()),
    ];

    // When
    let merged = merge_event_lists(&stored, &fresh);

    // Then
    assert_eq!(merged.len(), 2);
    assert_eq!(
        strs(&merged, "ts"),
        vec!["2026-06-30 10:10:00.000000", "2026-06-30 10:30:00.000000"]
    );
}

#[test]
fn task_event_reobserved_later_keeps_earliest_ts_and_freshest_meta() {
    // Given — agent_tasks.last_modified_at is mutable: re-running bumps the ts
    let stored = vec![agent_message(
        "2026-06-30 10:30:00.000000",
        "task message",
        meta(&[("task_id", Value::from("t1"))]),
    )];
    let fresh = vec![agent_message(
        "2026-06-30 10:45:00.000000",
        "task message",
        meta(&[("task_id", Value::from("t1"))]),
    )];

    // When
    let merged = merge_event_lists(&stored, &fresh);

    // Then
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].get_str("ts"), Some("2026-06-30 10:30:00.000000")); // earliest observed timestamp
    assert_eq!(
        merged[0].get("meta"),
        Some(&Value::Object(meta(&[("task_id", Value::from("t1"))])))
    ); // freshest meta
}

#[test]
fn duplicate_task_events_differing_only_in_volatile_meta_dedup_into_one() {
    // Given
    let stored = vec![agent_message(
        "2026-06-30 10:30:00.000000",
        "streaming message",
        meta(&[
            ("field_path", Value::from("messages.1.text")),
            ("repeat", Value::from(1.0)),
        ]),
    )];
    let fresh_meta = || {
        meta(&[
            ("field_path", Value::from("messages.1.text")),
            ("repeat", Value::from(6.0)),
            ("merged_count", Value::from(2.0)),
        ])
    };
    let fresh = vec![agent_message(
        "2026-06-30 10:30:00.000000",
        "streaming message",
        fresh_meta(),
    )];

    // When
    let merged = merge_event_lists(&stored, &fresh);

    // Then — one event; fresh meta wins (merged_count/repeat are informational, not identity)
    assert_eq!(merged.len(), 1);
    assert_eq!(merged[0].get("meta"), Some(&Value::Object(fresh_meta())));
}

#[test]
fn stored_richer_than_fresh_keeps_stored_only_events_resorted_and_reseqenced() {
    // Given — fresh read lost the first two events (evicted from the store)
    let stored = vec![
        session_event("2026-06-30 10:10:00.000000", "q1", Object::new()),
        command("2026-06-30 10:20:00.000000", "npm test"),
        agent_message(
            "2026-06-30 10:30:00.000000",
            "task message",
            meta(&[("task_id", Value::from("t1"))]),
        ),
    ];
    let fresh = vec![agent_message(
        "2026-06-30 10:30:00.000000",
        "task message",
        meta(&[("task_id", Value::from("t1"))]),
    )];

    // When
    let merged = merge_event_lists(&stored, &fresh);

    // Then — evicted events recovered; chronological order; seq 1..N
    assert_eq!(strs(&merged, "content"), vec!["q1", "npm test", "task message"]);
    assert_eq!(seqs(&merged), vec![1.0, 2.0, 3.0]);
}

#[test]
fn interleaving_unsorted_inputs_produce_chronological_output() {
    // Given
    let stored = vec![
        agent_message("2026-06-30 10:30:00.000000", "c", Object::new()),
        session_event("2026-06-30 10:10:00.000000", "a", Object::new()),
    ];
    let fresh = vec![
        command("2026-06-30 10:20:00.000000", "b"),
        session_event("2026-06-30 10:10:00.000000", "a", Object::new()),
        agent_message("2026-06-30 10:30:00.000000", "c", Object::new()),
    ];

    // When
    let merged = merge_event_lists(&stored, &fresh);

    // Then
    assert_eq!(strs(&merged, "content"), vec!["a", "b", "c"]);
    assert_eq!(seqs(&merged), vec![1.0, 2.0, 3.0]);
}

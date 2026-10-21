//! Port of `jsonlSink.test.ts`.

use super::fixtures::TempDir;
use crate::adapters::jsonl_sink::JsonlSink;
use crate::domain::models::{CaptureBundle, EventRecord};
use crate::js::{Object, Value, json, path};

fn header(capture_id: &str) -> Object {
    let mut per_label = Object::new();
    per_label.insert("specify", 1.0);
    per_label.insert("implement", 1.0);
    per_label.insert("review", 1.0);
    let mut header = Object::new();
    header.insert("type", "bundle_header");
    header.insert("capture_id", capture_id);
    header.insert(
        "labels",
        vec![Value::from("specify"), Value::from("implement"), Value::from("review")],
    );
    header.insert("conversations_per_label", per_label);
    header.insert(
        "conversation_ids",
        vec![Value::from("c1"), Value::from("c2"), Value::from("c3")],
    );
    header.insert("extracted_at", "2026-06-30T12:00:00.000Z");
    header.insert("source", "warp");
    header
}

fn event(seq: usize, content: &str) -> EventRecord {
    let mut meta = Object::new();
    meta.insert("field_path", "1");
    meta.insert("note", "has \"quotes\" and \n newline");
    let mut event = Object::new();
    event.insert("capture_id", "add-feature-x");
    event.insert("label", "specify");
    event.insert("conversation_id", "c1");
    event.insert("seq", seq);
    event.insert("ts", format!("2026-06-30 10:0{seq}:00.000000"));
    event.insert("role", "assistant");
    event.insert("kind", "agent_message");
    event.insert("content", content);
    event.insert("meta", meta);
    event
}

// --- §9.9 jsonlSink JSONL validity ---

#[test]
fn values_with_newlines_and_quotes_write_valid_lines_that_round_trip() {
    // Given
    let tmp = TempDir::new("wa-99-");
    let out_dir = tmp.join("out");
    let events = vec![
        event(1, "line one\nwith newline and \"quotes\""),
        event(2, "plain content"),
        event(3, "multi\nline\ncontent"),
    ];
    let bundle = CaptureBundle {
        header: header("add-feature-x"),
        events: events.clone(),
    };

    // When
    let sink = JsonlSink::new(&out_dir);
    let out_path = sink.write(&bundle).expect("write");
    assert_eq!(out_path, path::join(&[&out_dir, "add-feature-x.jsonl"]));

    // Then — read back the raw file and split into physical lines
    let raw = std::fs::read_to_string(&out_path).expect("read");
    let lines: Vec<&str> = raw.split('\n').collect();
    // trailing newline => last element is ""
    assert_eq!(lines.last(), Some(&""));
    let json_lines = &lines[..lines.len() - 1];

    // every line is valid JSON (a raw newline in a value would break this)
    let parsed: Vec<Object> = json_lines
        .iter()
        .map(|l| json::parse(l).expect("valid JSON").as_object().expect("object").clone())
        .collect();

    // line 1 is the bundle_header
    assert_eq!(parsed[0].get_str("type"), Some("bundle_header"));
    assert_eq!(parsed[0].get_str("capture_id"), Some("add-feature-x"));

    // exactly 1 header + N event lines
    assert_eq!(json_lines.len(), 1 + events.len());

    // no raw newline inside any value: each serialized line contains no literal \n
    for line in json_lines {
        assert!(!line.contains('\n'));
    }

    // round-trip: header + events reconstructed
    let round_tripped = CaptureBundle {
        header: parsed[0].clone(),
        events: parsed[1..].to_vec(),
    };
    assert_eq!(round_tripped, bundle);
}

#[test]
fn atomic_write_leaves_no_temp_file_in_the_output_directory() {
    // Given
    let tmp = TempDir::new("wa-99-");
    let out_dir = tmp.join("out");
    let bundle = CaptureBundle {
        header: header("add-feature-x"),
        events: vec![event(1, "plain")],
    };

    // When
    JsonlSink::new(&out_dir).write(&bundle).expect("write");

    // Then — only the target .jsonl exists; no leftover .tmp staging files
    let files: Vec<String> = std::fs::read_dir(&out_dir)
        .expect("read dir")
        .map(|e| e.expect("entry").file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(files, vec!["add-feature-x.jsonl".to_string()]);
    assert!(files.iter().all(|f| !f.ends_with(".tmp")));
}

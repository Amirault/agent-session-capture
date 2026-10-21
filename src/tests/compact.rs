//! §9.11 compact — task-node noise reduction.

use crate::adapters::compact::compact_nodes;
use crate::adapters::protobuf_walk::WalkNode;

fn n(field_path: &str, value: &str) -> WalkNode {
    WalkNode::new(field_path, value)
}

fn values(nodes: &[WalkNode]) -> Vec<&str> {
    nodes.iter().map(|x| x.value.as_str()).collect()
}

fn paths(nodes: &[WalkNode]) -> Vec<&str> {
    nodes.iter().map(|x| x.field_path.as_str()).collect()
}

#[test]
fn base64_artifacts_are_dropped_while_real_text_and_short_tokens_are_kept() {
    // Given — a 20-char non-decoding base64 residue, real text, and a 4-char base64-looking token
    let nodes = vec![
        n("5.7", "KgwKBAgCEAQKBAgEEAg="),
        n("2", "Implement Spec Using Manual Skill"),
        n("5.7.1", "IgA="),
    ];

    // When
    let out = compact_nodes(nodes);

    // Then — real text and the short token survive; the artifact is excluded
    let values = values(&out);
    assert!(values.contains(&"Implement Spec Using Manual Skill"));
    assert!(values.contains(&"IgA="));
    assert!(!values.contains(&"KgwKBAgCEAQKBAgEEAg="));
}

#[test]
fn pure_uuids_are_dropped_and_non_uuid_text_is_kept() {
    // Given — a UUID, real text, and a non-UUID identifier-looking string
    let nodes = vec![
        n("1", "f730cfb4-ce86-4069-b469-743804a94fe4"),
        n("2", "Implement Spec Using Manual Skill"),
        n("9", "not-a-uuid"),
    ];

    // When
    let out = compact_nodes(nodes);

    // Then — only the non-UUID text remains, in order
    assert_eq!(values(&out), vec!["Implement Spec Using Manual Skill", "not-a-uuid"]);
}

#[test]
fn duplicates_collapse_onto_first_survivor_with_repeat_count_in_first_occurrence_order() {
    // Given — Acme x3 on X.1, plus two distinct X.2 values
    let nodes = vec![
        n("X.1", "Acme"),
        n("X.1", "Acme"),
        n("X.2", "agent-add-mcp"),
        n("X.1", "Acme"),
        n("X.2", "different-action"),
    ];

    // When
    let out = compact_nodes(nodes);

    // Then — three survivors in first-occurrence order; repeat only on the duplicated one
    assert_eq!(values(&out), vec!["Acme", "agent-add-mcp", "different-action"]);
    assert_eq!(paths(&out), vec!["X.1", "X.2", "X.2"]);
    assert_eq!(out[0].repeat, Some(3));
    assert_eq!(out[1].repeat, None);
    assert_eq!(out[2].repeat, None);
}

#[test]
fn long_value_is_head_tail_truncated_with_marker_and_short_value_is_intact() {
    // Given — a 2013-char value and a short value
    let long = format!("{}MIDDLE{}ENDTAIL", "A".repeat(1000), "B".repeat(1000));
    let nodes = vec![n("X.3", &long), n("X.4", "short")];

    // When
    let out = compact_nodes(nodes);

    // Then — long survivor is truncated and flagged; short survivor is untouched
    let big = out.iter().find(|x| x.field_path == "X.3").expect("X.3");
    assert!(big.truncated);
    assert_eq!(big.original_len, Some(long.len()));
    assert!(big.value.starts_with(&"A".repeat(1000)));
    assert!(big.value.ends_with("ENDTAIL"));
    assert!(big.value.contains("[truncated"));
    let small = out.iter().find(|x| x.field_path == "X.4").expect("X.4");
    assert!(!small.truncated);
    assert_eq!(small.value, "short");
}

#[test]
fn base64_and_truncate_length_boundaries_drop_16_chars_and_keep_2000_chars_intact() {
    // Given — exactly 16 base64-charset chars (drop boundary) and exactly 2000 chars (truncate boundary, not > threshold)
    let at_base64_min = "A".repeat(16);
    let at_truncate_max = format!("{} ", "x".repeat(1999)); // space => not base64; length 2000 => not > 2000
    let nodes = vec![n("B.1", &at_base64_min), n("B.2", &at_truncate_max)];

    // When
    let out = compact_nodes(nodes);

    // Then — only the 2000-char value survives, untruncated, at full length
    assert_eq!(paths(&out), vec!["B.2"]);
    assert!(!out[0].truncated);
    assert_eq!(out[0].value.len(), 2000);
}

#[test]
fn realistic_mix_keeps_only_real_signal_with_repeat_counts_and_truncation() {
    // Given — UUIDs, base64 residue, duplicated siblings, and a long realistic message
    let uuid = "f730cfb4-ce86-4069-b469-743804a94fe4";
    let long = "DETAIL: reviewing the implementation step by step in detail. ".repeat(60);
    let nodes = vec![
        n("1", uuid),
        n("2", "Implement Spec Using Manual Skill"),
        n("5.1", "bed38ec6-f50c-4826-abfc-83967bd6a1fe"),
        n("5.11", uuid),
        n("5.7", "KgwKBAgCEAQKBAgEEAg="),
        n("5.7", "KgwKBAgCEAQKBAgEEAg="),
        n("S.1", "Acme"),
        n("S.1", "Acme"),
        n("S.2", "agent-add-mcp"),
        n("S.2", "agent-add-mcp"),
        n("S.3", &long),
    ];

    // When
    let out = compact_nodes(nodes);

    // Then — four survivors: real text, two duplicated siblings (repeat:2), one truncated long
    assert_eq!(paths(&out), vec!["2", "S.1", "S.2", "S.3"]);
    assert_eq!(out[0].value, "Implement Spec Using Manual Skill");
    assert_eq!(out[1].value, "Acme");
    assert_eq!(out[1].repeat, Some(2));
    assert_eq!(out[2].value, "agent-add-mcp");
    assert_eq!(out[2].repeat, Some(2));
    assert!(out[3].truncated);
    assert_eq!(out[3].original_len, Some(long.len()));
    assert!(out[3].value.starts_with("DETAIL"));
}

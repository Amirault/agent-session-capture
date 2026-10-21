//! §9.6 protobufWalk.

use super::fixtures::protobuf::{encode_message, encode_string, encode_tag, encode_varint};
use crate::adapters::protobuf_walk::walk_protobuf;

/// Standard base64 (with padding), as `Buffer.toString("base64")` writes it.
fn to_base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[test]
fn task_blob_strings_are_emitted_in_document_order_with_field_paths() {
    // Given — field 1 = title, field 2 = nested {field 1 = tool call}, field 3 = output
    let blob = [
        encode_string(1, "Task title"),
        encode_message(2, &encode_string(1, "tool call")),
        encode_string(3, "command output"),
    ]
    .concat();

    // When
    let walked = walk_protobuf(&blob).expect("walk");

    // Then
    assert!(walked.complete);
    let values: Vec<&str> = walked.nodes.iter().map(|n| n.value.as_str()).collect();
    assert_eq!(values, vec!["Task title", "tool call", "command output"]);
    let paths: Vec<&str> = walked.nodes.iter().map(|n| n.field_path.as_str()).collect();
    assert_eq!(paths, vec!["1", "2.1", "3"]);
}

#[test]
fn base64_wrapped_protobuf_leaf_is_decoded_and_recursed() {
    // Given — an inner message {field 1 = "inner secret"}, base64-encoded, stored as field 5
    let inner = encode_string(1, "inner secret");
    let b64 = to_base64(&inner);
    let blob = encode_string(5, &b64);

    // When
    let walked = walk_protobuf(&blob).expect("walk");

    // Then — the inner text is recovered with a path that descends into the decoded leaf
    let recovered = walked
        .nodes
        .iter()
        .find(|n| n.value == "inner secret")
        .expect("recovered");
    assert_eq!(recovered.field_path, "5.1");
    // the raw base64 blob is not emitted as a string in its place
    assert!(!walked.nodes.iter().any(|n| n.value == b64));
}

#[test]
fn malformed_blob_returns_partial_results_marked_incomplete() {
    // Given — a good string field, then a length-delimited field whose declared length exceeds the buffer
    let blob = [
        encode_string(1, "recovered text"),
        encode_tag(2, 2),
        encode_varint(100),
        b"abc".to_vec(),
    ]
    .concat();

    // When
    let walked = walk_protobuf(&blob).expect("walk");

    // Then — recovered without throwing, partial node present, walk flagged incomplete
    assert!(!walked.complete);
    assert!(walked.nodes.iter().any(|n| n.value == "recovered text"));
}

#[test]
fn raw_mode_retains_uuids_and_tokens_without_filtering() {
    // Given — strings that look like identifiers/secrets
    let uuid = "550e8400-e29b-41d4-a716-446655440000";
    let token = "sk-ant-api03-xxxxxxxxxxxxxxxx";
    let blob = [encode_string(1, uuid), encode_string(2, token)].concat();

    // When
    let walked = walk_protobuf(&blob).expect("walk");

    // Then — both retained verbatim
    let values: Vec<&str> = walked.nodes.iter().map(|n| n.value.as_str()).collect();
    assert_eq!(values, vec![uuid, token]);
}

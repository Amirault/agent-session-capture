//! Noise-reduction pass over walked protobuf nodes.
//!
//! Drops non-signal strings (base64 residue the walker could not unfold, pure
//! UUIDs), dedupes repeated (field_path, value) pairs with a repeat count, and
//! truncates long values — preserving first-occurrence order. Dedupe runs
//! before truncation so duplicate long values collapse on their original form.
//! Lengths and slices are in UTF-16 code units, as in the original.

use std::collections::HashMap;

use super::protobuf_walk::{WalkNode, looks_like_base64};
use crate::js::string::{utf16_len, utf16_slice};

/// Drop base64-looking strings shorter than this (they are usually real tokens).
const BASE64_MIN_LEN: usize = 16;
/// Truncate values longer than this (head + marker + tail keeps both ends).
const TRUNCATE_THRESHOLD: usize = 2000;
const TRUNCATE_HEAD: usize = 1000;
const TRUNCATE_TAIL: usize = 500;

pub fn compact_nodes(nodes: Vec<WalkNode>) -> Vec<WalkNode> {
    let mut seen: HashMap<(String, String), usize> = HashMap::new();
    let mut survivors: Vec<WalkNode> = Vec::new();

    for node in nodes {
        if looks_like_base64(&node.value) && node.value.len() >= BASE64_MIN_LEN {
            continue;
        }
        if is_pure_uuid(&node.value) {
            continue;
        }
        let key = (node.field_path.clone(), node.value.clone());
        if let Some(&i) = seen.get(&key) {
            let existing = &mut survivors[i];
            existing.repeat = Some(existing.repeat.unwrap_or(1) + 1);
            continue;
        }
        seen.insert(key, survivors.len());
        survivors.push(node);
    }

    for node in &mut survivors {
        let length = utf16_len(&node.value);
        if length > TRUNCATE_THRESHOLD {
            let head = utf16_slice(&node.value, 0, TRUNCATE_HEAD);
            let tail = utf16_slice(&node.value, length - TRUNCATE_TAIL, length);
            node.value = format!("{head}…[truncated len={length}]…{tail}");
            node.truncated = true;
            node.original_len = Some(length);
        }
    }
    survivors
}

/// True for the canonical 8-4-4-4-12 hex UUID form.
fn is_pure_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 36 {
        return false;
    }
    b.iter().enumerate().all(|(i, &c)| {
        if matches!(i, 8 | 13 | 18 | 23) {
            c == b'-'
        } else {
            c.is_ascii_hexdigit()
        }
    })
}

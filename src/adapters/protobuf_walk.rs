//! Generic, schema-less protobuf wire-format walker.
//!
//! `agent_tasks.task` is a protobuf BLOB. Readable strings (task title, tool
//! calls, commands, outputs, file paths) are recovered by walking the wire
//! format: each field is a (field_number, wire_type) tag + value.
//!
//! Length-delimited fields (wire type 2) are ambiguous (string | bytes |
//! nested message). Disambiguation, mirroring `protoc --decode_raw`:
//!   1. Try parsing as a nested message. If it parses cleanly AND yields
//!      strings, recurse (field paths accumulate, e.g. "2.1").
//!   2. Else, if it is valid UTF-8 and mostly printable: it is a string. If it
//!      also looks like base64 that decodes to a valid protobuf, recurse on the
//!      decoded bytes instead.
//!   3. Else (binary, not a message) -> skip.
//!
//! Malformed/truncated input never fails: the walk stops and returns whatever
//! it recovered with `complete: false` (callers flag confidence "heuristic").

use crate::js::buffer::base64_decode;
use crate::js::spread_push;
use crate::js::string::utf16_len;

/// One recovered string.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct WalkNode {
    /// Dotted field-number path from the root to this string (e.g. "2.1").
    pub field_path: String,
    pub value: String,
    /// Times this (field_path, value) occurred (set by compaction when > 1).
    pub repeat: Option<u32>,
    /// True when `value` was truncated (set by compaction).
    pub truncated: bool,
    /// Original value length before truncation (set by compaction).
    pub original_len: Option<usize>,
}

impl WalkNode {
    pub fn new(field_path: &str, value: &str) -> Self {
        Self {
            field_path: field_path.to_string(),
            value: value.to_string(),
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WalkResult {
    pub nodes: Vec<WalkNode>,
    /// false if the walk hit malformed/truncated bytes (partial recovery).
    pub complete: bool,
}

const MAX_DEPTH: usize = 12;

const PRINTABLE_RATIO_THRESHOLD: f64 = 0.75;

/// A varint as a double (as the original computed it), and its byte length.
fn read_varint(buf: &[u8], pos: usize) -> Option<(f64, usize)> {
    let mut value = 0.0f64;
    let mut multiplier = 1.0f64;
    let mut p = pos;
    for _ in 0..10 {
        let byte = *buf.get(p)?;
        p += 1;
        value += f64::from(byte & 0x7f) * multiplier;
        if byte & 0x80 == 0 {
            return Some((value, p - pos));
        }
        multiplier *= 128.0;
    }
    None
}

/// ECMAScript ToUint32 of a non-negative integral double.
fn to_uint32(value: f64) -> u32 {
    (value % 4_294_967_296.0) as u32
}

fn is_printable_code(c: u32) -> bool {
    (c >= 0x20 && c != 0x7f) || c == 0x09 || c == 0x0a || c == 0x0d
}

fn printable_ratio(s: &str) -> f64 {
    let length = utf16_len(s);
    if length == 0 {
        return 1.0;
    }
    let printable = s.chars().filter(|c| is_printable_code(*c as u32)).count();
    printable as f64 / length as f64
}

/// Only base64 alphabet characters (and `=`), length a positive multiple of 4.
pub fn looks_like_base64(s: &str) -> bool {
    if s.len() < 4 || s.len() % 4 != 0 {
        return false;
    }
    s.bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b'+' || c == b'/' || c == b'=')
}

/// Walk a task blob; `Err` only for the original's argument-spread overflow.
pub fn walk_protobuf(buf: &[u8]) -> Result<WalkResult, String> {
    walk(buf, "", 0)
}

fn walk(buf: &[u8], prefix: &str, depth: usize) -> Result<WalkResult, String> {
    let mut nodes = Vec::new();
    let mut pos = 0;
    let mut complete = true;

    while pos < buf.len() {
        let Some((tag, tag_bytes)) = read_varint(buf, pos) else {
            complete = false;
            break;
        };
        pos += tag_bytes;
        let field_number = to_uint32(tag) >> 3;
        let wire_type = to_uint32(tag) & 7;
        let field_path = if prefix.is_empty() {
            field_number.to_string()
        } else {
            format!("{prefix}.{field_number}")
        };

        match wire_type {
            0 => {
                let Some((_, n)) = read_varint(buf, pos) else {
                    complete = false;
                    break;
                };
                pos += n;
            }
            1 | 5 => {
                let size = if wire_type == 1 { 8 } else { 4 };
                if pos + size > buf.len() {
                    complete = false;
                    break;
                }
                pos += size;
            }
            2 => {
                let Some((len, n)) = read_varint(buf, pos) else {
                    complete = false;
                    break;
                };
                pos += n;
                if len > (buf.len() - pos) as f64 {
                    complete = false;
                    break;
                }
                let len = len as usize;
                let data = &buf[pos..pos + len];
                pos += len;
                // The original appended with `nodes.push(...inner)`, which V8
                // rejects for a huge `inner`: the task is then skipped. Real task
                // blobs stay far below the bound (≤ 25k nodes) or far above it.
                spread_push(&mut nodes, interpret_length_delimited(data, &field_path, depth)?.nodes)?;
            }
            _ => {
                // Group / unknown wire types (3, 4, 6, 7) -> stop.
                complete = false;
                break;
            }
        }
    }
    Ok(WalkResult { nodes, complete })
}

fn interpret_length_delimited(data: &[u8], field_path: &str, depth: usize) -> Result<WalkResult, String> {
    if data.is_empty() || depth >= MAX_DEPTH {
        return Ok(WalkResult {
            nodes: vec![],
            complete: true,
        });
    }
    // 1. A real sub-message parses cleanly and yields strings.
    let sub = walk(data, field_path, depth + 1)?;
    if sub.complete && !sub.nodes.is_empty() {
        return Ok(WalkResult {
            nodes: sub.nodes,
            complete: true,
        });
    }
    // 2. Valid UTF-8 and mostly printable -> string.
    if let Ok(text) = std::str::from_utf8(data)
        && !text.is_empty()
        && printable_ratio(text) >= PRINTABLE_RATIO_THRESHOLD
    {
        // 2a. base64-wrapped protobuf leaf?
        if text.len() >= 8 && looks_like_base64(text) {
            let decoded = base64_decode(text);
            if !decoded.is_empty() {
                let inner = walk(&decoded, field_path, depth + 1)?;
                if inner.complete && !inner.nodes.is_empty() {
                    return Ok(WalkResult {
                        nodes: inner.nodes,
                        complete: true,
                    });
                }
            }
        }
        // 2b. plain string.
        return Ok(WalkResult {
            nodes: vec![WalkNode::new(field_path, text)],
            complete: true,
        });
    }
    // 3. Binary, not a message.
    Ok(WalkResult {
        nodes: vec![],
        complete: true,
    })
}

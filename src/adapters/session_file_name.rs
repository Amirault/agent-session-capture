//! Conversation id → `sessions/<file>.jsonl` basename.

use crate::js::buffer::sha1_hex;

fn is_safe_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-'
}

/// Map a conversation id to its session file basename.
///
/// Pass-through when the id is already a safe single path segment
/// (`[A-Za-z0-9._-]+`, not `.` / `..`). Otherwise sanitize disallowed
/// characters (each UTF-16 unit) to `-` and append `-<first 8 hex of
/// sha1(cid)>`: deterministic and collision-proof, and never a path that can
/// escape the sessions/ directory.
pub fn session_file_name(conversation_id: &str) -> String {
    if !conversation_id.is_empty()
        && conversation_id.chars().all(is_safe_char)
        && conversation_id != "."
        && conversation_id != ".."
    {
        return conversation_id.to_string();
    }
    let sanitized: String = conversation_id
        .chars()
        .flat_map(|c| {
            let replacement = if is_safe_char(c) {
                c.to_string()
            } else {
                "-".repeat(c.len_utf16())
            };
            replacement.chars().collect::<Vec<_>>()
        })
        .collect();
    let digest = sha1_hex(conversation_id.as_bytes());
    format!("{sanitized}-{}", &digest[..8])
}

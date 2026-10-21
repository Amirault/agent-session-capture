//! The exact `: CAPTURE_MARKER v=1 id=<id> [label=<label>]` line parser
//! (Hermes, Langfuse).

use super::models::{DEFAULT_LABEL, Label};
use crate::js::string::js_trim;

/// One canonical marker emission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExactMarker {
    pub capture_id: String,
    pub label: Label,
}

/// The exact markers a shell command ran: every line of `command` that is, once
/// trimmed, the canonical marker line and nothing else. A marker quoted inside
/// another command (`echo ': CAPTURE_MARKER …'`) is a mention, not an emission.
pub fn exact_markers_in_command(command: &str) -> Vec<ExactMarker> {
    command
        .split('\n')
        .filter_map(|line| parse_exact_marker_line(js_trim(line)))
        .collect()
}

fn parse_exact_marker_line(line: &str) -> Option<ExactMarker> {
    let tokens: Vec<&str> = line.split(' ').collect();
    if (tokens.len() != 4 && tokens.len() != 5)
        || tokens[0] != ":"
        || tokens[1] != "CAPTURE_MARKER"
        || tokens[2] != "v=1"
        || !tokens[3].starts_with("id=")
        || (tokens.len() == 5 && !tokens[4].starts_with("label="))
    {
        return None;
    }
    let capture_id = &tokens[3]["id=".len()..];
    let label = if tokens.len() == 5 {
        &tokens[4]["label=".len()..]
    } else {
        DEFAULT_LABEL
    };
    if capture_id.is_empty() || label.is_empty() {
        return None;
    }
    Some(ExactMarker {
        capture_id: capture_id.to_string(),
        label: label.to_string(),
    })
}

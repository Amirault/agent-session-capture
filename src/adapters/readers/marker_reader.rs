//! Marker selection & binding (Warp).
//!
//! The correlation marker `: CAPTURE_MARKER v=1 id=<id> [label=<label>]` is a
//! shell no-op recorded cleanly in `commands.command`, and creates a `blocks`
//! row at the SAME `start_ts` carrying `ai_metadata.conversation_id`. The
//! marker binds to its conversation by joining on `start_ts` (verified 1:1).

use crate::adapters::sqlite_readable_db::{Cell, ReadableDb};
use crate::domain::models::{DEFAULT_LABEL, Label, SeedConfidence, SeedMatch, SeedStatus};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::source_errors::ReadError;
use crate::js::string::js_trim;

/// A parsed marker command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedMarker {
    pub capture_id: String,
    pub label: Label,
}

/// Parse a marker command into its capture id + label, or `None`.
///
/// Only the first line counts (Warp records the marker and the real command
/// that follows as one multi-line value). A marker chained on that line
/// (`cd app; : CAPTURE_MARKER ...`, `build.sh && : CAPTURE_MARKER ...`) still
/// ran, so any unquoted `;` / `&&` segment is accepted.
pub fn parse_marker(command: &str) -> Option<ParsedMarker> {
    let first_line = command.split('\n').next().unwrap_or("");
    unquoted_segments(first_line)
        .iter()
        .find_map(|segment| parse_marker_segment(segment))
}

fn unquoted_segments(line: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
            current.push(c);
            i += 1;
            continue;
        }
        if c == '\'' || c == '"' {
            quote = Some(c);
            current.push(c);
            i += 1;
            continue;
        }
        if c == ';' || (c == '&' && chars.get(i + 1) == Some(&'&')) {
            segments.push(std::mem::take(&mut current));
            i += if c == '&' { 2 } else { 1 };
            continue;
        }
        current.push(c);
        i += 1;
    }
    segments.push(current);
    segments
}

fn parse_marker_segment(segment: &str) -> Option<ParsedMarker> {
    let tokens: Vec<&str> = js_trim(segment).split(' ').filter(|t| !t.is_empty()).collect();
    if tokens.first() != Some(&":") || tokens.get(1) != Some(&"CAPTURE_MARKER") || tokens.get(2) != Some(&"v=1") {
        return None;
    }
    let mut capture_id: Option<&str> = None;
    let mut label = DEFAULT_LABEL;
    for t in &tokens {
        if let Some(id) = t.strip_prefix("id=") {
            capture_id = Some(id);
        } else if let Some(v) = t.strip_prefix("label=")
            && !v.is_empty()
        {
            label = v;
        }
    }
    let capture_id = capture_id.filter(|id| !id.is_empty())?;
    Some(ParsedMarker {
        capture_id: capture_id.to_string(),
        label: label.to_string(),
    })
}

/// Fallback binding for local orchestrated subagents: the most recent AI
/// exchange at or before the marker, within a 10-minute look-back window.
fn fallback_via_ai_queries(db: &ReadableDb, marker_ts: &Option<String>) -> Option<String> {
    let rows = db
        .all(
            "SELECT conversation_id
               FROM ai_queries
              WHERE start_ts BETWEEN datetime(?, '-600 seconds') AND ?
              ORDER BY start_ts DESC
              LIMIT 1",
            &[marker_ts, marker_ts],
        )
        .ok()?;
    rows.first().and_then(|r| r.get("conversation_id").text())
}

/// Find every marker emission for `capture_id`, bound to a conversation where
/// possible, in `start_ts` order. Unbindable / collision emissions are
/// included (reported in the run summary) with no conversation id.
pub fn find_seeds(db: &ReadableDb, capture_id: &str) -> Result<Vec<SeedMatch>, ReadError> {
    let rows = db.all(
        "SELECT c.command, c.start_ts,
                json_extract(b.ai_metadata, '$.conversation_id') AS cid
           FROM commands c
           LEFT JOIN blocks b ON b.start_ts = c.start_ts
          WHERE c.command LIKE ': CAPTURE_MARKER%'
          ORDER BY c.start_ts",
        &[],
    )?;

    // One emission per start_ts, in chronological order.
    let mut by_emission: OrderedMap<Option<String>, (String, Vec<String>)> = OrderedMap::new();
    for row in &rows {
        let start_ts = row.get("start_ts").text();
        let command = row.get("command").text().unwrap_or_default();
        let entry = by_emission.entry_or_insert_with(start_ts, || (command, Vec::new()));
        let cid = row.get("cid");
        if cid.truthy()
            && let Some(cid) = cid.text()
        {
            entry.1.push(cid);
        }
    }

    let mut seeds = Vec::new();
    let mut bound_conversations = std::collections::HashSet::new();
    for (start_ts, (command, cids)) in by_emission.iter() {
        let Some(parsed) = parse_marker(command) else { continue };
        if parsed.capture_id != capture_id {
            continue;
        }
        let mut distinct: Vec<&String> = Vec::new();
        for cid in cids {
            if !distinct.contains(&cid) {
                distinct.push(cid);
            }
        }
        let (status, conversation_id) = match distinct.len() {
            0 => match fallback_via_ai_queries(db, start_ts) {
                Some(cid) => (SeedStatus::Bound, Some(cid)),
                None => (SeedStatus::Unbindable, None),
            },
            1 => (SeedStatus::Bound, Some(distinct[0].clone())),
            _ => (SeedStatus::Collision, None),
        };
        if status == SeedStatus::Bound {
            let cid = conversation_id.clone().expect("bound");
            if !bound_conversations.insert(cid) {
                continue; // dedup re-emissions
            }
        }
        seeds.push(SeedMatch {
            conversation_id,
            label: parsed.label,
            marker_command: command.clone(),
            start_ts: start_ts.clone().unwrap_or_else(|| "null".to_string()),
            status,
            confidence: (status == SeedStatus::Bound && distinct.is_empty()).then_some(SeedConfidence::Heuristic),
        });
    }
    let _ = Cell::Null;
    Ok(seeds)
}

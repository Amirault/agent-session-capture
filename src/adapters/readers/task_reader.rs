//! `agent_tasks` → assistant events, by walking each protobuf `task` BLOB.

use std::collections::HashMap;

use super::{cell_display, in_list};
use crate::adapters::collapse_deltas::collapse_deltas;
use crate::adapters::compact::compact_nodes;
use crate::adapters::envelope_dedupe::{EnvelopeDedupeState, dedupe_envelope};
use crate::adapters::protobuf_walk::walk_protobuf;
use crate::adapters::schema_overlay::apply_schema_names;
use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::models::{EventDraft, EventKind, ReaderResult, Role, SkippedRow};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value};

/// Read agent_tasks for the given conversations: walk → compact → schema
/// names → collapse streaming deltas → dedupe repeated envelopes. A walk that
/// hit malformed bytes marks its events `confidence: "heuristic"`.
pub fn read_tasks(db: &ReadableDb, conversation_ids: &[String]) -> Result<ReaderResult, ReadError> {
    if conversation_ids.is_empty() {
        return Ok(ReaderResult::default());
    }
    let (placeholders, params) = in_list(conversation_ids);
    let rows = db.all(
        &format!(
            "SELECT conversation_id, task_id, task, last_modified_at
       FROM agent_tasks
      WHERE conversation_id IN ({placeholders})
      ORDER BY last_modified_at, task_id"
        ),
        &params,
    )?;

    let mut result = ReaderResult::default();
    let mut envelope_states: HashMap<String, EnvelopeDedupeState> = HashMap::new();
    for row in rows {
        let task_id = cell_display(row.get("task_id"));
        let task = row.get("task").bytes().unwrap_or_default();
        if task.is_empty() {
            result.skipped.push(SkippedRow::new(
                "agent_tasks",
                "empty task",
                format!("task_id={task_id}"),
            ));
            continue;
        }
        let conversation_id = cell_display(row.get("conversation_id"));
        let ts = cell_display(row.get("last_modified_at"));
        let walked = match walk_protobuf(&task) {
            Ok(walked) => walked,
            Err(error) => {
                result.skipped.push(SkippedRow::new(
                    "agent_tasks",
                    "walk threw",
                    format!("task_id={task_id}: {error}"),
                ));
                continue;
            }
        };
        let state = envelope_states.entry(conversation_id.clone()).or_default();
        let collapsed = collapse_deltas(&apply_schema_names(compact_nodes(walked.nodes)));
        for (task_event_index, node) in collapsed.iter().enumerate() {
            let Some(node) = dedupe_envelope(node, state) else {
                continue;
            };
            let mut meta = Object::new();
            meta.insert("field_path", node.named.node.field_path.as_str());
            meta.insert("task_id", task_id.as_str());
            meta.insert("task_event_index", task_event_index);
            if let Some(kind) = &node.named.message_kind {
                meta.insert("message_kind", kind.as_str());
            }
            if let Some(tool) = &node.named.tool {
                meta.insert("tool", tool.as_str());
            }
            if let Some(id) = &node.tool_call_id {
                meta.insert("tool_call_id", id.as_str());
            }
            if let Some(fields) = &node.fields {
                meta.insert("fields", fields.clone());
            }
            if let Some(skills) = &node.skills {
                meta.insert("skills", skills.iter().cloned().map(Value::Object).collect::<Vec<_>>());
            }
            if let Some(count) = node.merged_count {
                meta.insert("merged_count", count);
            }
            if let Some(repeat) = node.named.node.repeat {
                meta.insert("repeat", f64::from(repeat));
            }
            if node.named.node.truncated {
                meta.insert("truncated", true);
                meta.insert("original_len", node.named.node.original_len.unwrap_or(0));
            }
            if node.named.schema_mismatch {
                meta.insert("confidence", "schema-mismatch");
            }
            if !walked.complete && !meta.contains_key("confidence") {
                meta.insert("confidence", "heuristic");
            }
            result.drafts.push(EventDraft {
                conversation_id: conversation_id.clone(),
                ts: ts.clone(),
                role: Role::Assistant,
                kind: EventKind::AgentMessage,
                content: node.named.node.value.clone(),
                meta,
            });
        }
    }
    Ok(result)
}

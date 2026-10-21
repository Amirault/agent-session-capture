//! `blocks` → shell command events.

use super::{cell_display, cell_to_value, in_list};
use crate::adapters::ansi::strip_ansi;
use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::models::{EventDraft, EventKind, ReaderResult, Role, SkippedRow};
use crate::domain::schemas::parse_block_ai_metadata;
use crate::domain::source_errors::ReadError;
use crate::js::Object;

/// Read command executions for the given conversations, selected by
/// `ai_metadata.conversation_id` (subagent blocks share the parent
/// conversation_id). Styled command/output BLOBs are ANSI-stripped.
pub fn read_blocks(db: &ReadableDb, conversation_ids: &[String]) -> Result<ReaderResult, ReadError> {
    if conversation_ids.is_empty() {
        return Ok(ReaderResult::default());
    }
    let (placeholders, params) = in_list(conversation_ids);
    let rows = db.all(
        &format!(
            "SELECT b.id, b.start_ts, b.stylized_command, b.stylized_output, b.pwd, b.git_branch,
            b.git_branch_name, b.exit_code, b.did_execute, b.ai_metadata
       FROM blocks b
      WHERE json_extract(b.ai_metadata, '$.conversation_id') IN ({placeholders})
      ORDER BY b.start_ts"
        ),
        &params,
    )?;

    let mut result = ReaderResult::default();
    for row in rows {
        let ts_display = cell_display(row.get("start_ts"));
        let ai_metadata = row.get("ai_metadata").text();
        let meta = match parse_block_ai_metadata(ai_metadata.as_deref()) {
            None => {
                result.skipped.push(SkippedRow::new(
                    "blocks",
                    "ai_metadata null",
                    format!("ts={ts_display}"),
                ));
                continue;
            }
            Some(Err(error)) => {
                result.skipped.push(SkippedRow::new(
                    "blocks",
                    "ai_metadata failed schema",
                    format!("ts={ts_display}: {error}"),
                ));
                continue;
            }
            Some(Ok(meta)) => meta,
        };
        let Some(conversation_id) = meta.conversation_id.filter(|c| !c.is_empty()) else {
            result.skipped.push(SkippedRow::new(
                "blocks",
                "no conversation_id",
                format!("ts={ts_display}"),
            ));
            continue;
        };

        let mut event_meta = Object::new();
        event_meta.insert("block_id", cell_to_value(row.get("id")));
        event_meta.insert("output", strip_ansi(row.get("stylized_output").bytes().as_deref()));
        event_meta.insert("exit_code", cell_to_value(row.get("exit_code")));
        let branch_name = row.get("git_branch_name");
        let branch = if matches!(branch_name, crate::adapters::sqlite_readable_db::Cell::Null) {
            row.get("git_branch")
        } else {
            branch_name
        };
        if branch.truthy() {
            event_meta.insert("git_branch", cell_to_value(branch));
        }
        if let Some(sub) = meta.subagent_task_id.filter(|s| !s.is_empty()) {
            event_meta.insert("subagent_task_id", sub);
        }
        result.drafts.push(EventDraft {
            conversation_id,
            ts: row.get("start_ts").text().unwrap_or_default(),
            role: Role::Tool,
            kind: EventKind::Command,
            content: strip_ansi(row.get("stylized_command").bytes().as_deref()),
            meta: event_meta,
        });
    }
    Ok(result)
}

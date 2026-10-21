//! `ai_queries` → user prompt events.

use super::{cell_display, cell_to_value, in_list};
use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::models::{EventDraft, EventKind, ReaderResult, Role, SkippedRow};
use crate::domain::schemas::parse_query_input;
use crate::domain::source_errors::ReadError;
use crate::js::Object;

/// Read user prompts for the given conversations. Each row's `input` is
/// validated; rows that fail are reported and skipped. The query's git branch
/// comes from the most recent block at or before it.
pub fn read_queries(db: &ReadableDb, conversation_ids: &[String]) -> Result<ReaderResult, ReadError> {
    if conversation_ids.is_empty() {
        return Ok(ReaderResult::default());
    }
    let (placeholders, params) = in_list(conversation_ids);
    let rows = db.all(
        &format!(
            "SELECT q.conversation_id, q.exchange_id, q.start_ts, q.input,
            q.working_directory, q.model_id,
            (SELECT b.git_branch_name FROM blocks b
              WHERE json_extract(b.ai_metadata, '$.conversation_id') = q.conversation_id
                AND b.start_ts <= q.start_ts AND b.git_branch_name IS NOT NULL
              ORDER BY b.start_ts DESC LIMIT 1) AS git_branch
       FROM ai_queries q
      WHERE q.conversation_id IN ({placeholders})
      ORDER BY q.start_ts"
        ),
        &params,
    )?;

    let mut result = ReaderResult::default();
    for row in rows {
        let input = row.get("input").text().unwrap_or_else(|| "null".to_string());
        let text = match parse_query_input(&input) {
            Ok(text) => text,
            Err(error) => {
                result.skipped.push(SkippedRow::new(
                    "ai_queries",
                    "input failed schema",
                    format!(
                        "cid={} ts={}: {error}",
                        cell_display(row.get("conversation_id")),
                        cell_display(row.get("start_ts"))
                    ),
                ));
                continue;
            }
        };
        let mut meta = Object::new();
        meta.insert("cwd", cell_to_value(row.get("working_directory")));
        meta.insert("model", cell_to_value(row.get("model_id")));
        meta.insert("exchange_id", cell_to_value(row.get("exchange_id")));
        let branch = row.get("git_branch");
        if branch.truthy() {
            meta.insert("git_branch", cell_to_value(branch));
        }
        result.drafts.push(EventDraft {
            conversation_id: cell_display(row.get("conversation_id")),
            ts: cell_display(row.get("start_ts")),
            role: Role::User,
            kind: EventKind::Query,
            content: text,
            meta,
        });
    }
    Ok(result)
}

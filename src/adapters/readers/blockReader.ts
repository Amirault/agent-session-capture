import type { EventDraft, ReaderResult, SkippedRow } from "../../domain/models.js";
import type { ReadableDb } from "../../domain/ports.js";
import { parseBlockAiMetadata } from "../../domain/schemas.js";
import { stripAnsi } from "../ansi.js";

interface BlockRowRead {
  id: number;
  start_ts: string | null;
  stylized_command: Buffer | null;
  stylized_output: Buffer | null;
  pwd: string | null;
  git_branch: string | null;
  git_branch_name: string | null;
  exit_code: number | null;
  did_execute: number | null;
  ai_metadata: string | null;
}

/**
 * Read command executions (blocks) for the given conversations.
 *
 * Selection is by ai_metadata.conversation_id (subagent blocks share the parent
 * conversation_id, so they are included automatically — no expansion step).
 * stylized_command/output are ANSI-exploded BLOBs; we strip ANSI to recover clean
 * text. ai_metadata is parsed with zod; rows that fail are logged and skipped.
 */
export function readBlocks(
  db: ReadableDb,
  conversationIds: readonly string[]
): ReaderResult {
  if (conversationIds.length === 0) return { drafts: [], skipped: [] };

  const placeholders = conversationIds.map(() => "?").join(",");
  const rows = db.all<BlockRowRead>(
    `SELECT b.id, b.start_ts, b.stylized_command, b.stylized_output, b.pwd, b.git_branch,
            b.git_branch_name, b.exit_code, b.did_execute, b.ai_metadata
       FROM blocks b
      WHERE json_extract(b.ai_metadata, '$.conversation_id') IN (${placeholders})
      ORDER BY b.start_ts`,
    ...conversationIds
  );

  const drafts: EventDraft[] = [];
  const skipped: SkippedRow[] = [];
  for (const row of rows) {
    const meta = parseBlockAiMetadata(row.ai_metadata);
    if (meta === null) {
      skipped.push({ table: "blocks", reason: "ai_metadata null", detail: `ts=${row.start_ts}` });
      continue;
    }
    if (!meta.ok) {
      skipped.push({
        table: "blocks",
        reason: "ai_metadata failed schema",
        detail: `ts=${row.start_ts}: ${meta.error}`,
      });
      continue;
    }
    const conversation_id = meta.data.conversation_id;
    if (!conversation_id) {
      skipped.push({ table: "blocks", reason: "no conversation_id", detail: `ts=${row.start_ts}` });
      continue;
    }

    const eventMeta: Record<string, unknown> = {
      block_id: row.id,
      output: stripAnsi(row.stylized_output),
      exit_code: row.exit_code,
    };
    const branch = row.git_branch_name ?? row.git_branch;
    if (branch) eventMeta.git_branch = branch;
    if (meta.data.subagent_task_id) eventMeta.subagent_task_id = meta.data.subagent_task_id;

    drafts.push({
      conversation_id,
      ts: row.start_ts ?? "",
      role: "tool",
      kind: "command",
      content: stripAnsi(row.stylized_command),
      meta: eventMeta,
    });
  }
  return { drafts, skipped };
}

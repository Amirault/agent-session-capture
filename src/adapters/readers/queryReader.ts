import type { EventDraft, ReaderResult, SkippedRow } from "../../domain/models.js";
import type { ReadableDb } from "../../domain/ports.js";
import { parseQueryInput } from "../../domain/schemas.js";

interface QueryRowWithGit {
  conversation_id: string;
  exchange_id: string;
  start_ts: string;
  input: string;
  working_directory: string | null;
  model_id: string;
  git_branch: string | null;
}

/**
 * Read user prompts (ai_queries) for the given conversations.
 *
 * Each row's `input` is parsed with the zod schema; rows that fail are logged and
 * skipped — never silently dropped, never crash. The query's git_branch context is
 * derived from the most recent block at or before the query's start_ts (ai_queries
 * itself has no git_branch column).
 */
export function readQueries(
  db: ReadableDb,
  conversationIds: readonly string[]
): ReaderResult {
  if (conversationIds.length === 0) return { drafts: [], skipped: [] };

  const placeholders = conversationIds.map(() => "?").join(",");
  const rows = db.all<QueryRowWithGit>(
    `SELECT q.conversation_id, q.exchange_id, q.start_ts, q.input,
            q.working_directory, q.model_id,
            (SELECT b.git_branch_name FROM blocks b
              WHERE json_extract(b.ai_metadata, '$.conversation_id') = q.conversation_id
                AND b.start_ts <= q.start_ts AND b.git_branch_name IS NOT NULL
              ORDER BY b.start_ts DESC LIMIT 1) AS git_branch
       FROM ai_queries q
      WHERE q.conversation_id IN (${placeholders})
      ORDER BY q.start_ts`,
    ...conversationIds
  );

  const drafts: EventDraft[] = [];
  const skipped: SkippedRow[] = [];
  for (const row of rows) {
    const parsed = parseQueryInput(row.input);
    if (!parsed.ok) {
      skipped.push({
        table: "ai_queries",
        reason: "input failed schema",
        detail: `cid=${row.conversation_id} ts=${row.start_ts}: ${parsed.error}`,
      });
      continue;
    }
    const meta: Record<string, unknown> = {
      cwd: row.working_directory,
      model: row.model_id,
      exchange_id: row.exchange_id,
    };
    if (row.git_branch) meta.git_branch = row.git_branch;
    drafts.push({
      conversation_id: row.conversation_id,
      ts: row.start_ts,
      role: "user",
      kind: "query",
      content: parsed.data[0]!.Query.text,
      meta,
    });
  }
  return { drafts, skipped };
}

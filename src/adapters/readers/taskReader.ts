import type { EventDraft, ReaderResult, SkippedRow, TaskRow } from "../../domain/models.js";
import type { ReadableDb } from "../../domain/ports.js";
import { walkProtobuf } from "../protobufWalk.js";
import { compactNodes } from "../compact.js";
import { applySchemaNames } from "../schemaOverlay.js";
import { collapseDeltas } from "../collapseDeltas.js";
import {
  createEnvelopeDedupeState,
  dedupeEnvelopes,
  type EnvelopeDedupeState,
} from "../envelopeDedupe.js";

/**
 * Read agent_tasks for the given conversations and walk each `task` BLOB.
 *
 * The BLOB is the protobuf `Task` message (schema: warpdotdev/warp-proto-apis).
 * The schema-less walker recovers readable strings with wire-format field
 * paths; `applySchemaNames` then renames those paths to semantic field names
 * and extracts the `message_kind` (Message oneof variant) and `tool`
 * (ToolCall oneof variant) that drive decision tracing. The schema overlay
 * validates every path segment and, on any mismatch with the live schema rev,
 * keeps the original numbered path and marks `schema_mismatch` — it never
 * silently relabels. `collapseDeltas` then collapses Warp's per-field
 * streaming deltas — many `tool_call`/`tool_call_result` occurrences that
 * each set a single leaf field of what is logically one call/result — into
 * one event per call carrying a `fields` map (plus a `skills` summary for
 * `updated_skills_context` fan-out), and merges any other consecutive
 * same-`message_kind` deltas of one entity onto their final value with a
 * `merged_count`. A walk that hits malformed/truncated bytes still returns
 * whatever it recovered (partial), with every event marked
 * `confidence: "heuristic"`. The walker never throws; a task that errors is
 * skipped (never crashes the run). Subagent tasks share the parent
 * conversation_id, so they are included automatically.
 */
export function readTasks(
  db: ReadableDb,
  conversationIds: readonly string[]
): ReaderResult {
  if (conversationIds.length === 0) return { drafts: [], skipped: [] };

  const placeholders = conversationIds.map(() => "?").join(",");
  const rows = db.all<TaskRow>(
    `SELECT conversation_id, task_id, task, last_modified_at
       FROM agent_tasks
      WHERE conversation_id IN (${placeholders})
      ORDER BY last_modified_at, task_id`,
    ...conversationIds
  );

  const drafts: EventDraft[] = [];
  const skipped: SkippedRow[] = [];
  const envelopeStateByConversation = new Map<string, EnvelopeDedupeState>();
  for (const row of rows) {
    if (!row.task || row.task.length === 0) {
      skipped.push({
        table: "agent_tasks",
        reason: "empty task",
        detail: `task_id=${row.task_id}`,
      });
      continue;
    }

    let result;
    try {
      result = walkProtobuf(row.task);
    } catch (e) {
      skipped.push({
        table: "agent_tasks",
        reason: "walk threw",
        detail: `task_id=${row.task_id}: ${(e as Error).message}`,
      });
      continue;
    }

    const envelopeState =
      envelopeStateByConversation.get(row.conversation_id) ?? createEnvelopeDedupeState();
    envelopeStateByConversation.set(row.conversation_id, envelopeState);
    const collapsed = collapseDeltas(applySchemaNames(compactNodes(result.nodes)));
    for (let taskEventIndex = 0; taskEventIndex < collapsed.length; taskEventIndex++) {
      const node = dedupeEnvelopes([collapsed[taskEventIndex]!], envelopeState)[0];
      if (node === undefined) continue;
      const meta: Record<string, unknown> = {
        field_path: node.field_path,
        task_id: row.task_id,
        task_event_index: taskEventIndex,
      };
      if (node.message_kind !== undefined) meta.message_kind = node.message_kind;
      if (node.tool !== undefined) meta.tool = node.tool;
      if (node.tool_call_id !== undefined) meta.tool_call_id = node.tool_call_id;
      if (node.fields !== undefined) meta.fields = node.fields;
      if (node.skills !== undefined) meta.skills = node.skills;
      if (node.merged_count !== undefined) meta.merged_count = node.merged_count;
      if (node.repeat !== undefined) meta.repeat = node.repeat;
      if (node.truncated) {
        meta.truncated = true;
        meta.original_len = node.original_len;
      }
      if (node.schema_mismatch) meta.confidence = "schema-mismatch";
      if (!result.complete && meta.confidence === undefined) {
        meta.confidence = "heuristic";
      }
      drafts.push({
        conversation_id: row.conversation_id,
        ts: row.last_modified_at,
        role: "assistant",
        kind: "agent_message",
        content: node.value,
        meta,
      });
    }
  }
  return { drafts, skipped };
}

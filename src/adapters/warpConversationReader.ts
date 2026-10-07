import type { EventDraft, Label, SkippedRow } from "../domain/models.js";
import type { ConversationReader, ConversationSource, CaptureRead } from "../domain/ports.js";
import { findSeeds } from "./readers/markerReader.js";
import { readQueries } from "./readers/queryReader.js";
import { readBlocks } from "./readers/blockReader.js";
import { readTasks } from "./readers/taskReader.js";

/**
 * ConversationReader backed by Warp's local SQLite DB.
 *
 * Owns the source-specific work the use-case must not depend on: binding a capture id
 * to its conversations (via CAPTURE_MARKER emissions in markerReader) and reading
 * every event for those conversations (queries, blocks, agent_tasks). All reads
 * run inside the read-only VACUUM INTO snapshot provided by the
 * ConversationSource, so the use-case receives plain, materialized data.
 *
 * A different source (e.g. Claude Code JSONL transcripts) would be a sibling
 * adapter implementing the same ConversationReader port instead of this one.
 */
export class WarpConversationReader implements ConversationReader {
  constructor(private readonly source: ConversationSource) {}

  readCapture(captureId: string): CaptureRead {
    return this.source.withSnapshot((db) => {
      const seeds = findSeeds(db, captureId);
      const unbindable = seeds.filter((s) => s.status === "unbindable");
      const collisions = seeds.filter((s) => s.status === "collision");
      const heuristic_bindings = seeds.filter((s) => s.confidence === "heuristic");

      const labelByCid = new Map<string, Label>();
      for (const s of seeds) {
        if (s.status === "bound" && s.conversation_id) {
          labelByCid.set(s.conversation_id, s.label);
        }
      }
      const allCids = [...labelByCid.keys()];

      // Gather drafts from every reader in fixed order (queries, blocks,
      // tasks); the use-case's stable sort preserves this order for equal ts.
      const drafts: EventDraft[] = [];
      const skipped: SkippedRow[] = [];
      for (const result of [
        readQueries(db, allCids),
        readBlocks(db, allCids),
        readTasks(db, allCids),
      ]) {
        drafts.push(...result.drafts);
        skipped.push(...result.skipped);
      }

      return { source: "warp", labelByCid, drafts, skipped, unbindable, collisions, heuristic_bindings };
    });
  }
}

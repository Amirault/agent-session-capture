import type { EventDraft, Label, SessionInfo, SkippedRow } from "../domain/models.js";
import type {
  ConversationReader,
  ConversationSource,
  CaptureRead,
  ReadableDb,
  SessionReader,
  ReadSessionsOptions,
  SessionsRead,
  SessionGather,
} from "../domain/ports.js";
import { parseSinceUtc, parseStampUtc } from "../domain/stamps.js";
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
 * Also implements the SessionReader port for the non-marker `--sessions` mode:
 * enumerate every conversation (durable tables only) and gather events only for
 * the ones that changed. A different source (e.g. Claude Code JSONL transcripts)
 * would be a sibling adapter implementing the same ports instead of this one.
 */
export class WarpConversationReader implements ConversationReader, SessionReader {
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

  /**
   * Session mode (`--sessions`): enumerate every conversation and gather
   * events for the ones that changed — all inside ONE snapshot.
   *
   * Enumeration unions the durable `agent_conversations` and `agent_tasks`
   * tables (unlike `blocks`/`ai_queries`, these do not evict) and takes the
   * MAX stamp per conversation, so a conversation present only in
   * `agent_tasks` is listed too. The `--since`/`--conversation` filters are
   * applied in TypeScript; skip-unchanged decisions use raw string equality
   * (no parsing). Gathering reuses the marker-mode readers in the same fixed
   * order, one conversation at a time — marker commands appear as ordinary
   * command events, with no special-casing.
   */
  readSessions(options: ReadSessionsOptions): SessionsRead {
    return this.source.withSnapshot((db) => {
      let selected = enumerateSessions(db);

      if (options.conversation !== undefined) {
        selected = selected.filter((s) => s.conversation_id === options.conversation);
      }
      if (options.since !== undefined) {
        const cutoff = parseSinceUtc(options.since);
        if (cutoff !== null) {
          // A stamp that cannot be parsed fails toward inclusion — the mode
          // must not silently hide a conversation because of it.
          selected = selected.filter((s) => {
            if (s.last_modified === null) return true;
            const activity = parseStampUtc(s.last_modified);
            return activity === null ? true : activity >= cutoff;
          });
        }
      }

      const reads = new Map<string, SessionGather>();
      for (const info of selected) {
        if (!shouldGather(info, options)) continue;
        const cid = info.conversation_id;
        const queries = readQueries(db, [cid]);
        const blocks = readBlocks(db, [cid]);
        const tasks = readTasks(db, [cid]);
        reads.set(cid, {
          drafts: [...queries.drafts, ...blocks.drafts, ...tasks.drafts],
          skipped: [...queries.skipped, ...blocks.skipped, ...tasks.skipped],
          last_modified: info.last_modified,
        });
      }

      return { source: "warp", sessions: selected, reads };
    });
  }
}

interface SessionStampRow {
  conversation_id: string;
  last_modified: string | null;
}

/**
 * Enumerate every conversation with its last-activity stamp: the MAX over the
 * durable `agent_conversations`/`agent_tasks` tables only. Deterministic
 * order: stamped sessions by stamp (tiebreak on id), then unstamped ones.
 */
function enumerateSessions(db: ReadableDb): SessionInfo[] {
  const rows = db.all<SessionStampRow>(
    `SELECT conversation_id, MAX(last_modified_at) AS last_modified
       FROM (
         SELECT conversation_id, last_modified_at FROM agent_conversations
         UNION ALL
         SELECT conversation_id, last_modified_at FROM agent_tasks
       )
      GROUP BY conversation_id
      ORDER BY (last_modified IS NULL), last_modified, conversation_id`
  );
  return rows.map((row) => ({
    conversation_id: row.conversation_id,
    last_modified: row.last_modified,
  }));
}

/**
 * Skip-unchanged: gather unless the stored header's stamp equals the fresh
 * stamp (raw string equality — the stamps never change representation). New
 * conversations, `null` stamps (no durable stamp — always re-extract) and the
 * `--conversation` debug filter are always gathered.
 */
function shouldGather(info: SessionInfo, options: ReadSessionsOptions): boolean {
  if (options.conversation !== undefined && options.conversation === info.conversation_id) {
    return true;
  }
  if (info.last_modified === null) return true;
  const stored = options.storedByCid.get(info.conversation_id);
  if (stored === undefined) return true;
  return stored.last_modified !== info.last_modified;
}

import path from "node:path";
import type {
  BundleSource,
  EventDraft,
  SessionEvent,
  SessionHeader,
  SessionRunSummary,
  SkippedRow,
  WrittenSession,
} from "../domain/models.js";
import type { SessionReader } from "../domain/ports.js";
import { causalSortTimestamp } from "../domain/causalSort.js";
import { mergeEventLists } from "../domain/mergeBundle.js";
import { JsonlSink } from "../adapters/jsonlSink.js";
import { loadStoredSession, scanStoredSessions, sourceMismatch } from "../adapters/jsonlSessionReader.js";

export interface ExtractSessionsOptions {
  /** Output directory (session files land in `<outDir>/sessions/`). */
  outDir: string;
  /** The run's source (validated by the CLI; drives the stored-file guards). */
  source: BundleSource;
  /** Raw `--since` value, when given. */
  since?: string;
  /** Raw `--conversation` value, when given. */
  conversation?: string;
  /** When true, replace stored session files instead of merging with them. */
  noMerge?: boolean;
}

export interface ExtractSessionsResult {
  summary: SessionRunSummary;
}

/** Order drafts into SessionEvents: causal sort with a source-order tiebreak, then seq 1..N. */
function toSessionEvents(drafts: EventDraft[]): SessionEvent[] {
  const highWaterByConversation = new Map<string, string>();
  return drafts
    .map((draft, originalIndex) => ({
      draft,
      sortTimestamp: causalSortTimestamp(draft, highWaterByConversation),
      originalIndex,
    }))
    .sort((a, b) =>
      a.sortTimestamp < b.sortTimestamp
        ? -1
        : a.sortTimestamp > b.sortTimestamp
          ? 1
          : a.originalIndex - b.originalIndex
    )
    .map((candidate, index) => ({
      conversation_id: candidate.draft.conversation_id,
      seq: index + 1,
      ts: candidate.draft.ts,
      role: candidate.draft.role,
      kind: candidate.draft.kind,
      content: candidate.draft.content,
      meta: candidate.draft.meta,
    }));
}

/**
 * Orchestrate the non-marker session export (`--sessions`) against a
 * SessionReader:
 *
 * 1. Scan `<outDir>/sessions/*.jsonl` — each file's FIRST line only (files can
 *    be MBs; a full parse is only needed for merging). Guards mirror the marker
 *    mode's stored-bundle guards: corrupt / header-vs-filename conversation
 *    mismatch fails the run loudly, nothing overwritten — unless `--no-merge`
 *    downgrades each violation to a warning and replaces the file. A file of
 *    another source (the local sources share `sessions/`) is left alone,
 *    unless this run gathers its conversation: then the source mismatch fails
 *    the run the same way.
 * 2. `reader.readSessions({ since, conversation, storedByCid })` — one source
 *    snapshot for the whole run; the reader skips conversations whose stored
 *    `last_modified` stamp is unchanged.
 * 3. Per gathered conversation: causal order + seq 1..N, then decay-safe merge
 *    with the stored file's events when present and `!noMerge` (re-sorted,
 *    re-seq'd), then an atomic write (tmp + rename).
 * 4. Stored files not in this run's enumeration (source eviction, or `--since`
 *    filtered them out) are kept untouched — files are the durable record,
 *    never GC'd.
 *
 * A fresh-read failure throws (the CLI reports it and exits 1): nothing is
 * overwritten on failure and stored files are never deleted, so no
 * stored-only degrade is needed. Every gather, load and merge completes before
 * the first write, so a mid-run failure leaves the disk untouched.
 */
export async function extractSessions(
  reader: SessionReader,
  options: ExtractSessionsOptions
): Promise<ExtractSessionsResult> {
  const { outDir, source, since, conversation } = options;
  const noMerge = options.noMerge ?? false;
  const sessionsDir = path.join(outDir, "sessions");

  // 1. Scan the stored sessions: first-line headers only, guarded.
  const { byCid: storedByEntry, otherSources, warnings } = scanStoredSessions(sessionsDir, { source, noMerge });
  const storedByCid = new Map([...storedByEntry].map(([cid, entry]) => [cid, entry.header]));

  // 2. Fresh read: one snapshot; only changed conversations are gathered.
  const { sessions, reads } = await reader.readSessions({ since, conversation, storedByCid });
  for (const cid of reads.keys()) {
    const other = otherSources.get(cid);
    if (other === undefined) continue;
    if (!noMerge) {
      throw new Error(
        `${sourceMismatch(other, source)} — refusing to continue; fix or remove the file, or re-run with --no-merge to replace it`
      );
    }
    warnings.push(`replacing stored session (--no-merge): ${sourceMismatch(other, source)}`);
  }

  // 3. Build every session in memory first (loads + merges); writes come after,
  //    so any failure leaves every file on disk untouched.
  const built: { header: SessionHeader; events: SessionEvent[] }[] = [];
  const skippedRows: SkippedRow[] = [];
  for (const [cid, gather] of reads) {
    let events = toSessionEvents(gather.drafts);
    const storedEntry = storedByEntry.get(cid);
    if (!noMerge && storedEntry !== undefined) {
      const storedFile = loadStoredSession(storedEntry.path);
      events = mergeEventLists(storedFile.events, events);
    }
    built.push({
      header: {
        type: "session_header",
        conversation_id: cid,
        source,
        last_modified: gather.last_modified,
        extracted_at: new Date().toISOString(),
      },
      events,
    });
    skippedRows.push(...gather.skipped);
  }

  // 4. Write atomically, one file per conversation.
  const sink = new JsonlSink(outDir);
  const written: WrittenSession[] = [];
  for (const session of built) {
    const outPath = sink.writeSession(session.header, session.events);
    written.push({
      conversation_id: session.header.conversation_id,
      path: outPath,
      events: session.events.length,
    });
  }

  // 5. Accounting: unchanged (stamp matched, skipped) and untouched stored
  //    files (not in this run's enumeration — kept, never GC'd).
  const gatheredCids = new Set(reads.keys());
  const unchanged = sessions
    .filter((s) => !gatheredCids.has(s.conversation_id))
    .map((s) => s.conversation_id);

  const touchedPaths = new Set<string>(written.map((w) => w.path));
  for (const cid of unchanged) {
    const entry = storedByEntry.get(cid);
    if (entry !== undefined) touchedPaths.add(entry.path);
  }
  const untouched = [...storedByEntry.values()]
    .filter((entry) => !touchedPaths.has(entry.path))
    .map((entry) => entry.path);

  return {
    summary: {
      source,
      since: since ?? null,
      conversation: conversation ?? null,
      enumerated: sessions.length,
      written,
      unchanged,
      untouched,
      skipped_rows: skippedRows,
      warnings,
    },
  };
}

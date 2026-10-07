import type { BundleSource, EventDraft, Label, SeedMatch, SkippedRow, CaptureBundle } from "./models.js";

/**
 * Minimal read-only query interface over a SQLite snapshot.
 * Both the live snapshot and the test fixture implement this, so readers and
 * the use-case are testable without touching the real Warp DB.
 */
export interface ReadableDb {
  all<T = unknown>(sql: string, ...params: unknown[]): T[];
}

/**
 * Source port: owns the live Warp DB and hands out a read-only snapshot to run
 * work against. The snapshot is ephemeral and deleted once `fn` returns.
 */
export interface ConversationSource {
  withSnapshot<T>(fn: (db: ReadableDb) => T): T;
  /** Resolve the live DB path or throw a clear error listing probed paths. */
  resolveLiveDbPath(): string;
}

/** Sink port: writes a bundle as strict JSONL and returns the output path. */
export interface ConversationSink {
  write(bundle: CaptureBundle): string;
}

/**
 * Reader port: resolves one capture id's conversations and their events from a source.
 *
 * The use-case depends on this port — never on a source's storage details — so a
 * different source (e.g. Claude Code's JSONL transcripts) can be a sibling
 * adapter implementing the same contract. The Warp SQLite implementation
 * lives in adapters/warpConversationReader.ts.
 */
export interface ConversationReader {
  readCapture(captureId: string): CaptureRead;
}

/**
 * What a ConversationReader returns for one capture id.
 *
 * `labelByCid` is the source-agnostic binding (conversation -> its label); the
 * use-case classifies, time-orders, and tallies labels from it. The
 * `unbindable`/`collisions` anomaly fields are marker-specific (Warp emits
 * CAPTURE_MARKER rows); a non-marker source returns empty arrays for both.
 */
export interface CaptureRead {
  source: BundleSource;
  labelByCid: Map<string, Label>;
  drafts: EventDraft[];
  skipped: SkippedRow[];
  unbindable: SeedMatch[];
  collisions: SeedMatch[];
  /** Seeds bound via the ai_queries temporal-proximity fallback (local orchestrated subagents). */
  heuristic_bindings: SeedMatch[];
}


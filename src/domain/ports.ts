import type {
  BundleSource,
  EventDraft,
  Label,
  SeedMatch,
  SessionHeader,
  SessionInfo,
  SkippedRow,
  CaptureBundle,
} from "./models.js";

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
  readCapture(captureId: string): Promise<CaptureRead>;
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

/** Options for SessionReader.readSessions. */
export interface ReadSessionsOptions {
  /**
   * ISO-8601 date or datetime cutoff (already validated by the CLI): only
   * conversations whose last activity is at/after it are enumerated.
   */
  since?: string;
  /** Debug filter: enumerate exactly this one conversation (always gathered). */
  conversation?: string;
  /**
   * Stored session headers by conversation id, passed in by the use case. The
   * pragmatic input that lets one snapshot serve enumeration + skip-unchanged
   * for any future adapter: a session whose stored `last_modified` equals the
   * fresh stamp (raw string equality) is not gathered.
   */
  storedByCid: Map<string, SessionHeader>;
}

/** What the reader gathered for one conversation. */
export interface SessionGather {
  drafts: EventDraft[];
  skipped: SkippedRow[];
  /** The conversation's fresh source-native stamp (recorded in the header). */
  last_modified: string | null;
}

/** What SessionReader.readSessions returns for the whole run. */
export interface SessionsRead {
  source: BundleSource;
  /** Sessions enumerated in this run, after the since/conversation filters. */
  sessions: SessionInfo[];
  /** Gathered events per conversation — only conversations that changed (or new). */
  reads: Map<string, SessionGather>;
}

/**
 * Reader port for the non-marker session mode (`--sessions`): enumerates every
 * conversation in the source and gathers events for the ones that changed.
 *
 * A sibling of ConversationReader (which stays untouched for marker mode). The
 * whole read runs inside ONE source snapshot; `storedByCid` (supplied by the
 * use case from the stored file headers) drives skip-unchanged so unchanged
 * conversations are never gathered. Every reader (Warp, Claude Code, Hermes,
 * Langfuse) implements it.
 */
export interface SessionReader {
  readSessions(options: ReadSessionsOptions): Promise<SessionsRead>;
}


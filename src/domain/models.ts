/**
 * Domain models.
 *
 * The tool reads local agent session stores and exports, raw and time-ordered,
 * every conversation tagged with one capture id through a CAPTURE_MARKER.
 * These types are pure (no I/O, no dependencies) so they can be reused by every
 * port and adapter.
 */

/**
 * A free-form tag a marker may carry (`label=<label>`) to distinguish several
 * conversations of one capture id — a step name, an agent role, a run number, …
 * Markers without a label get DEFAULT_LABEL.
 */
export type Label = string;
export const DEFAULT_LABEL = "default";

/** Who emitted an event in the conversation. */
export type Role = "user" | "assistant" | "tool";

/**
 * Kind of conversation event.
 * - query        : a user prompt (from ai_queries.input)
 * - agent_message: assistant text (recovered from agent_tasks protobuf walk)
 * - command      : a shell command execution (from blocks)
 * - tool_call    : a tool invocation (from agent_tasks walk)
 * - tool_result  : a tool/command output (from agent_tasks walk or blocks output)
 */
export type EventKind =
  | "query"
  | "agent_message"
  | "command"
  | "tool_call"
  | "tool_result";

/** A single time-ordered conversation event, serialized as one JSONL line. */
export interface ConversationEvent {
  capture_id: string;
  label: Label;
  conversation_id: string;
  /** Monotonic sequence number within the whole bundle (time-ordered). */
  seq: number;
  /** ISO-8601 timestamp (from the source row's start_ts). */
  ts: string;
  role: Role;
  kind: EventKind;
  content: string;
  meta: Record<string, unknown>;
}

/** A reader-produced event before the use-case attaches capture_id, label, and seq. */
export type EventDraft = Omit<ConversationEvent, "capture_id" | "label" | "seq">;

/**
 * One event of an exported session (the `--sessions` mode). Deliberately
 * without `capture_id`/`label`: a session is not a capture — it is every
 * conversation in the store, not the subset a marker bound. Readers' existing
 * EventDrafts feed both modes unchanged.
 */
export interface SessionEvent extends EventDraft {
  /** Monotonic sequence number within the session (time-ordered). */
  seq: number;
}

/** First line of every `sessions/<file>.jsonl` file. */
export interface SessionHeader {
  type: "session_header";
  conversation_id: string;
  source: BundleSource;
  /**
   * Source-native last-activity stamp. An equal value on re-run means the
   * session is unchanged and the stored file is skipped. `null` means the
   * source has no durable stamp — such a session is always re-extracted.
   */
  last_modified: string | null;
  extracted_at: string;
}

/** A stored session file: its header (line 1) plus its events (lines 2..N). */
export interface SessionFile {
  header: SessionHeader;
  events: SessionEvent[];
}

/** One enumerated session, before any events are gathered for it. */
export interface SessionInfo {
  conversation_id: string;
  /** Source-native last-activity stamp; `null` means "no durable stamp". */
  last_modified: string | null;
}

/** Uniform return shape for every reader. */
export interface ReaderResult {
  drafts: EventDraft[];
  skipped: SkippedRow[];
}

/** First line of every bundle file. */
export type BundleSource = "warp" | "claude-code" | "hermes" | "langfuse";

export interface BundleHeader {
  type: "bundle_header";
  capture_id: string;
  labels: Label[];
  conversations_per_label: Record<Label, number>;
  conversation_ids: string[];
  extracted_at: string;
  source: BundleSource;
}

export interface CaptureBundle {
  header: BundleHeader;
  events: ConversationEvent[];
}

/** Result of binding one marker emission to a conversation. */
export type SeedStatus = "bound" | "unbindable" | "collision";

/**
 * How confident we are in a bound binding.
 * - certain  : backed by the blocks.ai_metadata JOIN (verified 1:1).
 * - heuristic: backed by the ai_queries temporal-proximity fallback
 *              (used when a local orchestrated subagent writes no blocks row).
 */
export type SeedConfidence = "certain" | "heuristic";

export interface SeedMatch {
  /** The conversation id, or null when the marker could not be bound. */
  conversation_id: string | null;
  label: Label;
  marker_command: string;
  start_ts: string;
  status: SeedStatus;
  /**
   * Only set on status=="bound" seeds.
   * Absent means "certain" (blocks JOIN — original behaviour).
   */
  confidence?: SeedConfidence;
}

export interface SkippedRow {
  table: string;
  reason: string;
  detail: string;
}

/** Human-readable run summary (also returned to the CLI). */
export interface RunSummary {
  capture_id: string;
  conversations: number;
  events: number;
  labels: Label[];
  unbindable: SeedMatch[];
  collisions: SeedMatch[];
  /**
   * Seeds bound via the ai_queries temporal-proximity fallback.
   * Present when a local orchestrated subagent writes no blocks row.
   * Worth flagging for human review.
   */
  heuristic_bindings: SeedMatch[];
  skipped_rows: SkippedRow[];
  /** Set when the fresh external-source read failed and the result fell back to a stored bundle. */
  fresh_read_error: string | null;
  output_path: string | null;
}

/** One file written by a session-mode run. */
export interface WrittenSession {
  conversation_id: string;
  /** Path actually written (`<outDir>/sessions/<file>.jsonl`). */
  path: string;
  events: number;
}

/** Result of one `--sessions` run (feeds formatSessionReport). */
export interface SessionRunSummary {
  source: BundleSource;
  /** Raw `--since` value as passed on the command line, when given. */
  since: string | null;
  /** Raw `--conversation` value as passed on the command line, when given. */
  conversation: string | null;
  /** Sessions enumerated in this run, after the --since/--conversation filters. */
  enumerated: number;
  /** Sessions written or rewritten this run, in write order. */
  written: WrittenSession[];
  /** Conversation ids whose stored stamp matched — skipped, file untouched. */
  unchanged: string[];
  /** Stored files not touched by this run (not in this run's enumeration). */
  untouched: string[];
  /** Reader-reported skipped rows (surfaced one per line). */
  skipped_rows: SkippedRow[];
  /** Guard warnings downgraded from errors by --no-merge (one per line). */
  warnings: string[];
}

// --- Raw DB row shapes (what the readers pull from the snapshot) -------------

export interface TaskRow {
  conversation_id: string;
  task_id: string;
  /** Protobuf BLOB. */
  task: Buffer;
  last_modified_at: string;
}

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

/** Uniform return shape for every reader. */
export interface ReaderResult {
  drafts: EventDraft[];
  skipped: SkippedRow[];
}

/** First line of every bundle file. */
export type BundleSource = "warp" | "claude-code" | "hermes";

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

// --- Raw DB row shapes (what the readers pull from the snapshot) -------------

export interface TaskRow {
  conversation_id: string;
  task_id: string;
  /** Protobuf BLOB. */
  task: Buffer;
  last_modified_at: string;
}

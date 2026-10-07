import type {
  ConversationEvent,
  EventDraft,
  RunSummary,
  CaptureBundle,
} from "../domain/models.js";
import { computeBundleHeader } from "../domain/bundleHeader.js";
import { mergeBundles } from "../domain/mergeBundle.js";
import type { ConversationReader, CaptureRead } from "../domain/ports.js";

export interface ExtractOptions {
  /** When true, replace the existing bundle instead of merging with it. */
  noMerge?: boolean;
  /** A previously-captured bundle to merge with (decay-safe). */
  existingBundle?: CaptureBundle | null;
}

export interface ExtractResult {
  bundle: CaptureBundle;
  summary: RunSummary;
}

function numericMeta(draft: EventDraft, key: string): number | undefined {
  const value = draft.meta[key];
  return typeof value === "number" ? value : undefined;
}

function causalSortTimestamp(
  draft: EventDraft,
  highWaterByConversation: Map<string, string>
): string {
  if (numericMeta(draft, "message_id") === undefined) return draft.ts;

  const previous = highWaterByConversation.get(draft.conversation_id);
  const sortTimestamp = previous !== undefined && previous > draft.ts ? previous : draft.ts;
  highWaterByConversation.set(draft.conversation_id, sortTimestamp);
  return sortTimestamp;
}
/**
 * Orchestrate the full extraction for one capture id against a ConversationReader:
 *   reader.readCapture -> bound conversations (label per conversation) + event drafts
 *   -> classify each event by its conversation's label
 *   -> causally order the whole bundle and assign a monotonic seq
 *   -> compute the bundle header (label tally).
 *
 * When an `existingBundle` is supplied (a prior capture on disk) and `noMerge`
 * is false, the fresh bundle is merged with it decay-safe: fresh events are
 * primary, stored events fill gaps left by marker decay or ring-buffer
 * eviction.
 *
 * If the fresh external-source read errors and a stored bundle exists, the
 * result degrades to stored-only (with a `fresh_read_error` warning) instead of
 * aborting — this keeps the stored bundle a recovery path. With no stored bundle to fall
 * back on (first capture), it throws.
 *
 * The reader owns every source-specific detail (how a capture id binds to its
 * conversations, how events are read); this use-case is source-agnostic
 * orchestration. Subagent conversations share the parent conversation_id, so
 * they are pulled in automatically by the reader — no expansion step. Re-runs
 * of a label are distinct conversations and are all kept (no dedup across labels).
 */
export function extractBundle(
  reader: ConversationReader,
  captureId: string,
  options: ExtractOptions = {}
): ExtractResult {
  const { existingBundle = null, noMerge = false } = options;

  // Try the fresh read from the external source. If it errors and we have a
  // stored bundle, degrade to stored-only so the stored bundle stays a recovery path.
  let captureRead: CaptureRead | null = null;
  let freshReadError: string | null = null;
  try {
    captureRead = reader.readCapture(captureId);
  } catch (e) {
    freshReadError = e instanceof Error ? e.message : String(e);
  }

  if (captureRead === null) {
    if (existingBundle) {
      return storedOnlyResult(captureId, existingBundle, freshReadError!);
    }
    // No stored bundle to fall back on — first capture cannot recover.
    throw new Error(
      `fresh read failed for capture id "${captureId}" and no stored bundle exists to fall back on: ${freshReadError}`
    );
  }

  const { source, labelByCid, drafts, skipped, unbindable, collisions, heuristic_bindings } = captureRead;

  // Hermes persists true insertion order in message_id because clocks can move
  // backwards. Raise only the internal sort key to the previous timestamp in the
  // same conversation; this preserves causality while keeping the public ts intact.
  const highWaterByConversation = new Map<string, string>();
  const events: ConversationEvent[] = drafts
    .filter((d) => labelByCid.has(d.conversation_id))
    .map((d, originalIndex) => ({
      draft: d,
      label: labelByCid.get(d.conversation_id)!,
      sortTimestamp: causalSortTimestamp(d, highWaterByConversation),
      originalIndex,
    }))
    .sort((a, b) =>
      a.sortTimestamp < b.sortTimestamp
        ? -1
        : a.sortTimestamp > b.sortTimestamp
          ? 1
          : a.originalIndex - b.originalIndex
    )
    .map((c, i) => ({
      capture_id: captureId,
      label: c.label,
      conversation_id: c.draft.conversation_id,
      seq: i + 1,
      ts: c.draft.ts,
      role: c.draft.role,
      kind: c.draft.kind,
      content: c.draft.content,
      meta: c.draft.meta,
    }));

  const headerFields = computeBundleHeader(captureId, source, labelByCid);
  const freshBundle: CaptureBundle = {
    header: {
      type: "bundle_header",
      ...headerFields,
      extracted_at: new Date().toISOString(),
    },
    events,
  };

  // Merge with the stored bundle when one exists and --no-merge is not set.
  const merged =
    existingBundle && !noMerge
      ? mergeBundles(existingBundle, freshBundle)
      : freshBundle;

  const bundle = merged;

  const summary: RunSummary = {
    capture_id: captureId,
    conversations: merged.header.conversation_ids.length,
    events: merged.events.length,
    labels: merged.header.labels,
    unbindable,
    collisions,
    heuristic_bindings,
    skipped_rows: skipped,
    fresh_read_error: null,
    output_path: null,
  };

  return { bundle, summary };
}

/**
 * Build a stored-only result when the fresh read failed: re-stamp the stored
 * bundle's `extracted_at` and report its stats. The stored events are preserved
 * intact (no merge, no loss); `fresh_read_error` surfaces the degradation.
 */
function storedOnlyResult(
  captureId: string,
  existingBundle: CaptureBundle,
  freshReadError: string
): ExtractResult {
  const bundle: CaptureBundle = {
    header: { ...existingBundle.header, extracted_at: new Date().toISOString() },
    events: existingBundle.events,
  };
  const summary: RunSummary = {
    capture_id: captureId,
    conversations: bundle.header.conversation_ids.length,
    events: bundle.events.length,
    labels: bundle.header.labels,
    unbindable: [],
    collisions: [],
    heuristic_bindings: [],
    skipped_rows: [],
    fresh_read_error: freshReadError,
    output_path: null,
  };
  return { bundle, summary };
}

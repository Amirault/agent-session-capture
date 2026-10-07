import type { ConversationEvent, Label, CaptureBundle } from "./models.js";
import { computeBundleHeader } from "./bundleHeader.js";

/** Merge-only aggregation metadata can change when the same task is re-read. */
const VOLATILE_META_KEYS: ReadonlySet<string> = new Set(["merged_count", "repeat"]);
const SOURCE_SURROGATE_META_KEYS: ReadonlySet<string> = new Set([
  "block_id",
  "exchange_id",
  "task_event_index",
  "task_id",
]);

type KeyMode = "strong" | "compatibility";

/**
 * Stable content identity used across captures. `seq` is always per-run. Only
 * Warp agent_tasks timestamps are mutable; stable timestamps remain part of all
 * other event identities so separate same-content queries/commands cannot merge.
 * Compatibility mode strips newly-added source IDs solely for matching legacy
 * stored bundles that predate those IDs.
 */
function eventKey(event: ConversationEvent, mode: KeyMode): string {
  const meta = Object.fromEntries(
    Object.entries(event.meta)
      .filter(
        ([key]) =>
          !VOLATILE_META_KEYS.has(key) &&
          (mode === "strong" || !SOURCE_SURROGATE_META_KEYS.has(key))
      )
      .sort(([left], [right]) => left.localeCompare(right))
  );
  return JSON.stringify({
    conversation_id: event.conversation_id,
    label: event.label,
    role: event.role,
    kind: event.kind,
    content: event.content,
    meta,
    ...(hasMutableTaskTimestamp(event) ? {} : { ts: event.ts }),
  });
}

function hasMutableTaskTimestamp(event: ConversationEvent): boolean {
  return (
    event.kind === "agent_message" &&
    (typeof event.meta.task_id === "string" ||
      typeof event.meta.task_event_index === "number" ||
      typeof event.meta.field_path === "string")
  );
}

function hasSourceSurrogate(event: ConversationEvent): boolean {
  return Object.keys(event.meta).some((key) => SOURCE_SURROGATE_META_KEYS.has(key));
}

function addIndex(index: Map<string, number[]>, key: string, eventIndex: number): void {
  const indexes = index.get(key) ?? [];
  indexes.push(eventIndex);
  index.set(key, indexes);
}

function claimFreshIndex(
  index: ReadonlyMap<string, number[]>,
  cursors: Map<string, number>,
  consumed: ReadonlySet<number>,
  key: string
): number | undefined {
  const candidates = index.get(key);
  if (candidates === undefined) return undefined;
  let cursor = cursors.get(key) ?? 0;
  while (cursor < candidates.length && consumed.has(candidates[cursor]!)) cursor++;
  cursors.set(key, cursor + 1);
  return candidates[cursor];
}

/**
 * Merge a stored bundle with a fresh one, decay-safe.
 *
 * Fresh events are primary; stored events only fill gaps fresh can no longer bind
 * (full marker decay) or has evicted (partial ring-buffer eviction). Overlapping
 * re-read events dedup by stable source identity. Only Warp agent_tasks ignore
 * their mutable row timestamp, and legacy bundles use a compatibility key that
 * omits newly-added source IDs. Fresh content and meta remain primary; only the
 * earliest observed task timestamp is reconciled from the stored copy.
 *
 * Pure — no I/O. The caller owns reading the stored bundle and writing the
 * merged result. The merged header (labels, per-label tallies, …) is
 * recomputed from the merged event set.
 */
export function mergeBundles(existing: CaptureBundle, fresh: CaptureBundle): CaptureBundle {
  const captureId = fresh.header.capture_id;
  const unioned: ConversationEvent[] = fresh.events.map((event) => ({ ...event }));
  const freshIndexesByStrongKey = new Map<string, number[]>();
  const freshIndexesByCompatibilityKey = new Map<string, number[]>();
  for (let index = 0; index < unioned.length; index++) {
    const event = unioned[index]!;
    addIndex(freshIndexesByStrongKey, eventKey(event, "strong"), index);
    addIndex(freshIndexesByCompatibilityKey, eventKey(event, "compatibility"), index);
  }

  // Pair stored copies with fresh copies by source identity and occurrence. The
  // compatibility fallback is allowed only for stored events predating source
  // surrogates, preventing two newly-identified same-content events from merging.
  const consumedFreshIndexes = new Set<number>();
  const strongCursors = new Map<string, number>();
  const compatibilityCursors = new Map<string, number>();
  for (const stored of existing.events) {
    let freshIndex = claimFreshIndex(
      freshIndexesByStrongKey,
      strongCursors,
      consumedFreshIndexes,
      eventKey(stored, "strong")
    );
    if (freshIndex === undefined && !hasSourceSurrogate(stored)) {
      freshIndex = claimFreshIndex(
        freshIndexesByCompatibilityKey,
        compatibilityCursors,
        consumedFreshIndexes,
        eventKey(stored, "compatibility")
      );
    }
    if (freshIndex === undefined) {
      unioned.push(stored);
      continue;
    }

    consumedFreshIndexes.add(freshIndex);
    const fresh = unioned[freshIndex]!;
    if (hasMutableTaskTimestamp(fresh) && stored.ts < fresh.ts) {
      unioned[freshIndex] = { ...fresh, ts: stored.ts };
    }
  }

  // Stable sort by ts (equal timestamps keep first-seen order), then reassign a
  // monotonic seq across the whole merged bundle.
  unioned.sort((a, b) => (a.ts < b.ts ? -1 : a.ts > b.ts ? 1 : 0));
  const events: ConversationEvent[] = unioned.map((e, i) => ({
    ...e,
    capture_id: captureId,
    seq: i + 1,
  }));

  const labelByCid = new Map<string, Label>();
  for (const e of events) {
    if (!labelByCid.has(e.conversation_id)) {
      labelByCid.set(e.conversation_id, e.label);
    }
  }

  const header = {
    type: "bundle_header" as const,
    ...computeBundleHeader(captureId, fresh.header.source, labelByCid),
    extracted_at: new Date().toISOString(),
  };

  return { header, events };
}

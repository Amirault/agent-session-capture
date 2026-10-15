import type { EventDraft } from "./models.js";

function numericMeta(draft: EventDraft, key: string): number | undefined {
  const value = draft.meta[key];
  return typeof value === "number" ? value : undefined;
}

/**
 * Causal sort timestamp for one draft.
 *
 * Hermes (and any source) can persist true insertion order in message_id
 * because clocks can move backwards. When a draft carries a numeric
 * `message_id`, raise only the internal sort key to the previous timestamp in
 * the same conversation; this preserves causality while keeping the public `ts`
 * intact. The caller keeps one high-water map per conversation (or per run,
 * when drafts are pre-grouped by conversation).
 */
export function causalSortTimestamp(
  draft: EventDraft,
  highWaterByConversation: Map<string, string>
): string {
  if (numericMeta(draft, "message_id") === undefined) return draft.ts;

  const previous = highWaterByConversation.get(draft.conversation_id);
  const sortTimestamp = previous !== undefined && previous > draft.ts ? previous : draft.ts;
  highWaterByConversation.set(draft.conversation_id, sortTimestamp);
  return sortTimestamp;
}

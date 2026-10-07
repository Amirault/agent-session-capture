import type { BundleHeader, BundleSource, Label } from "./models.js";

/**
 * Compute the bundle header fields (everything except `type` and `extracted_at`,
 * which the caller stamps) from the conversation→label binding.
 *
 * Shared by `extractBundle` (live read) and `mergeBundles` (decay-safe merge) so
 * how conversations are tallied lives in exactly one place. `labelByCid` preserves
 * insertion order, which becomes the `conversation_ids` order; labels are listed in
 * order of first appearance.
 */
export function computeBundleHeader(
  captureId: string,
  source: BundleSource,
  labelByCid: Map<string, Label>
): Omit<BundleHeader, "type" | "extracted_at"> {
  const perLabel: Record<Label, number> = {};
  for (const label of labelByCid.values()) {
    perLabel[label] = (perLabel[label] ?? 0) + 1;
  }
  return {
    capture_id: captureId,
    labels: Object.keys(perLabel),
    conversations_per_label: perLabel,
    conversation_ids: [...labelByCid.keys()],
    source,
  };
}

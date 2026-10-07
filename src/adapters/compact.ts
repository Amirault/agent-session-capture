/**
 * Noise-reduction pass over walked protobuf nodes.
 *
 * Drops non-signal strings (base64 residue the walker could not unfold, pure
 * UUIDs), dedupes repeated (field_path, value) pairs with a repeat count, and
 * truncates long values — preserving first-occurrence order. Dedupe runs before
 * truncation so duplicate long values collapse on their original form.
 * No regex is used; UUIDs are validated by character class.
 */
import { looksLikeBase64, type WalkNode } from "./protobufWalk.js";

/** Drop base64-looking strings shorter than this (they are usually real tokens). */
const BASE64_MIN_LEN = 16;
/** Truncate values longer than this (head + marker + tail keeps both ends). */
const TRUNCATE_THRESHOLD = 2000;
const TRUNCATE_HEAD = 1000;
const TRUNCATE_TAIL = 500;

export function compactNodes(nodes: WalkNode[]): WalkNode[] {
  const seen = new Map<string, WalkNode>();
  const survivors: WalkNode[] = [];

  for (const node of nodes) {
    // base64 residue: the walker already unfolded decodable base64, so what
    // remains here is genuine non-decoding noise.
    if (looksLikeBase64(node.value) && node.value.length >= BASE64_MIN_LEN) continue;
    // UUIDs are conversation/turn identifiers with no behavior signal.
    if (isPureUuid(node.value)) continue;

    const key = `${node.field_path}\u0000${node.value}`;
    const existing = seen.get(key);
    if (existing !== undefined) {
      existing.repeat = (existing.repeat ?? 1) + 1;
      continue;
    }
    const survivor: WalkNode = { ...node };
    seen.set(key, survivor);
    survivors.push(survivor);
  }

  // Truncate after dedupe so duplicate long values collapse on their original form.
  for (const node of survivors) {
    if (node.value.length > TRUNCATE_THRESHOLD) {
      const original = node.value;
      node.value =
        original.slice(0, TRUNCATE_HEAD) +
        `…[truncated len=${original.length}]…` +
        original.slice(-TRUNCATE_TAIL);
      node.truncated = true;
      node.original_len = original.length;
    }
  }

  return survivors;
}

/** True for the canonical 8-4-4-4-12 hex UUID form (no regex). */
function isPureUuid(s: string): boolean {
  if (s.length !== 36) return false;
  if (
    s.charCodeAt(8) !== 45 ||
    s.charCodeAt(13) !== 45 ||
    s.charCodeAt(18) !== 45 ||
    s.charCodeAt(23) !== 45
  ) {
    return false;
  }
  for (let i = 0; i < 36; i++) {
    if (i === 8 || i === 13 || i === 18 || i === 23) continue;
    const c = s.charCodeAt(i);
    const hex =
      (c >= 48 && c <= 57) || // 0-9
      (c >= 65 && c <= 70) || // A-F
      (c >= 97 && c <= 102); // a-f
    if (!hex) return false;
  }
  return true;
}

/**
 * Generic, schema-less protobuf wire-format walker.
 *
 * agent_tasks.task is a protobuf BLOB whose schema we do not have. We recover
 * readable strings (task title, tool calls, commands, outputs, file paths) by
 * walking the wire format: each field is a (field_number, wire_type) tag + value.
 *
 * Length-delimited fields (wire type 2) are ambiguous (string | bytes | nested
 * message). Disambiguation, mirroring `protoc --decode_raw`:
 *   1. Try parsing as a nested message. If it parses cleanly AND yields strings,
 *      recurse (field paths accumulate, e.g. "2.1").
 *   2. Else, if it is valid UTF-8 and mostly printable: it is a string. If it
 *      also looks like base64 that decodes to a valid protobuf, recurse on the
 *      decoded bytes instead (recovers base64-wrapped protobuf leaves).
 *   3. Else (binary, not a message) -> skip (no readable string).
 *
 * Malformed/truncated input never throws: the walk stops and returns whatever it
 * recovered so far with `complete: false` (callers flag confidence: "heuristic").
 * No regex is used.
 */

export interface WalkNode {
  /** Dotted field-number path from the root to this string (e.g. "2.1"). */
  field_path: string;
  value: string;
  kind: "string";
  /** Times this (field_path, value) occurred (set by compaction when > 1). */
  repeat?: number;
  /** True when `value` was truncated (set by compaction). */
  truncated?: boolean;
  /** Original value length before truncation (set by compaction). */
  original_len?: number;
}

export interface WalkResult {
  nodes: WalkNode[];
  /** false if the walk hit malformed/truncated bytes (partial recovery). */
  complete: boolean;
}

const MAX_DEPTH = 12;
const PRINTABLE_RATIO_THRESHOLD = 0.75;

interface Varint {
  value: number;
  bytes: number;
  ok: boolean;
}

function readVarint(buf: Buffer, pos: number): Varint {
  let value = 0;
  let shift = 0;
  let p = pos;
  for (let i = 0; i < 10; i++) {
    if (p >= buf.length) return { value: 0, bytes: 0, ok: false };
    const byte = buf[p]!;
    p += 1;
    value += (byte & 0x7f) * (2 ** shift);
    if ((byte & 0x80) === 0) return { value, bytes: p - pos, ok: true };
    shift += 7;
  }
  return { value: 0, bytes: 0, ok: false };
}

function tryUtf8(data: Buffer): string | null {
  const s = data.toString("utf8");
  // Round-trip: invalid UTF-8 becomes replacement chars and won't equal.
  if (Buffer.from(s, "utf8").equals(data)) return s;
  return null;
}

function isPrintableCode(c: number): boolean {
  return (c >= 0x20 && c !== 0x7f) || c === 0x09 || c === 0x0a || c === 0x0d;
}

function printableRatio(s: string): number {
  if (s.length === 0) return 1;
  let printable = 0;
  for (const ch of s) {
    if (isPrintableCode(ch.codePointAt(0)!)) printable += 1;
  }
  return printable / s.length;
}

export function looksLikeBase64(s: string): boolean {
  if (s.length < 4 || s.length % 4 !== 0) return false;
  for (const ch of s) {
    const c = ch.charCodeAt(0);
    const ok =
      (c >= 48 && c <= 57) ||
      (c >= 65 && c <= 90) ||
      (c >= 97 && c <= 122) ||
      c === 43 ||
      c === 47 ||
      c === 61;
    if (!ok) return false;
  }
  return true;
}

export function walkProtobuf(buf: Buffer, prefix = "", depth = 0): WalkResult {
  const nodes: WalkNode[] = [];
  let pos = 0;
  let complete = true;

  while (pos < buf.length) {
    const tag = readVarint(buf, pos);
    if (!tag.ok) {
      complete = false;
      break;
    }
    pos += tag.bytes;
    const fieldNumber = tag.value >>> 3;
    const wireType = tag.value & 7;
    const fieldPath = prefix ? `${prefix}.${fieldNumber}` : `${fieldNumber}`;

    if (wireType === 0) {
      const v = readVarint(buf, pos);
      if (!v.ok) {
        complete = false;
        break;
      }
      pos += v.bytes;
    } else if (wireType === 1) {
      if (pos + 8 > buf.length) {
        complete = false;
        break;
      }
      pos += 8;
    } else if (wireType === 5) {
      if (pos + 4 > buf.length) {
        complete = false;
        break;
      }
      pos += 4;
    } else if (wireType === 2) {
      const len = readVarint(buf, pos);
      if (!len.ok) {
        complete = false;
        break;
      }
      pos += len.bytes;
      if (len.value > buf.length - pos) {
        complete = false;
        break;
      }
      const data = buf.subarray(pos, pos + len.value);
      pos += len.value;
      const inner = interpretLengthDelimited(data, fieldPath, depth);
      nodes.push(...inner.nodes);
    } else {
      // Unknown / group wire types (3, 4, 6, 7) -> stop (malformed for our purposes).
      complete = false;
      break;
    }
  }

  return { nodes, complete };
}

function interpretLengthDelimited(
  data: Buffer,
  fieldPath: string,
  depth: number
): WalkResult {
  if (data.length === 0 || depth >= MAX_DEPTH) {
    return { nodes: [], complete: true };
  }

  // 1. Try as a nested message: a real sub-message parses cleanly and yields
  //    strings; a plain string's bytes usually hit an invalid wire type.
  const sub = walkProtobuf(data, fieldPath, depth + 1);
  if (sub.complete && sub.nodes.length > 0) {
    return { nodes: sub.nodes, complete: true };
  }

  // 2. Else, if it is valid UTF-8 and mostly printable -> string.
  const text = tryUtf8(data);
  if (text !== null && text.length > 0 && printableRatio(text) >= PRINTABLE_RATIO_THRESHOLD) {
    // 2a. base64-wrapped protobuf leaf?
    if (text.length >= 8 && looksLikeBase64(text)) {
      const decoded = tryBase64Decode(text);
      if (decoded !== null) {
        const inner = walkProtobuf(decoded, fieldPath, depth + 1);
        if (inner.complete && inner.nodes.length > 0) {
          return { nodes: inner.nodes, complete: true };
        }
      }
    }
    // 2b. plain string.
    return { nodes: [{ field_path: fieldPath, value: text, kind: "string" }], complete: true };
  }

  // 3. Binary, not a message -> no readable string (skip).
  return { nodes: [], complete: true };
}

function tryBase64Decode(s: string): Buffer | null {
  try {
    const b = Buffer.from(s, "base64");
    return b.length === 0 ? null : b;
  } catch {
    return null;
  }
}

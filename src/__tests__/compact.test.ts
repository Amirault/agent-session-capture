import { describe, it, expect } from "vitest";
import type { WalkNode } from "../adapters/protobufWalk.js";
import { compactNodes } from "../adapters/compact.js";

function n(field_path: string, value: string): WalkNode {
  return { field_path, value, kind: "string" };
}

describe("§9.11 compact — task-node noise reduction", () => {
  it("Given base64-artifact values (>=16 chars) alongside real text and a short token, When compacted, Then artifacts are dropped while real text and short tokens are kept", () => {
    // Given — a 20-char non-decoding base64 residue, real text, and a 4-char base64-looking token
    const nodes = [
      n("5.7", "KgwKBAgCEAQKBAgEEAg="),
      n("2", "Implement Spec Using Manual Skill"),
      n("5.7.1", "IgA="),
    ];

    // When
    const out = compactNodes(nodes);

    // Then — real text and the short token survive; the artifact is excluded
    const values = out.map((x) => x.value);
    expect(values).toContain("Implement Spec Using Manual Skill");
    expect(values).toContain("IgA=");
    expect(values).not.toContain("KgwKBAgCEAQKBAgEEAg=");
  });

  it("Given pure-UUID values alongside non-UUID text, When compacted, Then UUIDs are dropped and non-UUID text is kept", () => {
    // Given — a UUID, real text, and a non-UUID identifier-looking string
    const nodes = [
      n("1", "f730cfb4-ce86-4069-b469-743804a94fe4"),
      n("2", "Implement Spec Using Manual Skill"),
      n("9", "not-a-uuid"),
    ];

    // When
    const out = compactNodes(nodes);

    // Then — only the non-UUID text remains, in order
    expect(out.map((x) => x.value)).toEqual(["Implement Spec Using Manual Skill", "not-a-uuid"]);
  });

  it("Given repeated (field_path, value) pairs mixed with distinct siblings, When compacted, Then duplicates collapse onto the first survivor with a repeat count, distinct values are kept, and first-occurrence order is preserved", () => {
    // Given — Acme x3 on X.1, plus two distinct X.2 values
    const nodes = [
      n("X.1", "Acme"),
      n("X.1", "Acme"),
      n("X.2", "agent-add-mcp"),
      n("X.1", "Acme"),
      n("X.2", "different-action"),
    ];

    // When
    const out = compactNodes(nodes);

    // Then — three survivors in first-occurrence order; repeat only on the duplicated one
    expect(out.map((x) => x.value)).toEqual(["Acme", "agent-add-mcp", "different-action"]);
    expect(out.map((x) => x.field_path)).toEqual(["X.1", "X.2", "X.2"]);
    expect(out[0]!.repeat).toBe(3);
    expect(out[1]).not.toHaveProperty("repeat");
    expect(out[2]).not.toHaveProperty("repeat");
  });

  it("Given a value longer than the truncate threshold and a short value, When compacted, Then the long value is head+tail truncated with a marker, original_len, and truncated flag, and the short value is left intact", () => {
    // Given — a 2013-char value and a short value
    const long = "A".repeat(1000) + "MIDDLE" + "B".repeat(1000) + "ENDTAIL";
    const nodes = [n("X.3", long), n("X.4", "short")];

    // When
    const out = compactNodes(nodes);

    // Then — long survivor is truncated and flagged; short survivor is untouched
    const big = out.find((x) => x.field_path === "X.3")!;
    expect(big.truncated).toBe(true);
    expect(big.original_len).toBe(long.length);
    expect(big.value.startsWith("A".repeat(1000))).toBe(true);
    expect(big.value.endsWith("ENDTAIL")).toBe(true);
    expect(big.value).toContain("[truncated");
    const small = out.find((x) => x.field_path === "X.4")!;
    expect(small.truncated).toBeUndefined();
    expect(small.value).toBe("short");
  });

  it("Given values at the base64-length and truncate-length boundaries, When compacted, Then a 16-char base64 artifact is dropped and a 2000-char value is kept intact (not truncated)", () => {
    // Given — exactly 16 base64-charset chars (drop boundary) and exactly 2000 chars (truncate boundary, not > threshold)
    const atBase64Min = "A".repeat(16);
    const atTruncateMax = "x".repeat(1999) + " "; // space => not base64; length 2000 => not > 2000
    const nodes = [n("B.1", atBase64Min), n("B.2", atTruncateMax)];

    // When
    const out = compactNodes(nodes);

    // Then — only the 2000-char value survives, untruncated, at full length
    expect(out.map((x) => x.field_path)).toEqual(["B.2"]);
    expect(out[0]!.truncated).toBeUndefined();
    expect(out[0]!.value.length).toBe(2000);
  });

  it("Given a realistic mix of UUIDs, base64 artifacts, duplicates, and a long content dump, When compacted, Then only real signal survives in first-occurrence order with repeat counts and truncation", () => {
    // Given — UUIDs, base64 residue, duplicated siblings, and a long realistic message
    const uuid = "f730cfb4-ce86-4069-b469-743804a94fe4";
    const long = "DETAIL: reviewing the implementation step by step in detail. ".repeat(60);
    const nodes = [
      n("1", uuid),
      n("2", "Implement Spec Using Manual Skill"),
      n("5.1", "bed38ec6-f50c-4826-abfc-83967bd6a1fe"),
      n("5.11", uuid),
      n("5.7", "KgwKBAgCEAQKBAgEEAg="),
      n("5.7", "KgwKBAgCEAQKBAgEEAg="),
      n("S.1", "Acme"),
      n("S.1", "Acme"),
      n("S.2", "agent-add-mcp"),
      n("S.2", "agent-add-mcp"),
      n("S.3", long),
    ];

    // When
    const out = compactNodes(nodes);

    // Then — four survivors: real text, two duplicated siblings (repeat:2), one truncated long
    expect(out.map((x) => x.field_path)).toEqual(["2", "S.1", "S.2", "S.3"]);
    expect(out[0]!.value).toBe("Implement Spec Using Manual Skill");
    expect(out[1]!.value).toBe("Acme");
    expect(out[1]!.repeat).toBe(2);
    expect(out[2]!.value).toBe("agent-add-mcp");
    expect(out[2]!.repeat).toBe(2);
    expect(out[3]!.truncated).toBe(true);
    expect(out[3]!.original_len).toBe(long.length);
    expect(out[3]!.value.startsWith("DETAIL")).toBe(true);
  });
});

import { describe, it, expect } from "vitest";
import { mergeEventLists } from "../domain/mergeBundle.js";
import type { SessionEvent } from "../domain/models.js";

function sessionEvent(
  ts: string,
  content: string,
  meta: Record<string, unknown> = {},
  overrides: Partial<SessionEvent> = {},
): SessionEvent {
  return {
    conversation_id: "c1",
    seq: 1,
    ts,
    role: "user",
    kind: "query",
    content,
    meta,
    ...overrides,
  };
}

describe("§9.20 mergeEventLists — decay-safe merge over label-free session events", () => {
  it("Given identical stored and fresh events, When mergeEventLists, Then one event survives (same-content dedup) with no label key", () => {
    // Given
    const stored = [sessionEvent("2026-06-30 10:10:00.000000", "continue", { cwd: "/repo", model: "m" })];
    const fresh = [sessionEvent("2026-06-30 10:10:00.000000", "continue", { cwd: "/repo", model: "m" })];

    // When
    const merged = mergeEventLists(stored, fresh);

    // Then
    expect(merged).toHaveLength(1);
    expect(merged[0]!.content).toBe("continue");
    expect("label" in merged[0]!).toBe(false); // session events stay label-free
  });

  it("Given the same query content observed at two different stable timestamps, When mergeEventLists, Then both survive (different ts means different events)", () => {
    // Given
    const stored = [sessionEvent("2026-06-30 10:10:00.000000", "continue")];
    const fresh = [sessionEvent("2026-06-30 10:10:00.000000", "continue"), sessionEvent("2026-06-30 10:30:00.000000", "continue")];

    // When
    const merged = mergeEventLists(stored, fresh);

    // Then
    expect(merged).toHaveLength(2);
    expect(merged.map((e) => e.ts)).toEqual(["2026-06-30 10:10:00.000000", "2026-06-30 10:30:00.000000"]);
  });

  it("Given a task event re-observed with a later last_modified_at, When mergeEventLists, Then one event survives with the earliest observed timestamp and the freshest meta (mutable ts reconciliation)", () => {
    // Given — agent_tasks.last_modified_at is mutable: re-running bumps the ts
    const stored = [
      sessionEvent("2026-06-30 10:30:00.000000", "task message", { task_id: "t1" }, { kind: "agent_message", role: "assistant" }),
    ];
    const fresh = [
      sessionEvent("2026-06-30 10:45:00.000000", "task message", { task_id: "t1" }, { kind: "agent_message", role: "assistant" }),
    ];

    // When
    const merged = mergeEventLists(stored, fresh);

    // Then
    expect(merged).toHaveLength(1);
    expect(merged[0]!.ts).toBe("2026-06-30 10:30:00.000000"); // earliest observed timestamp
    expect(merged[0]!.meta).toEqual({ task_id: "t1" }); // freshest meta
  });

  it("Given duplicate task events differing only in volatile meta (repeat/merged_count), When mergeEventLists, Then they dedup into one event (volatile meta ignored by the identity key)", () => {
    // Given
    const stored = [
      sessionEvent("2026-06-30 10:30:00.000000", "streaming message", { field_path: "messages.1.text", repeat: 1 }, { kind: "agent_message", role: "assistant" }),
    ];
    const fresh = [
      sessionEvent("2026-06-30 10:30:00.000000", "streaming message", { field_path: "messages.1.text", repeat: 6, merged_count: 2 }, { kind: "agent_message", role: "assistant" }),
    ];

    // When
    const merged = mergeEventLists(stored, fresh);

    // Then — one event; fresh meta wins (merged_count/repeat are informational, not identity)
    expect(merged).toHaveLength(1);
    expect(merged[0]!.meta).toEqual({ field_path: "messages.1.text", repeat: 6, merged_count: 2 });
  });

  it("Given a stored session richer than the fresh read (ring-buffer eviction), When mergeEventLists, Then stored-only events survive and the merged list is re-sorted and re-seqenced 1..N", () => {
    // Given — fresh read lost the first two events (evicted from the store)
    const stored = [
      sessionEvent("2026-06-30 10:10:00.000000", "q1"),
      sessionEvent("2026-06-30 10:20:00.000000", "npm test", {}, { kind: "command" }),
      sessionEvent("2026-06-30 10:30:00.000000", "task message", { task_id: "t1" }, { kind: "agent_message", role: "assistant" }),
    ];
    const fresh = [
      sessionEvent("2026-06-30 10:30:00.000000", "task message", { task_id: "t1" }, { kind: "agent_message", role: "assistant" }),
    ];

    // When
    const merged = mergeEventLists(stored, fresh);

    // Then — evicted events recovered; chronological order; seq 1..N
    expect(merged.map((e) => e.content)).toEqual(["q1", "npm test", "task message"]);
    expect(merged.map((e) => e.seq)).toEqual([1, 2, 3]);
  });

  it("Given stored and fresh with interleaving timestamps and an input order that is unsorted, When mergeEventLists, Then the output is chronological regardless of input order", () => {
    // Given
    const stored = [
      sessionEvent("2026-06-30 10:30:00.000000", "c", {}, { kind: "agent_message", role: "assistant" }),
      sessionEvent("2026-06-30 10:10:00.000000", "a"),
    ];
    const fresh = [
      sessionEvent("2026-06-30 10:20:00.000000", "b", {}, { kind: "command" }),
      sessionEvent("2026-06-30 10:10:00.000000", "a"),
      sessionEvent("2026-06-30 10:30:00.000000", "c", {}, { kind: "agent_message", role: "assistant" }),
    ];

    // When
    const merged = mergeEventLists(stored, fresh);

    // Then
    expect(merged.map((e) => e.content)).toEqual(["a", "b", "c"]);
    expect(merged.map((e) => e.seq)).toEqual([1, 2, 3]);
  });
});

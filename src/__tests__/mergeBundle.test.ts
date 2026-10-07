import { describe, it, expect } from "vitest";
import { mergeBundles } from "../domain/mergeBundle.js";
import type {
  BundleSource,
  ConversationEvent,
  Label,
  CaptureBundle,
} from "../domain/models.js";

const SPEC = "2026-07-14-foo";

function event(
  conversationId: string,
  label: Label,
  ts: string,
  content: string,
  seq: number
): ConversationEvent {
  return {
    capture_id: SPEC,
    label,
    conversation_id: conversationId,
    seq,
    ts,
    role: "user",
    kind: "query",
    content,
    meta: {},
  };
}

/** Build a bundle whose header is structurally valid (merge recomputes it). */
function bundle(
  events: ConversationEvent[],
  captureId = SPEC,
  source: BundleSource = "warp"
): CaptureBundle {
  return {
    header: {
      type: "bundle_header",
      capture_id: captureId,
      labels: [],
      conversations_per_label: {},
      conversation_ids: [],
      extracted_at: "2026-07-14T00:00:00.000Z",
      source,
    },
    events,
  };
}

describe("§9.16 mergeBundles decay-safe merge", () => {
  it("Given a stored specify bundle and a fresh implement bundle (specify decayed live), When merged, Then both labels are present and specify is not lost", () => {
    // Given — specify captured at close (stored); implement just closed fresh;
    // specify's marker decayed so it is absent from the fresh read.
    const stored = bundle([
      event("c1", "specify", "2026-07-14 09:00:00.000000", "spec it", 1),
    ]);
    const fresh = bundle([
      event("c2", "implement", "2026-07-14 10:00:00.000000", "implement it", 1),
    ]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then — both labels survive; specify recovered from disk, implement from live
    expect(merged.header.labels).toEqual(["specify", "implement"]);
    expect(merged.events).toHaveLength(2);
    expect(merged.events.map((e) => e.label)).toEqual(["specify", "implement"]);
  });

  it("Given a stored bundle and a fresh re-read of the same conversations, When merged, Then overlapping events dedup by their natural key and no duplicates appear", () => {
    // Given — both bundles carry the same specify + implement events (re-read,
    // not yet decayed). seq differs between the two copies (per-run).
    const specifyEvents = [
      event("c1", "specify", "2026-07-14 09:00:00.000000", "spec it", 1),
      event("c1", "specify", "2026-07-14 09:10:00.000000", "spec more", 2),
    ];
    const implementEvents = [
      event("c2", "implement", "2026-07-14 10:00:00.000000", "implement it", 1),
    ];
    const stored = bundle([...specifyEvents, ...implementEvents]);
    // fresh re-read: same content, different seq assignment
    const fresh = bundle([
      event("c1", "specify", "2026-07-14 09:00:00.000000", "spec it", 1),
      event("c1", "specify", "2026-07-14 09:10:00.000000", "spec more", 2),
      event("c2", "implement", "2026-07-14 10:00:00.000000", "implement it", 3),
    ]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then — exactly 3 events (no duplicates), header recomputed from merged set
    expect(merged.events).toHaveLength(3);
    const keys = merged.events.map((e) => `${e.conversation_id}:${e.ts}:${e.content}`);
    expect(new Set(keys).size).toBe(3);
    expect(merged.header.labels).toEqual(["specify", "implement"]);
    expect(merged.header.conversations_per_label).toEqual({
      specify: 1,
      implement: 1,
    });
  });

  it("Given a stored event and a fresh re-read with a later mutable timestamp, When merged, Then one event survives with the earlier timestamp", () => {
    // Given
    const storedEvent = event(
      "c1",
      "review",
      "2026-07-14 09:00:00.000000",
      "same task event",
      1
    );
    storedEvent.role = "assistant";
    storedEvent.kind = "agent_message";
    storedEvent.meta = { field_path: "messages.agent_output.text", repeat: 1 };
    const freshEvent = event(
      "c1",
      "review",
      "2026-07-14 10:00:00.000000",
      "same task event",
      1
    );
    freshEvent.role = "assistant";
    freshEvent.kind = "agent_message";
    freshEvent.meta = { field_path: "messages.agent_output.text", repeat: 2 };
    const stored = bundle([storedEvent]);
    const fresh = bundle([freshEvent]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then
    expect(merged.events).toHaveLength(1);
    expect(merged.events[0]!.ts).toBe("2026-07-14 09:00:00.000000");
    expect(merged.events[0]!.meta.repeat).toBe(2);
  });

  it("Given an evicted stored query and a distinct fresh query with identical content, When merged, Then stable timestamps keep both events", () => {
    // Given
    const storedEvent = event(
      "c1",
      "review",
      "2026-07-14 09:00:00.000000",
      "continue",
      1
    );
    storedEvent.meta = { cwd: "/repo", model: "model-a" };
    const freshEvent = event(
      "c1",
      "review",
      "2026-07-14 10:00:00.000000",
      "continue",
      1
    );
    freshEvent.meta = { cwd: "/repo", model: "model-a" };

    // When
    const merged = mergeBundles(bundle([storedEvent]), bundle([freshEvent]));

    // Then
    expect(merged.events).toHaveLength(2);
    expect(merged.events.map((event) => event.ts)).toEqual([
      "2026-07-14 09:00:00.000000",
      "2026-07-14 10:00:00.000000",
    ]);
  });

  it("Given a legacy stored task event and a fresh identified re-read with a later timestamp, When merged, Then compatibility matching preserves one earliest event", () => {
    // Given
    const storedEvent = event(
      "c1",
      "review",
      "2026-07-14 09:00:00.000000",
      "same task event",
      1
    );
    storedEvent.role = "assistant";
    storedEvent.kind = "agent_message";
    storedEvent.meta = { field_path: "messages.agent_output.text" };
    const freshEvent = event(
      "c1",
      "review",
      "2026-07-14 10:00:00.000000",
      "same task event",
      1
    );
    freshEvent.role = "assistant";
    freshEvent.kind = "agent_message";
    freshEvent.meta = {
      field_path: "messages.agent_output.text",
      task_id: "task-1",
      task_event_index: 4,
    };

    // When
    const merged = mergeBundles(bundle([storedEvent]), bundle([freshEvent]));

    // Then
    expect(merged.events).toHaveLength(1);
    expect(merged.events[0]!.ts).toBe("2026-07-14 09:00:00.000000");
    expect(merged.events[0]!.meta.task_id).toBe("task-1");
    expect(merged.events[0]!.meta.task_event_index).toBe(4);
  });

  it("Given two same-content events with distinct stable surrogates, When merged, Then both survive", () => {
    // Given
    const first = event(
      "c1",
      "review",
      "2026-07-14 09:00:00.000000",
      "continue",
      1
    );
    first.meta = { message_id: 41, message_event_index: 0 };
    const second = event(
      "c1",
      "review",
      "2026-07-14 09:01:00.000000",
      "continue",
      2
    );
    second.meta = { message_id: 42, message_event_index: 0 };

    // When
    const merged = mergeBundles(bundle([first]), bundle([second]));

    // Then
    expect(merged.events).toHaveLength(2);
    expect(merged.events.map((e) => e.meta.message_id)).toEqual([41, 42]);
  });

  it("Given a label bindable fresh but some older events evicted from the live ring buffer, When merged, Then the evicted events are recovered from the stored bundle (fresh primary, stored fills gaps)", () => {
    // Given — specify captured at close with 3 prompts; since then 2 were
    // evicted from the live ring buffer, so the fresh read only has 1.
    const stored = bundle([
      event("c1", "specify", "2026-07-14 09:00:00.000000", "prompt one", 1),
      event("c1", "specify", "2026-07-14 09:10:00.000000", "prompt two", 2),
      event("c1", "specify", "2026-07-14 09:20:00.000000", "prompt three", 3),
    ]);
    const fresh = bundle([
      event("c1", "specify", "2026-07-14 09:20:00.000000", "prompt three", 1),
    ]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then — all 3 prompts present: the fresh one + the 2 evicted recovered
    expect(merged.events).toHaveLength(3);
    expect(merged.events.map((e) => e.content)).toEqual([
      "prompt one",
      "prompt two",
      "prompt three",
    ]);
  });

  it("Given merged events out of insertion order, When merged, Then they are sorted by ts with a monotonic 1..N seq", () => {
    // Given — stored specify (later ts) + fresh implement (earlier ts)
    const stored = bundle([
      event("c1", "specify", "2026-07-14 11:00:00.000000", "late specify", 1),
    ]);
    const fresh = bundle([
      event("c2", "implement", "2026-07-14 10:00:00.000000", "early implement", 1),
    ]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then — time-ordered, monotonic seq
    const ts = merged.events.map((e) => e.ts);
    expect([...ts].sort()).toEqual(ts);
    expect(merged.events.map((e) => e.seq)).toEqual([1, 2]);
  });

  it("Given a complete spec across all three labels split across stored and fresh, When merged, Then the header tallies all three labels", () => {
    // Given — specify + implement stored (captured at close); gate just closed
    // fresh. All three labels now present in the merged set.
    const stored = bundle([
      event("c1", "specify", "2026-07-14 09:00:00.000000", "spec", 1),
      event("c2", "implement", "2026-07-14 10:00:00.000000", "impl", 2),
    ]);
    const fresh = bundle([
      event("c3", "review", "2026-07-14 11:00:00.000000", "gate", 1),
    ]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then
    expect(merged.header.labels).toEqual([
      "specify",
      "implement",
      "review",
    ]);
    expect(merged.header.conversation_ids).toEqual(["c1", "c2", "c3"]);
  });

  it("Given an empty stored bundle and a non-empty fresh bundle, When merged, Then the result equals the fresh events (re-seq'd)", () => {
    // Given
    const stored = bundle([]);
    const fresh = bundle([
      event("c1", "specify", "2026-07-14 09:00:00.000000", "spec", 5),
    ]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then — fresh events kept, seq reassigned from 1
    expect(merged.events).toHaveLength(1);
    expect(merged.events[0]!.content).toBe("spec");
    expect(merged.events[0]!.seq).toBe(1);
  });

  it("Given a non-empty stored bundle and an empty fresh bundle, When merged, Then the stored events are preserved (re-seq'd)", () => {
    // Given — fresh read bound nothing (e.g. all decayed), stored has the goods
    const stored = bundle([
      event("c1", "specify", "2026-07-14 09:00:00.000000", "spec", 7),
    ]);
    const fresh = bundle([]);

    // When
    const merged = mergeBundles(stored, fresh);

    // Then — stored events survive, seq reassigned from 1
    expect(merged.events).toHaveLength(1);
    expect(merged.events[0]!.content).toBe("spec");
    expect(merged.events[0]!.seq).toBe(1);
  });
});

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createFixture, seedMarker, seedQuery, seedBlock, seedTask } from "./fixtures/fixtureDb.js";
import { fakeSource } from "./fixtures/fakeSource.js";
import { encodeString } from "./fixtures/protobuf.js";
import { extractBundle } from "../usecases/extractBundle.js";
import { WarpConversationReader } from "../adapters/warpConversationReader.js";
import type { ConversationReader, CaptureRead } from "../domain/ports.js";
import type { ConversationEvent, EventDraft, Label, CaptureBundle } from "../domain/models.js";
import { computeBundleHeader } from "../domain/bundleHeader.js";

const SPEC = "add-feature-x";

function taskBlob(text: string): Buffer {
  return encodeString(1, text);
}

describe("§9.7 extractBundle tally", () => {
  let tmp: string;
  let dbPath: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-97-"));
    dbPath = path.join(tmp, "f.db");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a spec with all three labels, When extracted, Then the header tallies all three labels", () => {
    // Given — one conversation per label, each with a marker + a query
    const db = createFixture(dbPath);
    seedMarker(db, { capture_id: SPEC, label: "specify", conversation_id: "c1", start_ts: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 10:05:00.000000", text: "spec it" });
    seedMarker(db, { capture_id: SPEC, label: "implement", conversation_id: "c2", start_ts: "2026-06-30 11:00:00.000000" });
    seedQuery(db, { conversation_id: "c2", start_ts: "2026-06-30 11:05:00.000000", text: "implement it" });
    seedMarker(db, { capture_id: SPEC, label: "review", conversation_id: "c3", start_ts: "2026-06-30 12:00:00.000000" });
    seedQuery(db, { conversation_id: "c3", start_ts: "2026-06-30 12:05:00.000000", text: "gate it" });

    // When
    const { bundle } = extractBundle(new WarpConversationReader(fakeSource(db)), SPEC);

    // Then
    expect(bundle).not.toBeNull();
    expect(bundle!.header.labels).toEqual(["specify", "implement", "review"]);
    expect(bundle!.header.conversations_per_label).toEqual({
      specify: 1,
      implement: 1,
      "review": 1,
    });
    db.close();
  });

  it("Given a spec missing a label plus an unbindable marker, When extracted, Then the labels present are tallied and the warning is surfaced", () => {
    // Given — specify + implement only, and one unbindable marker (no binding block)
    const db = createFixture(dbPath);
    seedMarker(db, { capture_id: SPEC, label: "specify", conversation_id: "c1", start_ts: "2026-06-30 10:00:00.000000" });
    seedMarker(db, { capture_id: SPEC, label: "implement", conversation_id: "c2", start_ts: "2026-06-30 11:00:00.000000" });
    db.prepare(`INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)`).run(
      `: CAPTURE_MARKER v=1 id=${SPEC} label=review`,
      "2026-06-30 12:00:00.000000"
    );

    // When
    const { bundle, summary } = extractBundle(new WarpConversationReader(fakeSource(db)), SPEC);

    // Then — incomplete, missing label reported, unbindable marker surfaced as a warning
    expect(bundle).not.toBeNull();
    expect(summary.unbindable).toHaveLength(1);
    expect(summary.unbindable[0]!.label).toBe("review");
    db.close();
  });

  it("Given re-runs of a label (two conversations), When extracted, Then both are kept", () => {
    // Given — two review conversations for the same spec
    const db = createFixture(dbPath);
    seedMarker(db, { capture_id: SPEC, label: "specify", conversation_id: "c1", start_ts: "2026-06-30 10:00:00.000000" });
    seedMarker(db, { capture_id: SPEC, label: "implement", conversation_id: "c2", start_ts: "2026-06-30 11:00:00.000000" });
    seedMarker(db, { capture_id: SPEC, label: "review", conversation_id: "c3", start_ts: "2026-06-30 12:00:00.000000" });
    seedMarker(db, { capture_id: SPEC, label: "review", conversation_id: "c4", start_ts: "2026-06-30 13:00:00.000000" });

    // When
    const { bundle, summary } = extractBundle(new WarpConversationReader(fakeSource(db)), SPEC);

    // Then — both re-run conversations kept, still complete
    expect(bundle).not.toBeNull();
    expect(bundle!.header.conversations_per_label["review"]).toBe(2);
    expect(bundle!.header.conversation_ids).toEqual(["c1", "c2", "c3", "c4"]);
    expect(summary.conversations).toBe(4);
    db.close();
  });
});

describe("§9.8 extractBundle ordering", () => {
  let tmp: string;
  let dbPath: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-98-"));
    dbPath = path.join(tmp, "f.db");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given events across labels at various timestamps, When extracted, Then they are ordered by start_ts with a monotonic seq and the correct label per conversation", () => {
    // Given — out-of-insertion-order timestamps across all three labels and all reader kinds
    const db = createFixture(dbPath);
    // specify conversation c1
    seedMarker(db, { capture_id: SPEC, label: "specify", conversation_id: "c1", start_ts: "2026-06-30 09:00:00.000000" });
    seedTask(db, { conversation_id: "c1", task: taskBlob("spec task"), last_modified_at: "2026-06-30 09:30:00.000000" });
    seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 09:10:00.000000", text: "spec query" });
    // implement conversation c2
    seedMarker(db, { capture_id: SPEC, label: "implement", conversation_id: "c2", start_ts: "2026-06-30 10:00:00.000000" });
    seedBlock(db, { conversation_id: "c2", start_ts: "2026-06-30 10:20:00.000000", command: "npm test" });
    seedQuery(db, { conversation_id: "c2", start_ts: "2026-06-30 10:10:00.000000", text: "impl query" });
    // gate conversation c3
    seedMarker(db, { capture_id: SPEC, label: "review", conversation_id: "c3", start_ts: "2026-06-30 11:00:00.000000" });
    seedQuery(db, { conversation_id: "c3", start_ts: "2026-06-30 11:10:00.000000", text: "gate query" });

    // When
    const { bundle } = extractBundle(new WarpConversationReader(fakeSource(db)), SPEC);

    // Then — non-decreasing timestamps and a 1..N monotonic sequence
    const events = bundle!.events;
    const ts = events.map((e) => e.ts);
    const seq = events.map((e) => e.seq);
    expect([...ts].sort()).toEqual(ts); // sorted ascending
    expect(seq).toEqual(events.map((_, i) => i + 1)); // 1..N
    expect(new Set(seq).size).toBe(seq.length); // unique
    // label matches the conversation that emitted each event
    const labelOf = new Map([["c1", "specify"], ["c2", "implement"], ["c3", "review"]]);
    for (const e of events) {
      expect(e.label).toBe(labelOf.get(e.conversation_id));
    }
    db.close();
  });
});

// --- helpers for merge / fallback tests (no fixture DB needed) ---

function specReadWith(
  label: Label,
  conversationId: string,
  prompts: { ts: string; content: string }[]
): CaptureRead {
  const labelByCid = new Map<string, Label>([[conversationId, label]]);
  const drafts: EventDraft[] = prompts.map((p) => ({
    conversation_id: conversationId,
    ts: p.ts,
    role: "user",
    kind: "query",
    content: p.content,
    meta: {},
  }));
  return {
    source: "warp",
    labelByCid,
    drafts,
    skipped: [],
    unbindable: [],
    collisions: [],
    heuristic_bindings: [],
  };
}

function fakeReader(read: CaptureRead): ConversationReader {
  return { readCapture: () => read };
}

function throwingReader(error: Error): ConversationReader {
  return { readCapture: () => { throw error; } };
}

function storedBundle(
  label: Label,
  conversationId: string,
  prompts: { ts: string; content: string }[]
): CaptureBundle {
  const labelByCid = new Map<string, Label>([[conversationId, label]]);
  const events: ConversationEvent[] = prompts.map((p, i) => ({
    capture_id: SPEC,
    label,
    conversation_id: conversationId,
    seq: i + 1,
    ts: p.ts,
    role: "user",
    kind: "query",
    content: p.content,
    meta: {},
  }));
  const headerFields = computeBundleHeader(SPEC, "warp", labelByCid);
  return {
    header: { type: "bundle_header", ...headerFields, extracted_at: "2026-07-14T00:00:00.000Z" },
    events,
  };
}

describe("§9.17 extractBundle merge + fresh-read-error fallback", () => {
  it("Given a stored specify bundle and a fresh implement read, When extracted with existingBundle, Then the merged result has both labels and no fresh_read_error", () => {
    // Given — specify captured at close (stored); fresh read has implement only
    const existing = storedBundle("specify", "c1", [
      { ts: "2026-07-14 09:00:00.000000", content: "spec it" },
    ]);
    const reader = fakeReader(
      specReadWith("implement", "c2", [{ ts: "2026-07-14 10:00:00.000000", content: "implement it" }])
    );

    // When
    const { bundle, summary } = extractBundle(reader, SPEC, { existingBundle: existing });

    // Then — both labels present (specify from disk, implement from live)
    expect(bundle).not.toBeNull();
    expect(bundle!.header.labels).toEqual(["specify", "implement"]);
    expect(summary.fresh_read_error).toBeNull();
    expect(bundle!.events).toHaveLength(2);
  });

  it("Given a reader that throws and a stored bundle, When extracted, Then it degrades to stored-only with a fresh_read_error warning", () => {
    // Given — fresh read fails (DB locked); stored bundle has specify + implement
    const existing = storedBundle("specify", "c1", [
      { ts: "2026-07-14 09:00:00.000000", content: "spec it" },
    ]);
    const reader = throwingReader(new Error("database is locked"));

    // When
    const { bundle, summary } = extractBundle(reader, SPEC, { existingBundle: existing });

    // Then — stored bundle preserved intact, fresh_read_error set
    expect(bundle).not.toBeNull();
    expect(summary.fresh_read_error).toContain("database is locked");
    expect(bundle!.events).toHaveLength(existing.events.length);
    expect(bundle!.header.labels).toEqual(existing.header.labels);
  });

  it("Given a reader that throws and no stored bundle, When extracted, Then it throws (first capture cannot recover)", () => {
    // Given — fresh read fails, nothing on disk to fall back on
    const reader = throwingReader(new Error("database is locked"));

    // When / Then
    expect(() => extractBundle(reader, SPEC)).toThrow("fresh read failed");
  });

  it("Given an existing bundle and --no-merge, When extracted, Then the result is fresh only (stored labels not recovered)", () => {
    // Given — stored specify; fresh read has implement; --no-merge replaces
    const existing = storedBundle("specify", "c1", [
      { ts: "2026-07-14 09:00:00.000000", content: "spec it" },
    ]);
    const reader = fakeReader(
      specReadWith("implement", "c2", [{ ts: "2026-07-14 10:00:00.000000", content: "implement it" }])
    );

    // When
    const { bundle } = extractBundle(reader, SPEC, {
      existingBundle: existing,
      noMerge: true,
    });

    // Then — fresh only: specify NOT recovered (replaced, not merged)
    expect(bundle).not.toBeNull();
    expect(bundle!.header.labels).toEqual(["implement"]);
    expect(bundle!.events).toHaveLength(1);
  });
});

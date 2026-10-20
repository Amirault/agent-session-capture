import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  createFixture,
  seedConversation,
  seedQuery,
  seedBlock,
  seedTask,
  type FixtureDb,
} from "./fixtures/fixtureDb.js";
import { fakeSource } from "./fixtures/fakeSource.js";
import { encodeString } from "./fixtures/protobuf.js";
import { WarpConversationReader } from "../adapters/warpConversationReader.js";
import type { SessionHeader } from "../domain/models.js";

function storedHeader(conversationId: string, lastModified: string | null): SessionHeader {
  return {
    type: "session_header",
    conversation_id: conversationId,
    source: "warp",
    last_modified: lastModified,
    extracted_at: "2026-07-01T00:00:00.000Z",
  };
}

describe("§9.19 WarpConversationReader.readSessions — enumeration, filters, skip-unchanged", () => {
  let tmp: string;
  let dbPath: string;
  let db: FixtureDb;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-919-"));
    dbPath = path.join(tmp, "f.db");
    db = createFixture(dbPath);
  });
  afterEach(() => {
    db.close();
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given conversations split across agent_conversations and agent_tasks, When readSessions, Then enumeration unions both durable tables and takes the MAX stamp (a tasks-only conversation is listed)", async () => {
    // Given — c1 in both tables (tasks stamp later), c2 conversations-only, c3 tasks-only
    seedConversation(db, { conversation_id: "c1", last_modified_at: "2026-06-30 10:00:00.000000" });
    seedTask(db, { conversation_id: "c1", task: encodeString(1, "c1 task"), last_modified_at: "2026-06-30 10:30:00.000000" });
    seedConversation(db, { conversation_id: "c2", last_modified_at: "2026-07-01 09:00:00.000000" });
    seedTask(db, { conversation_id: "c3", task: encodeString(1, "c3 task"), last_modified_at: "2026-07-02 08:00:00.000000" });

    // When
    const read = await new WarpConversationReader(fakeSource(db)).readSessions({ storedByCid: new Map() });

    // Then
    expect(read.source).toBe("warp");
    const byCid = new Map(read.sessions.map((s) => [s.conversation_id, s.last_modified]));
    expect(byCid.get("c1")).toBe("2026-06-30 10:30:00.000000"); // MAX across both tables
    expect(byCid.get("c2")).toBe("2026-07-01 09:00:00.000000");
    expect(byCid.get("c3")).toBe("2026-07-02 08:00:00.000000"); // listed without an agent_conversations row
  });

  it("Given conversations with different last activity, When readSessions with --since, Then only conversations at/after the cutoff are enumerated", async () => {
    // Given
    seedConversation(db, { conversation_id: "c1", last_modified_at: "2026-06-30 10:00:00.000000" });
    seedConversation(db, { conversation_id: "c2", last_modified_at: "2026-07-15 09:00:00.000000" });
    const reader = new WarpConversationReader(fakeSource(db));

    // When
    const afterJuly = await reader.readSessions({ since: "2026-07-01", storedByCid: new Map() });
    const atBoundary = await reader.readSessions({ since: "2026-07-15T09:00:00Z", storedByCid: new Map() });
    const afterJune = await reader.readSessions({ since: "2026-06-01", storedByCid: new Map() });

    // Then
    expect(afterJuly.sessions.map((s) => s.conversation_id)).toEqual(["c2"]);
    expect(atBoundary.sessions.map((s) => s.conversation_id)).toEqual(["c2"]); // at/after, inclusive
    expect(afterJune.sessions.map((s) => s.conversation_id)).toEqual(["c1", "c2"]);
  });

  it("Given one conversation with query/block/task rows and other conversations present, When readSessions, Then gather collects queries + blocks + tasks for that cid only (subagent tasks share the parent cid), with no marker anywhere in the fixture", async () => {
    // Given — c1 with all three event kinds, its task row tagged like a subagent task; c2 separate
    seedConversation(db, { conversation_id: "c1", last_modified_at: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 10:10:00.000000", text: "fix the login" });
    seedBlock(db, { conversation_id: "c1", start_ts: "2026-06-30 10:20:00.000000", command: "npm test" });
    seedTask(db, { conversation_id: "c1", task_id: "sub-1", task: encodeString(1, "subagent message"), last_modified_at: "2026-06-30 10:30:00.000000" });
    seedConversation(db, { conversation_id: "c2", last_modified_at: "2026-07-01 09:00:00.000000" });
    seedQuery(db, { conversation_id: "c2", start_ts: "2026-07-01 09:10:00.000000", text: "other conversation" });

    // When
    const read = await new WarpConversationReader(fakeSource(db)).readSessions({ storedByCid: new Map() });

    // Then — no marker rows exist anywhere; both conversations still enumerate and gather
    const markerRows = db
      .prepare("SELECT COUNT(*) AS n FROM commands WHERE command LIKE '%CAPTURE_MARKER%'")
      .get() as { n: number };
    expect(markerRows.n).toBe(0);

    const c1 = read.reads.get("c1")!;
    expect(c1.drafts.map((d) => d.kind)).toEqual(["query", "command", "agent_message"]); // fixed reader order
    expect(c1.drafts.every((d) => d.conversation_id === "c1")).toBe(true); // only c1's rows
    expect(c1.drafts[2]!.meta.task_id).toBe("sub-1"); // the subagent task rolls up via the shared cid
    expect(c1.last_modified).toBe("2026-06-30 10:30:00.000000");

    const c2 = read.reads.get("c2")!;
    expect(c2.drafts.map((d) => [d.conversation_id, d.kind, d.content])).toEqual([
      ["c2", "query", "other conversation"],
    ]);
    expect(read.reads.size).toBe(2);
  });

  it("Given stored headers by cid, When readSessions, Then only changed or new conversations are gathered (raw stamp equality) and --conversation forces a gather despite an equal stamp", async () => {
    // Given — c1 stored with an equal stamp, c2 stored with an older stamp, c3 not stored
    seedConversation(db, { conversation_id: "c1", last_modified_at: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 10:10:00.000000", text: "same" });
    seedConversation(db, { conversation_id: "c2", last_modified_at: "2026-07-01 09:00:00.000000" });
    seedQuery(db, { conversation_id: "c2", start_ts: "2026-07-01 09:10:00.000000", text: "changed" });
    seedConversation(db, { conversation_id: "c3", last_modified_at: "2026-07-02 09:00:00.000000" });
    seedQuery(db, { conversation_id: "c3", start_ts: "2026-07-02 09:10:00.000000", text: "new" });
    const storedByCid = new Map<string, SessionHeader>([
      ["c1", storedHeader("c1", "2026-06-30 10:00:00.000000")],
      ["c2", storedHeader("c2", "2026-06-30 08:00:00.000000")],
    ]);

    // When
    const read = await new WarpConversationReader(fakeSource(db)).readSessions({ storedByCid });

    // Then — equal stamp skipped, different and new gathered
    expect(read.reads.has("c1")).toBe(false);
    expect(read.reads.has("c2")).toBe(true);
    expect(read.reads.has("c3")).toBe(true);

    // And — the --conversation debug filter always gathers its conversation
    const forced = await new WarpConversationReader(fakeSource(db)).readSessions({
      conversation: "c1",
      storedByCid,
    });
    expect(forced.sessions.map((s) => s.conversation_id)).toEqual(["c1"]);
    expect(forced.reads.has("c1")).toBe(true);
  });
});

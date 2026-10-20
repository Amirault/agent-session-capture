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
import { extractSessions } from "../usecases/extractSessions.js";
import { WarpConversationReader } from "../adapters/warpConversationReader.js";
import { sessionFileName } from "../adapters/sessionFileName.js";
import type { SessionEvent, SessionHeader } from "../domain/models.js";

/** c1: conversation + query + block + task; c2: conversation + query; c3: tasks only. */
function seedThreeConversations(db: FixtureDb): void {
  seedConversation(db, { conversation_id: "c1", last_modified_at: "2026-06-30 10:00:00.000000" });
  seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 10:10:00.000000", text: "fix the login" });
  seedBlock(db, { conversation_id: "c1", start_ts: "2026-06-30 10:20:00.000000", command: "npm test" });
  seedTask(db, { conversation_id: "c1", task: encodeString(1, "task message"), last_modified_at: "2026-06-30 10:30:00.000000" });
  seedConversation(db, { conversation_id: "c2", last_modified_at: "2026-07-01 09:00:00.000000" });
  seedQuery(db, { conversation_id: "c2", start_ts: "2026-07-01 09:10:00.000000", text: "second session" });
  seedTask(db, { conversation_id: "c3", task: encodeString(1, "task only"), last_modified_at: "2026-07-02 08:00:00.000000" });
}

function sessionsDirOf(outDir: string): string {
  return path.join(outDir, "sessions");
}

function readSessionFile(outDir: string, cid: string): { header: SessionHeader; events: SessionEvent[] } {
  const raw = fs.readFileSync(path.join(sessionsDirOf(outDir), `${sessionFileName(cid)}.jsonl`), "utf8");
  const lines = raw.split("\n").slice(0, -1); // drop trailing "" from final newline
  return {
    header: JSON.parse(lines[0]!) as SessionHeader,
    events: lines.slice(1).map((l) => JSON.parse(l) as SessionEvent),
  };
}

/** {fileName -> {content, mtimeMs}} — proves content equality, not just mtime. */
function snapshotSessionsDir(sessionsDir: string): Record<string, { content: string; mtimeMs: number }> {
  const snapshot: Record<string, { content: string; mtimeMs: number }> = {};
  if (!fs.existsSync(sessionsDir)) return snapshot;
  for (const name of fs.readdirSync(sessionsDir).sort()) {
    snapshot[name] = {
      content: fs.readFileSync(path.join(sessionsDir, name), "utf8"),
      mtimeMs: fs.statSync(path.join(sessionsDir, name)).mtimeMs,
    };
  }
  return snapshot;
}

describe("§9.21 extractSessions use case — incremental, decay-safe session export", () => {
  let tmp: string;
  let dbPath: string;
  let outDir: string;
  let db: FixtureDb;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-921-"));
    dbPath = path.join(tmp, "f.db");
    outDir = path.join(tmp, "out");
    db = createFixture(dbPath);
    seedThreeConversations(db);
  });
  afterEach(() => {
    db.close();
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a store with three conversations and no markers, When extractSessions runs on a fresh outDir, Then one file per conversation is written with a session_header on line 1, chronological events, seq 1..N, and no capture_id/label", async () => {
    // Given — fixture seeded in beforeEach; no CAPTURE_MARKER anywhere

    // When
    const { summary } = await extractSessions(new WarpConversationReader(fakeSource(db)), { outDir, source: "warp" });

    // Then — accounting
    expect(summary.enumerated).toBe(3);
    expect(summary.written.map((w) => w.conversation_id).sort()).toEqual(["c1", "c2", "c3"]);
    expect(summary.unchanged).toEqual([]);
    expect(summary.untouched).toEqual([]);
    expect(summary.warnings).toEqual([]);

    // one file per conversation, in the sessions/ subdir
    expect(fs.readdirSync(sessionsDirOf(outDir)).sort()).toEqual(["c1.jsonl", "c2.jsonl", "c3.jsonl"]);
    expect(summary.written[0]!.path.endsWith(path.join("sessions", "c1.jsonl"))).toBe(true);

    // line 1 is the session_header; events chronological, seq 1..N, no capture_id/label
    const c1 = readSessionFile(outDir, "c1");
    expect(c1.header.type).toBe("session_header");
    expect(c1.header.conversation_id).toBe("c1");
    expect(c1.header.source).toBe("warp");
    expect(c1.header.last_modified).toBe("2026-06-30 10:30:00.000000"); // MAX over conversations+tasks
    expect(c1.header.extracted_at).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
    expect(c1.events.map((e) => e.kind)).toEqual(["query", "command", "agent_message"]);
    const ts = c1.events.map((e) => e.ts);
    expect([...ts].sort()).toEqual(ts);
    expect(c1.events.map((e) => e.seq)).toEqual(c1.events.map((_, i) => i + 1));
    for (const e of c1.events) {
      expect("capture_id" in e).toBe(false);
      expect("label" in e).toBe(false);
      expect(e.conversation_id).toBe("c1");
    }

    // the tasks-only conversation is exported too
    const c3 = readSessionFile(outDir, "c3");
    expect(c3.header.last_modified).toBe("2026-07-02 08:00:00.000000");
    expect(c3.events.map((e) => e.kind)).toEqual(["agent_message"]);
  });

  it("Given a completed first run, When extractSessions re-runs with no store changes, Then nothing is rewritten (content and mtimes identical) and all sessions are reported unchanged", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    const before = snapshotSessionsDir(sessionsDirOf(outDir));

    // When
    const { summary } = await extractSessions(reader, { outDir, source: "warp" });

    // Then — no gather, no write, byte-identical files
    expect(summary.written).toEqual([]);
    expect(summary.unchanged.sort()).toEqual(["c1", "c2", "c3"]);
    expect(summary.untouched).toEqual([]);
    expect(snapshotSessionsDir(sessionsDirOf(outDir))).toEqual(before);
  });

  it("Given a completed first run, When one conversation's stamp is bumped, Then only that session file is rewritten and the others stay byte-identical", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    const before = snapshotSessionsDir(sessionsDirOf(outDir));

    // When — c1's task row is touched (mutable last_modified_at)
    db.prepare("UPDATE agent_tasks SET last_modified_at = ? WHERE conversation_id = ?")
      .run("2026-06-30 11:00:00.000000", "c1");
    const { summary } = await extractSessions(reader, { outDir, source: "warp" });

    // Then — exactly the changed session is rewritten
    expect(summary.written.map((w) => w.conversation_id)).toEqual(["c1"]);
    expect(summary.unchanged.sort()).toEqual(["c2", "c3"]);
    const after = snapshotSessionsDir(sessionsDirOf(outDir));
    expect(after["c1.jsonl"]!.mtimeMs).toBeGreaterThan(before["c1.jsonl"]!.mtimeMs);
    expect(after["c2.jsonl"]).toEqual(before["c2.jsonl"]);
    expect(after["c3.jsonl"]).toEqual(before["c3.jsonl"]);

    // the rewritten header carries the new stamp; the re-read task event dedups
    // against the stored one (mutable-ts reconciliation, earliest ts kept)
    const c1 = readSessionFile(outDir, "c1");
    expect(c1.header.last_modified).toBe("2026-06-30 11:00:00.000000");
    const taskEvents = c1.events.filter((e) => e.kind === "agent_message");
    expect(taskEvents).toHaveLength(1);
    expect(taskEvents[0]!.ts).toBe("2026-06-30 10:30:00.000000");
    expect(c1.events).toHaveLength(3);
  });

  it("Given a completed first run, When a conversation's ai_queries rows are evicted and its stamp bumped, Then the evicted query events survive in the merged file (decay-safe)", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    expect(readSessionFile(outDir, "c1").events.map((e) => e.kind)).toEqual(["query", "command", "agent_message"]);

    // When — Warp evicts c1's queries; the conversations stamp changes anyway
    db.prepare("DELETE FROM ai_queries WHERE conversation_id = ?").run("c1");
    db.prepare("UPDATE agent_conversations SET last_modified_at = ? WHERE conversation_id = ?")
      .run("2026-06-30 12:00:00.000000", "c1");
    const { summary } = await extractSessions(reader, { outDir, source: "warp" });

    // Then — c1 re-gathered and re-merged: the evicted query is recovered from
    // the stored file; the fresh block + task dedup; nothing duplicated
    expect(summary.written.map((w) => w.conversation_id)).toEqual(["c1"]);
    const c1 = readSessionFile(outDir, "c1");
    expect(c1.events).toHaveLength(3);
    expect(c1.events.some((e) => e.kind === "query" && e.content === "fix the login")).toBe(true);
    expect(c1.events.some((e) => e.kind === "command" && e.content === "npm test")).toBe(true);
    expect(c1.events.some((e) => e.kind === "agent_message" && e.content === "task message")).toBe(true);
    expect(c1.events.map((e) => e.seq)).toEqual([1, 2, 3]);
  });

  it("Given a completed first run, When extractSessions re-runs with --conversation for one cid, Then only that conversation is enumerated and force-rewritten, the other stored files staying untouched", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    const before = snapshotSessionsDir(sessionsDirOf(outDir));

    // When
    const { summary } = await extractSessions(reader, { outDir, source: "warp", conversation: "c2" });

    // Then — exactly one enumerated session, force-gathered despite the equal
    // stamp; the other stored files are kept untouched, never GC'd
    expect(summary.enumerated).toBe(1);
    expect(summary.written.map((w) => w.conversation_id)).toEqual(["c2"]);
    expect(summary.unchanged).toEqual([]);
    expect(summary.untouched.map((p) => path.basename(p)).sort()).toEqual(["c1.jsonl", "c3.jsonl"]);
    const after = snapshotSessionsDir(sessionsDirOf(outDir));
    expect(after["c1.jsonl"]).toEqual(before["c1.jsonl"]);
    expect(after["c3.jsonl"]).toEqual(before["c3.jsonl"]);
    expect(readSessionFile(outDir, "c2").events).toHaveLength(1);
  });

  it("Given a corrupt stored session file, When extractSessions re-runs with the default merge, Then it throws loudly and overwrites nothing", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    const before = snapshotSessionsDir(sessionsDirOf(outDir));
    const corruptPath = path.join(sessionsDirOf(outDir), "c2.jsonl");
    fs.writeFileSync(corruptPath, "not valid json\n", "utf8");

    // When / Then — the run refuses loudly, the corrupt file is left as-is
    await expect(extractSessions(reader, { outDir, source: "warp" })).rejects.toThrow(/corrupt session file/);
    expect(fs.readFileSync(corruptPath, "utf8")).toBe("not valid json\n");
    const after = snapshotSessionsDir(sessionsDirOf(outDir));
    expect(after["c2.jsonl"]!.content).toBe("not valid json\n");
    expect(after["c1.jsonl"]).toEqual(before["c1.jsonl"]);
  });

  it("Given a corrupt stored session file and --no-merge, When extractSessions re-runs, Then the file is replaced and the corruption is warned about one-per-line", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    fs.writeFileSync(path.join(sessionsDirOf(outDir), "c2.jsonl"), "not valid json\n", "utf8");

    // When
    const { summary } = await extractSessions(reader, { outDir, source: "warp", noMerge: true });

    // Then — the corrupt entry is replaced by a fresh, valid write
    expect(summary.warnings.join("\n")).toContain("corrupt session file");
    expect(summary.warnings.some((w) => w.startsWith("replacing stored session (--no-merge): "))).toBe(true);
    expect(summary.written.map((w) => w.conversation_id)).toEqual(["c2"]);
    expect(summary.unchanged.sort()).toEqual(["c1", "c3"]);
    const c2 = readSessionFile(outDir, "c2");
    expect(c2.header.type).toBe("session_header");
    expect(c2.events.map((e) => e.kind)).toEqual(["query"]);
  });

  it("Given a stored session file from another source, When extractSessions re-runs, Then the default merge refuses loudly; with --no-merge it warns and replaces", async () => {
    // Given — a hermes-written file squatting in the warp sessions dir
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    const foreignHeader = JSON.stringify({
      type: "session_header",
      conversation_id: "c2",
      source: "hermes",
      last_modified: "2026-07-01 09:00:00.000000",
      extracted_at: "2026-07-29T00:00:00.000Z",
    });
    const foreignPath = path.join(sessionsDirOf(outDir), "c2.jsonl");
    fs.writeFileSync(foreignPath, `${foreignHeader}\n`, "utf8");

    // When / Then — default: refuse, leave the file untouched
    await expect(extractSessions(reader, { outDir, source: "warp" })).rejects.toThrow(/source mismatch/);
    expect(fs.readFileSync(foreignPath, "utf8")).toBe(`${foreignHeader}\n`);

    // And with --no-merge: warn, replace with a fresh warp file
    const { summary } = await extractSessions(reader, { outDir, source: "warp", noMerge: true });
    expect(summary.warnings.join("\n")).toContain("source mismatch");
    expect(readSessionFile(outDir, "c2").header.source).toBe("warp");
  });

  it("Given a stored file whose header conversation_id does not match its filename, When extractSessions re-runs, Then it refuses loudly (collision/tampering) and touches nothing", async () => {
    // Given
    const reader = new WarpConversationReader(fakeSource(db));
    await extractSessions(reader, { outDir, source: "warp" });
    const mismatchHeader = JSON.stringify({
      type: "session_header",
      conversation_id: "c2",
      source: "warp",
      last_modified: "2026-07-01 09:00:00.000000",
      extracted_at: "2026-07-29T00:00:00.000Z",
    });
    const mismatchPath = path.join(sessionsDirOf(outDir), "not-c2.jsonl");
    fs.writeFileSync(mismatchPath, `${mismatchHeader}\n`, "utf8");

    // When / Then
    await expect(extractSessions(reader, { outDir, source: "warp" })).rejects.toThrow(/conversation mismatch/);
    expect(fs.readFileSync(mismatchPath, "utf8")).toBe(`${mismatchHeader}\n`);
  });

  it("Given a conversation id with path separators, When extractSessions runs, Then the file name is sanitized into sessions/ with no directory escape", async () => {
    // Given — an evil cid that would escape the outDir if used verbatim
    const evilCid = "../../etc/escape";
    seedConversation(db, { conversation_id: evilCid, last_modified_at: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: evilCid, start_ts: "2026-06-30 10:10:00.000000", text: "evil cid" });

    // When
    const { summary } = await extractSessions(new WarpConversationReader(fakeSource(db)), { outDir, source: "warp" });

    // Then — the evil cid is written exactly once, as a flat, safe name directly
    // in sessions/ (alongside the seeded conversations, all flat)
    const sessionsDir = sessionsDirOf(outDir);
    const writtenEvil = summary.written.filter((w) => w.conversation_id === evilCid);
    expect(writtenEvil).toHaveLength(1);
    const writtenPath = writtenEvil[0]!.path;
    expect(path.dirname(writtenPath)).toBe(sessionsDir);
    expect(path.basename(writtenPath)).toMatch(/^[A-Za-z0-9._-]+\.jsonl$/);
    expect(path.basename(writtenPath)).toMatch(/-[0-9a-f]{8}\.jsonl$/); // digest suffix
    expect(fs.readdirSync(sessionsDir).sort()).toEqual(
      ["c1.jsonl", "c2.jsonl", "c3.jsonl", path.basename(writtenPath)].sort()
    );
    // nothing outside the outDir tree
    expect(fs.existsSync(path.join(tmp, "..", "..", "etc", "escape.jsonl"))).toBe(false);
    expect(fs.existsSync(path.join(outDir, "..", "escape.jsonl"))).toBe(false);
    // the header preserves the original, unsanitized conversation id
    expect(readSessionFile(outDir, evilCid).header.conversation_id).toBe(evilCid);
  });
});

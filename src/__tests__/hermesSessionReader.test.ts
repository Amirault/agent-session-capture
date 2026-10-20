import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import Database from "better-sqlite3";
import { HermesConversationReader } from "../adapters/hermesConversationReader.js";
import { extractSessions } from "../usecases/extractSessions.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(TEST_DIR, "..", "..");
const TSX = path.join(ROOT, "node_modules", ".bin", "tsx");

type Json = Record<string, unknown>;

interface SessionRow {
  id: string;
  parent?: string;
  source?: string;
  endReason?: string;
  modelConfig?: Json;
}

/** The subset of Hermes' `state.db` schema the reader requires. */
function createHermesDb(dbPath: string): Database.Database {
  const db = new Database(dbPath);
  db.exec(`
    CREATE TABLE sessions (
      id TEXT PRIMARY KEY,
      source TEXT NOT NULL,
      model TEXT,
      parent_session_id TEXT,
      started_at REAL NOT NULL,
      ended_at REAL,
      end_reason TEXT,
      model_config TEXT NOT NULL DEFAULT '{}'
    );
    CREATE TABLE messages (
      id INTEGER PRIMARY KEY AUTOINCREMENT,
      session_id TEXT NOT NULL,
      role TEXT NOT NULL,
      content TEXT,
      tool_calls TEXT,
      tool_call_id TEXT,
      tool_name TEXT,
      timestamp REAL NOT NULL,
      active INTEGER NOT NULL DEFAULT 1,
      compacted INTEGER NOT NULL DEFAULT 0
    );
  `);
  return db;
}

/** Epoch seconds (Hermes' `timestamp`/`started_at` scale) of an ISO-8601 UTC instant. */
function at(iso: string): number {
  return Date.parse(iso) / 1000;
}

function seedSession(db: Database.Database, row: SessionRow, startedAt: string): void {
  db.prepare(
    `INSERT INTO sessions (id, source, model, parent_session_id, started_at, end_reason, model_config)
     VALUES (?, ?, 'gpt-5.6-sol', ?, ?, ?, ?)`
  ).run(
    row.id,
    row.source ?? "cli",
    row.parent ?? null,
    at(startedAt),
    row.endReason ?? null,
    JSON.stringify(row.modelConfig ?? {})
  );
}

/** One user message: maps to exactly one `query` event whose content is `content`. */
function seedUserMessage(db: Database.Database, sessionId: string, content: string, timestamp: string): void {
  db.prepare(
    `INSERT INTO messages (session_id, role, content, timestamp) VALUES (?, 'user', ?, ?)`
  ).run(sessionId, content, at(timestamp));
}

/**
 * Example 3: root `20261015_101500_ab12` (ended by compression), its
 * compression continuation `20261015_113000_cd34`, and a `/branch` child
 * `20261015_120000_ef56`.
 */
const ROOT_ID = "20261015_101500_ab12";
const CONTINUATION_ID = "20261015_113000_cd34";
const BRANCH_ID = "20261015_120000_ef56";

function seedExample3(dbPath: string): void {
  const db = createHermesDb(dbPath);
  seedSession(db, { id: ROOT_ID, endReason: "compression" }, "2026-10-15T10:15:00Z");
  seedUserMessage(db, ROOT_ID, "root question", "2026-10-15T10:15:01Z");
  seedUserMessage(db, ROOT_ID, "root follow-up", "2026-10-15T10:20:00Z");
  seedSession(db, { id: CONTINUATION_ID, parent: ROOT_ID }, "2026-10-15T11:30:00Z");
  seedUserMessage(db, CONTINUATION_ID, "continued after compression", "2026-10-15T11:30:01Z");
  seedUserMessage(db, CONTINUATION_ID, "continuation follow-up", "2026-10-15T11:45:00Z");
  seedSession(
    db,
    { id: BRANCH_ID, parent: ROOT_ID, modelConfig: { _branched_from: ROOT_ID } },
    "2026-10-15T12:00:00Z"
  );
  seedUserMessage(db, BRANCH_ID, "explore an alternative", "2026-10-15T12:00:01Z");
  db.close();
}

function readJsonl(filePath: string): Json[] {
  return fs
    .readFileSync(filePath, "utf8")
    .trimEnd()
    .split("\n")
    .map((line) => JSON.parse(line) as Json);
}

describe("HermesConversationReader.readSessions — one session per lineage root", () => {
  let tmp: string;
  let dbPath: string;
  let outDir: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-hermes-sessions-"));
    dbPath = path.join(tmp, "state.db");
    outDir = path.join(tmp, "out");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a root, its compression continuation and a /branch child, When readSessions, Then the continuation joins its root's session while the branch child is its own, each stamped with its latest message (ISO-8601 UTC)", async () => {
    // Given
    seedExample3(dbPath);

    // When
    const read = await new HermesConversationReader({ dbPath }).readSessions({ storedByCid: new Map() });

    // Then
    expect(read.source).toBe("hermes");
    expect(read.sessions).toEqual([
      { conversation_id: ROOT_ID, last_modified: "2026-10-15T11:45:00.000Z" }, // the continuation's latest message
      { conversation_id: BRANCH_ID, last_modified: "2026-10-15T12:00:01.000Z" },
    ]);
    expect(read.reads.get(ROOT_ID)?.drafts.map((d) => [d.conversation_id, d.content])).toEqual([
      [ROOT_ID, "root question"],
      [ROOT_ID, "root follow-up"],
      [ROOT_ID, "continued after compression"],
      [ROOT_ID, "continuation follow-up"],
    ]);
    expect(read.reads.get(BRANCH_ID)?.drafts.map((d) => [d.conversation_id, d.content])).toEqual([
      [BRANCH_ID, "explore an alternative"],
    ]);
    expect(read.reads.has(CONTINUATION_ID)).toBe(false);
  });

  it("Given Example 3, When extractSessions re-runs with no state.db change, Then the first run wrote one file for the root (root + continuation events) and one for the branch child, and the second reports both unchanged", async () => {
    // Given
    seedExample3(dbPath);
    const reader = new HermesConversationReader({ dbPath });
    const first = await extractSessions(reader, { outDir, source: "hermes" });
    const rootFile = path.join(outDir, "sessions", `${ROOT_ID}.jsonl`);
    const rootBytes = fs.readFileSync(rootFile, "utf8");

    // When
    const second = await extractSessions(reader, { outDir, source: "hermes" });

    // Then
    expect(first.summary.written.map((w) => [path.basename(w.path), w.events])).toEqual([
      [`${ROOT_ID}.jsonl`, 4],
      [`${BRANCH_ID}.jsonl`, 1],
    ]);
    expect(fs.readdirSync(path.join(outDir, "sessions")).sort()).toEqual([
      `${ROOT_ID}.jsonl`,
      `${BRANCH_ID}.jsonl`,
    ]);
    const [header, ...events] = readJsonl(rootFile);
    expect(header).toMatchObject({
      type: "session_header",
      conversation_id: ROOT_ID,
      source: "hermes",
      last_modified: "2026-10-15T11:45:00.000Z",
    });
    expect(events.map((e) => [e.conversation_id, e.content])).toEqual([
      [ROOT_ID, "root question"],
      [ROOT_ID, "root follow-up"],
      [ROOT_ID, "continued after compression"],
      [ROOT_ID, "continuation follow-up"],
    ]);

    expect(second.summary.enumerated).toBe(2);
    expect(second.summary.written).toEqual([]);
    expect(second.summary.unchanged).toEqual([ROOT_ID, BRANCH_ID]);
    expect(fs.readFileSync(rootFile, "utf8")).toBe(rootBytes);
  });

  it("Given delegate, generic and tool children of a root, When readSessions, Then the delegate subagent joins its root while the generic and tool children are their own sessions (the marker-mode lineage rule)", async () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, { id: "root" }, "2026-10-15T09:00:00Z");
    seedUserMessage(db, "root", "root", "2026-10-15T09:00:01Z");
    seedSession(db, { id: "delegate", parent: "root", source: "subagent" }, "2026-10-15T09:01:00Z");
    seedUserMessage(db, "delegate", "delegate", "2026-10-15T09:01:01Z");
    seedSession(db, { id: "delegate-by-config", parent: "root", modelConfig: { _delegate_from: "root" } }, "2026-10-15T09:02:00Z");
    seedUserMessage(db, "delegate-by-config", "delegate-by-config", "2026-10-15T09:02:01Z");
    seedSession(db, { id: "generic", parent: "root" }, "2026-10-15T09:03:00Z");
    seedUserMessage(db, "generic", "generic", "2026-10-15T09:03:01Z");
    seedSession(db, { id: "tool", parent: "root", source: "tool" }, "2026-10-15T09:04:00Z");
    seedUserMessage(db, "tool", "tool", "2026-10-15T09:04:01Z");
    db.close();

    // When
    const read = await new HermesConversationReader({ dbPath }).readSessions({ storedByCid: new Map() });

    // Then
    expect(read.sessions.map((s) => s.conversation_id)).toEqual(["root", "generic", "tool"]);
    expect(read.reads.get("root")?.drafts.map((d) => d.content)).toEqual(["root", "delegate", "delegate-by-config"]);
    expect(read.reads.get("generic")?.drafts.map((d) => d.content)).toEqual(["generic"]);
    expect(read.reads.get("tool")?.drafts.map((d) => d.content)).toEqual(["tool"]);
  });

  it("Given sessions last active before, at and after 2026-10-01, When readSessions with --since 2026-10-01, Then only the sessions active at/after the cutoff are enumerated", async () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, { id: "hermes-0412" }, "2026-09-28T15:00:00Z");
    seedUserMessage(db, "hermes-0412", "old", "2026-09-28T16:00:00Z");
    seedSession(db, { id: "hermes-0419" }, "2026-10-17T08:00:00Z");
    seedUserMessage(db, "hermes-0419", "new", "2026-10-17T09:00:00Z");
    seedSession(db, { id: "edge" }, "2026-09-30T23:00:00Z");
    seedUserMessage(db, "edge", "edge", "2026-10-01T00:00:00Z");
    db.close();

    // When
    const read = await new HermesConversationReader({ dbPath }).readSessions({
      since: "2026-10-01",
      storedByCid: new Map(),
    });

    // Then
    expect(read.sessions.map((s) => s.conversation_id)).toEqual(["edge", "hermes-0419"]); // at/after, inclusive
    expect([...read.reads.keys()]).toEqual(["edge", "hermes-0419"]);
  });

  it("Given two sessions, When readSessions with --conversation hermes-0412, Then exactly that session is enumerated and gathered, even when its stored stamp is unchanged", async () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, { id: "hermes-0412" }, "2026-09-28T15:00:00Z");
    seedUserMessage(db, "hermes-0412", "old", "2026-09-28T16:00:00Z");
    seedSession(db, { id: "hermes-0419" }, "2026-10-17T08:00:00Z");
    seedUserMessage(db, "hermes-0419", "new", "2026-10-17T09:00:00Z");
    db.close();
    const storedByCid = new Map([
      [
        "hermes-0412",
        {
          type: "session_header" as const,
          conversation_id: "hermes-0412",
          source: "hermes" as const,
          last_modified: "2026-09-28T16:00:00.000Z",
          extracted_at: "2026-10-01T00:00:00.000Z",
        },
      ],
    ]);

    // When
    const read = await new HermesConversationReader({ dbPath }).readSessions({
      conversation: "hermes-0412",
      storedByCid,
    });

    // Then
    expect(read.sessions.map((s) => s.conversation_id)).toEqual(["hermes-0412"]);
    expect([...read.reads.keys()]).toEqual(["hermes-0412"]);
    expect(read.reads.get("hermes-0412")?.drafts.map((d) => d.content)).toEqual(["old"]);
  });

  it("Given Example 3 on disk, When the CLI re-runs --sessions --source hermes, Then both runs exit 0, both files exist, and the re-run prints 2 sessions: 0 written, 2 unchanged, 0 untouched", () => {
    // Given
    seedExample3(dbPath);
    const run = () =>
      spawnSync(
        TSX,
        ["src/cli.ts", "--sessions", "--source", "hermes", "--hermes-db-path", dbPath, "--out", outDir],
        { cwd: ROOT, encoding: "utf8", timeout: 30000 }
      );
    const first = run();

    // When
    const second = run();

    // Then
    expect(first.status).toBe(0);
    expect(fs.existsSync(path.join(outDir, "sessions", `${ROOT_ID}.jsonl`))).toBe(true);
    expect(fs.existsSync(path.join(outDir, "sessions", `${BRANCH_ID}.jsonl`))).toBe(true);
    expect(second.status).toBe(0);
    expect(second.stdout).toBe("2 sessions: 0 written, 2 unchanged, 0 untouched\n");
  });
});

import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { ClaudeCodeTranscriptReader } from "../adapters/claudeCodeTranscriptReader.js";
import { extractSessions } from "../usecases/extractSessions.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(TEST_DIR, "..", "..");
const TSX = path.join(ROOT, "node_modules", ".bin", "tsx");
const PROJECT = "-Users-dev-acme-app";

type Json = Record<string, unknown>;

/** One user record whose string content maps to exactly one `query` event. */
function userRecord(sessionId: string, index: number, extra: Json = {}): Json {
  return {
    type: "user",
    uuid: `${sessionId}-u${index}`,
    parentUuid: null,
    sessionId,
    cwd: "/Users/dev/acme/app",
    message: { role: "user", content: `message ${index} of ${sessionId}` },
    timestamp: `2026-10-15T09:${String(Math.floor(index / 60)).padStart(2, "0")}:${String(index % 60).padStart(2, "0")}.000Z`,
    ...extra,
  };
}

function records(sessionId: string, count: number, extra: Json = {}): Json[] {
  return Array.from({ length: count }, (_, i) => userRecord(sessionId, i + 1, extra));
}

function writeJsonl(filePath: string, entries: Json[], mtime: string): void {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${entries.map((e) => JSON.stringify(e)).join("\n")}\n`, "utf8");
  const at = new Date(mtime);
  fs.utimesSync(filePath, at, at);
}

/**
 * Example 2: `a1b2c3.jsonl` (42 records) with `a1b2c3/subagents/agent-77.jsonl`
 * (18 records), and `d4e5f6.jsonl` (9 records).
 */
function seedExample2(claudeRoot: string): void {
  const projectDir = path.join(claudeRoot, PROJECT);
  writeJsonl(path.join(projectDir, "a1b2c3.jsonl"), records("a1b2c3", 42), "2026-10-15T10:00:00.000Z");
  writeJsonl(
    path.join(projectDir, "a1b2c3", "subagents", "agent-77.jsonl"),
    records("a1b2c3", 18, { agentId: "77", isSidechain: true }).map((r, i) => ({ ...r, uuid: `agent-77-u${i + 1}` })),
    "2026-10-16T08:30:00.000Z"
  );
  writeJsonl(path.join(projectDir, "d4e5f6.jsonl"), records("d4e5f6", 9), "2026-10-17T12:00:00.000Z");
}

describe("ClaudeCodeTranscriptReader.readSessions — one session per sessionId, subagents joined", () => {
  let tmp: string;
  let claudeRoot: string;
  let outDir: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-cc-sessions-"));
    claudeRoot = path.join(tmp, ".claude", "projects");
    outDir = path.join(tmp, "out");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a transcript root with a subagent parent and a plain session, When readSessions, Then one session per sessionId is enumerated with last_modified = the latest file mtime (ISO-8601 UTC)", async () => {
    // Given
    seedExample2(claudeRoot);

    // When
    const read = await new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }).readSessions({ storedByCid: new Map() });

    // Then
    expect(read.source).toBe("claude-code");
    expect(read.sessions).toEqual([
      { conversation_id: "a1b2c3", last_modified: "2026-10-16T08:30:00.000Z" }, // subagent file is the latest
      { conversation_id: "d4e5f6", last_modified: "2026-10-17T12:00:00.000Z" },
    ]);
    expect(read.reads.get("a1b2c3")?.drafts).toHaveLength(60);
    expect(read.reads.get("a1b2c3")?.last_modified).toBe("2026-10-16T08:30:00.000Z");
    expect(read.reads.get("d4e5f6")?.drafts).toHaveLength(9);
  });

  it("Given Example 2, When extractSessions re-runs with no transcript change, Then the first run wrote sessions/a1b2c3.jsonl (60 events, subagent events included) and sessions/d4e5f6.jsonl (9 events), and the second reports both unchanged", async () => {
    // Given
    seedExample2(claudeRoot);
    const reader = new ClaudeCodeTranscriptReader({ rootDir: claudeRoot });
    const first = await extractSessions(reader, { outDir, source: "claude-code", since: "2026-10-01" });
    const firstBytes = fs.readFileSync(path.join(outDir, "sessions", "a1b2c3.jsonl"), "utf8");

    // When
    const second = await extractSessions(reader, { outDir, source: "claude-code", since: "2026-10-01" });

    // Then
    expect(first.summary.written.map((w) => [path.basename(w.path), w.events])).toEqual([
      ["a1b2c3.jsonl", 60],
      ["d4e5f6.jsonl", 9],
    ]);
    const lines = firstBytes.trim().split("\n").map((line) => JSON.parse(line) as Json);
    expect(lines[0]).toMatchObject({
      type: "session_header",
      conversation_id: "a1b2c3",
      source: "claude-code",
      last_modified: "2026-10-16T08:30:00.000Z",
    });
    const events = lines.slice(1);
    expect(events.every((e) => e.conversation_id === "a1b2c3")).toBe(true);
    expect(events.filter((e) => (e.meta as Json).source_file === `${PROJECT}/a1b2c3/subagents/agent-77.jsonl`)).toHaveLength(18);

    expect(second.summary.enumerated).toBe(2);
    expect(second.summary.written).toEqual([]);
    expect(second.summary.unchanged).toEqual(["a1b2c3", "d4e5f6"]);
    expect(fs.readFileSync(path.join(outDir, "sessions", "a1b2c3.jsonl"), "utf8")).toBe(firstBytes);
  });

  it("Given sessions last active before and after 2026-10-01, When readSessions with --since 2026-10-01, Then only the session active at/after the cutoff is enumerated", async () => {
    // Given
    const projectDir = path.join(claudeRoot, PROJECT);
    writeJsonl(path.join(projectDir, "old-0412.jsonl"), records("old-0412", 2), "2026-09-28T16:00:00.000Z");
    writeJsonl(path.join(projectDir, "new-0419.jsonl"), records("new-0419", 3), "2026-10-17T09:00:00.000Z");
    writeJsonl(path.join(projectDir, "edge.jsonl"), records("edge", 1), "2026-10-01T00:00:00.000Z");
    const reader = new ClaudeCodeTranscriptReader({ rootDir: claudeRoot });

    // When
    const read = await reader.readSessions({ since: "2026-10-01", storedByCid: new Map() });

    // Then
    expect(read.sessions.map((s) => s.conversation_id)).toEqual(["edge", "new-0419"]); // at/after, inclusive
    expect([...read.reads.keys()]).toEqual(["edge", "new-0419"]);
  });

  it("Given two sessions, When readSessions with --conversation old-0412, Then exactly that session is enumerated and gathered, even when its stored stamp is unchanged", async () => {
    // Given
    const projectDir = path.join(claudeRoot, PROJECT);
    writeJsonl(path.join(projectDir, "old-0412.jsonl"), records("old-0412", 2), "2026-09-28T16:00:00.000Z");
    writeJsonl(path.join(projectDir, "new-0419.jsonl"), records("new-0419", 3), "2026-10-17T09:00:00.000Z");
    const storedByCid = new Map([
      [
        "old-0412",
        {
          type: "session_header" as const,
          conversation_id: "old-0412",
          source: "claude-code" as const,
          last_modified: "2026-09-28T16:00:00.000Z",
          extracted_at: "2026-10-01T00:00:00.000Z",
        },
      ],
    ]);

    // When
    const read = await new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }).readSessions({
      conversation: "old-0412",
      storedByCid,
    });

    // Then
    expect(read.sessions.map((s) => s.conversation_id)).toEqual(["old-0412"]);
    expect([...read.reads.keys()]).toEqual(["old-0412"]);
    expect(read.reads.get("old-0412")?.drafts).toHaveLength(2);
  });

  it("Given Example 2 on disk, When the CLI re-runs --sessions --source claude-code, Then both runs exit 0, both files exist, and the re-run prints 2 sessions: 0 written, 2 unchanged, 0 untouched", () => {
    // Given
    seedExample2(claudeRoot);
    const run = () =>
      spawnSync(
        TSX,
        ["src/cli.ts", "--sessions", "--source", "claude-code", "--since", "2026-10-01", "--claude-root", claudeRoot, "--out", outDir],
        { cwd: ROOT, encoding: "utf8", timeout: 30000 }
      );

    const first = run();

    // When
    const second = run();

    // Then
    expect(first.status).toBe(0);
    expect(fs.existsSync(path.join(outDir, "sessions", "a1b2c3.jsonl"))).toBe(true);
    expect(fs.existsSync(path.join(outDir, "sessions", "d4e5f6.jsonl"))).toBe(true);
    expect(second.status).toBe(0);
    expect(`${second.stdout}${second.stderr}`).toContain("2 sessions: 0 written, 2 unchanged, 0 untouched");
  });
});

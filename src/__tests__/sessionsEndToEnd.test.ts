import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import {
  createFixture,
  seedConversation,
  seedQuery,
  seedBlock,
  seedTask,
  type FixtureDb,
} from "./fixtures/fixtureDb.js";
import { encodeString } from "./fixtures/protobuf.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(TEST_DIR, "..", "..");
const TSX = path.join(ROOT, "node_modules", ".bin", "tsx");

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

describe("§9.23 end-to-end --sessions (CLI -> snapshot adapter -> use-case -> sink)", () => {
  let tmp: string;
  let dbPath: string;
  let outDir: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-923-"));
    dbPath = path.join(tmp, "warp.sqlite");
    outDir = path.join(tmp, "out");
    const db = createFixture(dbPath);
    seedThreeConversations(db);
    db.close();
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  function runCli(args: string[]): { stdout: string; stderr: string; exitCode: number } {
    const result = spawnSync(TSX, ["src/cli.ts", ...args], {
      cwd: ROOT,
      encoding: "utf8",
      timeout: 30000,
    });
    return {
      stdout: result.stdout ?? "",
      stderr: result.stderr ?? "",
      exitCode: result.status ?? 1,
    };
  }

  function sessionArgs(): string[] {
    return ["--sessions", "--source", "warp", "--db-path", dbPath, "--out", outDir];
  }

  it("Given a fixture DB with three conversations, When the CLI runs --sessions twice, Then the first run writes sessions/*.jsonl with a valid report and the second reports everything unchanged", () => {
    // When — first run
    const first = runCli(sessionArgs());

    // Then — three files under out/sessions/ plus the tally report
    expect(first.exitCode).toBe(0);
    const sessionsDir = path.join(outDir, "sessions");
    expect(fs.readdirSync(sessionsDir).sort()).toEqual(["c1.jsonl", "c2.jsonl", "c3.jsonl"]);
    expect(first.stdout).toContain(`wrote ${path.join(sessionsDir, "c1.jsonl")}`);
    expect(first.stdout).toContain(`wrote ${path.join(sessionsDir, "c2.jsonl")}`);
    expect(first.stdout).toContain(`wrote ${path.join(sessionsDir, "c3.jsonl")}`);
    expect(first.stdout).toContain("3 sessions: 3 written, 0 unchanged, 0 untouched");

    // strict NDJSON: trailing newline, every physical line parses
    const raw = fs.readFileSync(path.join(sessionsDir, "c1.jsonl"), "utf8");
    expect(raw.endsWith("\n")).toBe(true);
    const lines = raw.split("\n").slice(0, -1);
    const parsed = lines.map((l) => JSON.parse(l));
    for (const line of lines) expect(line.includes("\n")).toBe(false);

    // line 1 is the session_header; events carry no capture_id/label
    expect(parsed[0]!.type).toBe("session_header");
    expect(parsed[0]!.conversation_id).toBe("c1");
    const events = parsed.slice(1);
    expect(events.map((e) => e.kind)).toEqual(["query", "command", "agent_message"]);
    for (const e of events) {
      expect("capture_id" in e).toBe(false);
      expect("label" in e).toBe(false);
    }
    const ts = events.map((e) => e.ts);
    expect([...ts].sort()).toEqual(ts);

    // When — second run, nothing changed
    const second = runCli(sessionArgs());

    // Then — everything skipped as unchanged, exit 0, no wrote lines
    expect(second.exitCode).toBe(0);
    expect(second.stdout).toBe("3 sessions: 0 written, 3 unchanged, 0 untouched\n");
  });

  it("Given a fixture DB with three conversations, When the CLI runs --sessions with a --since cutoff between them, Then only the sessions at/after the cutoff are written", () => {
    // When — cutoff after c1's last activity, before c2/c3
    const result = runCli([...sessionArgs(), "--since", "2026-07-01"]);

    // Then
    expect(result.exitCode).toBe(0);
    const sessionsDir = path.join(outDir, "sessions");
    expect(fs.readdirSync(sessionsDir).sort()).toEqual(["c2.jsonl", "c3.jsonl"]);
    expect(result.stdout).toContain("2 sessions: 2 written, 0 unchanged, 0 untouched");
  });

  it("Given --id combined with --sessions, When the CLI runs, Then it exits 2 before any I/O and writes nothing", () => {
    // When
    const result = runCli([
      "--sessions", "--id", "some-capture", "--source", "warp",
      "--db-path", dbPath, "--out", outDir,
    ]);

    // Then
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain("error: --id cannot be combined with --sessions");
    expect(fs.existsSync(path.join(outDir, "sessions"))).toBe(false);
    expect(fs.existsSync(outDir)).toBe(false);
  });

  it("Given --since or --conversation without --sessions, When the CLI runs, Then it exits 2 before any I/O", () => {
    // When — a marker-mode invocation carrying a session-only flag
    const result = runCli([
      "--id", "some-capture", "--source", "warp",
      "--db-path", dbPath, "--out", outDir,
      "--since", "2026-07-01",
    ]);

    // Then
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain(
      "error: --since and --conversation are only valid together with --sessions"
    );
    expect(fs.existsSync(outDir)).toBe(false);
  });

  it("Given a --since cutoff no conversation reaches, When the CLI runs --sessions, Then it exits 1 with a reason and writes nothing", () => {
    // When
    const result = runCli([...sessionArgs(), "--since", "2100-01-01"]);

    // Then
    expect(result.exitCode).toBe(1);
    expect(result.stderr).toContain("no sessions enumerated");
    expect(result.stderr).toContain("--since 2100-01-01 too recent");
    expect(fs.existsSync(path.join(outDir, "sessions"))).toBe(false);
    expect(fs.existsSync(outDir)).toBe(false);
  });

  it("Given a source that does not implement the SessionReader port, When the CLI runs --sessions, Then it exits 2 with an explicit not-supported error", () => {
    // When — claude-code implements ConversationReader only
    const result = runCli(["--sessions", "--source", "claude-code", "--out", outDir]);

    // Then
    expect(result.exitCode).toBe(2);
    expect(result.stderr).toContain('error: source "claude-code" does not support --sessions yet');
    expect(fs.existsSync(outDir)).toBe(false);
  });
});

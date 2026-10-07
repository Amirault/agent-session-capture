import { spawnSync } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Database from "better-sqlite3";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HermesConversationReader } from "../adapters/hermesConversationReader.js";
import type { ConversationReader } from "../domain/ports.js";
import { extractBundle } from "../usecases/extractBundle.js";
import { createFixture, seedMarker as seedWarpMarker, seedQuery } from "./fixtures/fixtureDb.js";

const SPEC = "2026-07-15-hermes-session-capture-adapter";
const TOOL_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const TSX_CLI = path.join(TOOL_DIR, "node_modules", "tsx", "dist", "cli.mjs");

interface SessionOptions {
  parent?: string;
  source?: string;
  endReason?: string;
  modelConfig?: Record<string, unknown>;
  startedAt?: number;
}

interface MessageOptions {
  sessionId: string;
  role: "user" | "assistant" | "tool";
  timestamp: number;
  content?: string | null;
  toolCalls?: unknown;
  toolCallId?: string | null;
  toolName?: string | null;
  active?: number;
  compacted?: number;
}

function createHermesDb(dbPath: string, journalMode = "delete"): Database.Database {
  const db = new Database(dbPath);
  db.pragma(`journal_mode = ${journalMode}`);
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
      reasoning_content TEXT,
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

function seedSession(db: Database.Database, id: string, options: SessionOptions = {}): void {
  db.prepare(`
    INSERT INTO sessions (
      id, source, model, parent_session_id, started_at, ended_at, end_reason, model_config
    ) VALUES (?, ?, 'gpt-5.6-sol', ?, ?, NULL, ?, ?)
  `).run(
    id,
    options.source ?? "cli",
    options.parent ?? null,
    options.startedAt ?? 1_784_000_000,
    options.endReason ?? null,
    JSON.stringify(options.modelConfig ?? {})
  );
}

function insertMessage(db: Database.Database, options: MessageOptions): void {
  db.prepare(`
    INSERT INTO messages (
      session_id, role, content, reasoning_content, tool_calls,
      tool_call_id, tool_name, timestamp, active, compacted
    ) VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, ?)
  `).run(
    options.sessionId,
    options.role,
    options.content ?? null,
    options.toolCalls === undefined
      ? null
      : typeof options.toolCalls === "string"
        ? options.toolCalls
        : JSON.stringify(options.toolCalls),
    options.toolCallId ?? null,
    options.toolName ?? null,
    options.timestamp,
    options.active ?? 1,
    options.compacted ?? 0
  );
}

function toolCall(
  id: string,
  name: string,
  args: Record<string, unknown> | string
): Record<string, unknown> {
  return {
    id,
    type: "function",
    function: {
      name,
      arguments: typeof args === "string" ? args : JSON.stringify(args),
    },
  };
}

function marker(label: "specify" | "implement" | "review"): string {
  return `: CAPTURE_MARKER v=1 id=${SPEC} label=${label}`;
}

function seedToolCalls(
  db: Database.Database,
  sessionId: string,
  timestamp: number,
  calls: unknown,
  content: string | null = null
): void {
  insertMessage(db, {
    sessionId,
    role: "assistant",
    timestamp,
    content,
    toolCalls: calls,
  });
}

function seedMarker(
  db: Database.Database,
  sessionId: string,
  label: "specify" | "implement" | "review",
  timestamp: number,
  name = "terminal"
): void {
  seedToolCalls(db, sessionId, timestamp, [
    toolCall(`${sessionId}-${label}`, name, { command: marker(label) }),
  ]);
}

function seedText(db: Database.Database, sessionId: string, content: string, timestamp: number): void {
  insertMessage(db, { sessionId, role: "user", content, timestamp });
}

function runCli(args: string[], cwd: string): ReturnType<typeof spawnSync> {
  return spawnSync(process.execPath, [TSX_CLI, "src/cli.ts", ...args], {
    cwd,
    encoding: "utf8",
    env: { ...process.env, NO_COLOR: "1" },
  });
}

function readJsonl(filePath: string): Array<Record<string, unknown>> {
  return fs
    .readFileSync(filePath, "utf8")
    .trimEnd()
    .split("\n")
    .map((line) => JSON.parse(line) as Record<string, unknown>);
}

describe("HermesConversationReader", () => {
  let tempDir: string;
  let dbPath: string;

  beforeEach(() => {
    tempDir = fs.mkdtempSync(path.join(os.tmpdir(), "capture-hermes-test-"));
    dbPath = path.join(tempDir, "state.db");
  });

  afterEach(() => {
    vi.unstubAllEnvs();
    fs.rmSync(tempDir, { recursive: true, force: true });
  });

  it("GivenExactLifecycleMarkers_WhenTheHermesCliRuns_ShouldWriteACompleteBundle", () => {
    // Given
    const db = createHermesDb(dbPath);
    const labels = ["specify", "implement", "review"] as const;
    labels.forEach((label, index) => {
      const sessionId = `session-${label}`;
      seedSession(db, sessionId, { startedAt: 1_784_000_000 + index });
      seedToolCalls(db, sessionId, 1_784_000_000 + index, [
        toolCall(`${label}-marker`, "terminal", {
          command: `set -euo pipefail\n  ${marker(label)}  \nprintf done`,
        }),
      ]);
      seedText(db, sessionId, `${label}-prompt`, 1_784_000_010 + index);
    });
    db.close();
    const outputDir = path.join(tempDir, "out");

    // When
    const result = runCli(
      ["--source", "hermes", "--hermes-db-path", dbPath, "--id", SPEC, "--out", outputDir],
      TOOL_DIR
    );

    // Then
    expect(result.status).toBe(0);
    const lines = readJsonl(path.join(outputDir, `${SPEC}.jsonl`));
    expect(lines[0]).toMatchObject({
      type: "bundle_header",
      source: "hermes",
      labels: ["specify", "implement", "review"],
    });
    expect(lines.slice(1).map((line) => line.content)).toEqual(
      expect.arrayContaining(["specify-prompt", "implement-prompt", "review-prompt"])
    );
  });

  it.each(["terminal", "run_shell_command"])(
    "GivenAnExactMarkerInsideAMultilineAssistant%sCall_WhenExtracting_ShouldBind",
    (name) => {
      // Given
      const db = createHermesDb(dbPath);
      seedSession(db, "multiline-session");
      seedToolCalls(db, "multiline-session", 1_784_000_000, [
        toolCall("marker-call", name, { command: `cd /tmp\n${marker("implement")}\npwd` }),
      ]);
      db.close();

      // When
      const { bundle } = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

      // Then
      expect(bundle?.header.conversations_per_label.implement).toBe(1);
    }
  );

  it("GivenMarkerShapedNoise_WhenExtracting_ShouldRejectEveryNonCanonicalExecution", () => {
    // Given
    const db = createHermesDb(dbPath);
    const exact = marker("implement");
    const cases: Array<{ id: string; role?: "user" | "assistant" | "tool"; calls?: unknown; content?: string }> = [
      { id: "user-text", role: "user", content: exact },
      { id: "assistant-prose", role: "assistant", content: exact },
      { id: "tool-result", role: "tool", content: exact },
      { id: "other-tool", calls: [toolCall("x", "read_file", { command: exact })] },
      { id: "comment", calls: [toolCall("x", "terminal", { command: `# ${exact}` })] },
      { id: "echo", calls: [toolCall("x", "terminal", { command: `echo '${exact}'` })] },
      { id: "quoted", calls: [toolCall("x", "terminal", { command: `'${exact}'` })] },
      { id: "prefix", calls: [toolCall("x", "terminal", { command: `prefix ${exact}` })] },
      { id: "suffix", calls: [toolCall("x", "terminal", { command: `${exact} suffix` })] },
      {
        id: "wrong-order",
        calls: [toolCall("x", "terminal", { command: `: CAPTURE_MARKER id=${SPEC} v=1 label=implement` })],
      },
      { id: "wrong-version", calls: [toolCall("x", "terminal", { command: exact.replace("v=1", "v=2") })] },
    ];
    cases.forEach((entry, index) => {
      seedSession(db, entry.id);
      if (entry.calls !== undefined) {
        seedToolCalls(db, entry.id, 1_784_000_000 + index, entry.calls, entry.content ?? null);
      } else {
        insertMessage(db, {
          sessionId: entry.id,
          role: entry.role!,
          content: entry.content,
          timestamp: 1_784_000_000 + index,
        });
      }
    });
    db.close();

    // When
    const result = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    expect(result.bundle?.events).toEqual([]);
    expect(result.summary.conversations).toBe(0);
  });

  it("GivenParallelAndMalformedToolCalls_WhenExtracting_ShouldKeepValidEventsAndReportSkips", () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, "tool-session");
    seedToolCalls(
      db,
      "tool-session",
      1_784_000_000,
      [
        toolCall("marker-call", "terminal", { command: marker("implement") }),
        { id: "invalid-sibling", type: "function", function: { name: 42, arguments: "{}" } },
        toolCall("read-call", "read_file", { path: "README.md" }),
        toolCall("search-call", "search_files", { pattern: "Hermes" }),
      ],
      "I will inspect both sources."
    );
    seedToolCalls(db, "tool-session", 1_784_000_001, "{not-json", "Text survives malformed calls.");
    insertMessage(db, {
      sessionId: "tool-session",
      role: "tool",
      timestamp: 1_784_000_002,
      content: "README contents",
      toolCallId: "read-call",
      toolName: "read_file",
    });
    db.close();

    // When
    const result = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    expect(result.bundle?.events.map((event) => [event.role, event.kind])).toEqual(
      expect.arrayContaining([
        ["assistant", "agent_message"],
        ["assistant", "tool_call"],
        ["tool", "tool_result"],
      ])
    );
    expect(result.bundle?.events.filter((event) => event.kind === "tool_call").map((event) => event.meta.tool_call_id)).toEqual(
      expect.arrayContaining(["marker-call", "read-call", "search-call"])
    );
    expect(result.bundle?.events.map((event) => event.content)).toContain("Text survives malformed calls.");
    expect(result.summary.skipped_rows).toHaveLength(2);
  });

  it("GivenWorkflowAndNonWorkflowChildren_WhenExtracting_ShouldApplyNearestMarkedLineage", () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, "root", { endReason: "compression" });
    seedMarker(db, "root", "implement", 1_784_000_000);
    seedMarker(db, "root", "implement", 1_784_000_001);
    seedSession(db, "compression-child", { parent: "root" });
    seedSession(db, "delegate-child", { parent: "root", source: "subagent" });
    seedSession(db, "branch-child", {
      parent: "delegate-child",
      modelConfig: { _branched_from: "delegate-child" },
    });
    seedSession(db, "generic-child", { parent: "delegate-child" });
    seedSession(db, "null-delegate-child", {
      parent: "delegate-child",
      modelConfig: { _delegate_from: null },
    });
    seedSession(db, "tool-child", { parent: "delegate-child", source: "tool" });
    seedSession(db, "explicit-branch", {
      parent: "delegate-child",
      modelConfig: { _branched_from: "delegate-child" },
    });
    seedMarker(db, "explicit-branch", "review", 1_784_000_002);
    const ids = [
      "compression-child",
      "delegate-child",
      "branch-child",
      "generic-child",
      "null-delegate-child",
      "tool-child",
      "explicit-branch",
    ];
    ids.forEach((id, index) => seedText(db, id, id, 1_784_000_010 + index));
    db.close();

    // When
    const { bundle } = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    const labels = Object.fromEntries(
      bundle!.events.filter((event) => ids.includes(event.content)).map((event) => [event.content, event.label])
    );
    expect(labels).toEqual({
      "compression-child": "implement",
      "delegate-child": "implement",
      "explicit-branch": "review",
    });
    expect(bundle?.header.conversations_per_label.implement).toBe(3);
  });

  it("GivenAConflictingMarkerSession_WhenResolvingDescendants_ShouldBlockInheritanceAndReportCollision", () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, "root", { endReason: "compression" });
    seedMarker(db, "root", "specify", 1_784_000_000);
    seedSession(db, "conflict", { parent: "root", source: "subagent" });
    seedMarker(db, "conflict", "specify", 1_784_000_001);
    seedMarker(db, "conflict", "implement", 1_784_000_002);
    seedSession(db, "blocked", { parent: "conflict", source: "subagent" });
    seedText(db, "blocked", "blocked", 1_784_000_003);
    seedSession(db, "restarted", { parent: "conflict", source: "subagent" });
    seedMarker(db, "restarted", "review", 1_784_000_004);
    seedText(db, "restarted", "restarted", 1_784_000_005);
    db.close();

    // When
    const result = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    expect(result.bundle?.events.map((event) => event.content)).not.toContain("blocked");
    expect(result.bundle?.events.find((event) => event.content === "restarted")?.label).toBe(
      "review"
    );
    expect(result.summary.skipped_rows).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          table: "messages",
          reason: "same session has markers for multiple labels",
          detail: "conflict: specify, implement",
        }),
      ])
    );
  });

  it("GivenActiveCompactedAndParallelHermesMessages_WhenExtracting_ShouldPreserveHistoryAndIdentifiers", () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, "history-session");
    seedMarker(db, "history-session", "implement", 1_784_000_000);
    insertMessage(db, {
      sessionId: "history-session",
      role: "user",
      timestamp: 1_784_000_001,
      content: "inactive original",
      active: 0,
      compacted: 1,
    });
    insertMessage(db, {
      sessionId: "history-session",
      role: "user",
      timestamp: 1_784_000_002,
      content: "active summary",
      active: 1,
      compacted: 0,
    });
    db.close();

    // When
    const { bundle } = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    const original = bundle?.events.find((event) => event.content === "inactive original");
    const summary = bundle?.events.find((event) => event.content === "active summary");
    expect(original?.meta).toMatchObject({ active: 0, compacted: 1, session_source: "cli" });
    expect(summary?.meta).toMatchObject({ active: 1, compacted: 0, session_source: "cli" });
    expect(typeof original?.meta.message_id).toBe("number");
  });

  it("GivenAClockRegressionWithinOneConversation_WhenExtracting_ShouldKeepToolCallBeforeResult", () => {
    // Given
    const db = createHermesDb(dbPath);
    seedSession(db, "clock-session");
    seedMarker(db, "clock-session", "implement", 1_784_000_000);
    seedToolCalls(db, "clock-session", 1_784_000_020, [
      toolCall("clock-call", "read_file", { path: "README.md" }),
    ]);
    insertMessage(db, {
      sessionId: "clock-session",
      role: "tool",
      timestamp: 1_784_000_010,
      content: "clock-result",
      toolCallId: "clock-call",
      toolName: "read_file",
    });
    db.close();

    // When
    const { bundle } = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    expect(
      bundle?.events.filter((event) => event.meta.tool_call_id === "clock-call").map((event) => event.kind)
    ).toEqual(["tool_call", "tool_result"]);
  });

  it("GivenCausalDraftsAcrossConversations_WhenExtracting_ShouldUseATransitiveGlobalOrder", () => {
    // Given
    const reader: ConversationReader = {
      readCapture: () => ({
        source: "hermes",
        labelByCid: new Map([
          ["a", "implement"],
          ["b", "implement"],
          ["c", "implement"],
        ]),
        drafts: [
          {
            conversation_id: "a",
            ts: "2026-07-15T00:00:20.000Z",
            role: "assistant",
            kind: "tool_call",
            content: "call-a",
            meta: { message_id: 1, message_event_index: 0 },
          },
          {
            conversation_id: "a",
            ts: "2026-07-15T00:00:10.000Z",
            role: "tool",
            kind: "tool_result",
            content: "result-a",
            meta: { message_id: 2, message_event_index: 0 },
          },
          {
            conversation_id: "b",
            ts: "2026-07-15T00:00:15.000Z",
            role: "user",
            kind: "query",
            content: "conversation-b",
            meta: { message_id: 3, message_event_index: 0 },
          },
          {
            conversation_id: "c",
            ts: "2026-07-15T00:00:12.000Z",
            role: "user",
            kind: "query",
            content: "conversation-c",
            meta: { message_id: 4, message_event_index: 0 },
          },
        ],
        skipped: [],
        unbindable: [],
        collisions: [],
        heuristic_bindings: [],
      }),
    };

    // When
    const { bundle } = extractBundle(reader, SPEC);

    // Then
    expect(bundle?.events.map((event) => event.content)).toEqual([
      "conversation-c",
      "conversation-b",
      "call-a",
      "result-a",
    ]);
  });

  it("GivenCommittedWalFrames_WhenReadingTheLiveDb_ShouldSeeThemWithoutWriting", () => {
    // Given
    const writer = createHermesDb(dbPath, "wal");
    writer.pragma("wal_autocheckpoint = 0");
    writer.pragma("wal_checkpoint(TRUNCATE)");
    seedSession(writer, "wal-session");
    seedMarker(writer, "wal-session", "specify", 1_784_000_000);
    seedText(writer, "wal-session", "committed in WAL", 1_784_000_001);

    // When
    const { bundle } = extractBundle(new HermesConversationReader({ dbPath }), SPEC);

    // Then
    expect(bundle?.events.map((event) => event.content)).toContain("committed in WAL");
    expect(writer.pragma("query_only", { simple: true })).toBe(0);
    writer.close();
  });

  it("GivenHermesHomeAndAnExplicitOverride_WhenResolvingTheDb_ShouldTrimHomeAndPreferOverride", () => {
    // Given
    vi.stubEnv("HERMES_HOME", `  ${tempDir}  `);

    // When / Then
    expect(new HermesConversationReader().resolveDbPath()).toBe(dbPath);
    expect(new HermesConversationReader({ dbPath: "/override/state.db" }).resolveDbPath()).toBe(
      "/override/state.db"
    );
  });

  it.each([
    ["missing sessions table", "CREATE TABLE messages (id INTEGER)", "table sessions"],
    [
      "missing messages tool_calls column",
      "CREATE TABLE sessions (id TEXT, source TEXT, parent_session_id TEXT); CREATE TABLE messages (id INTEGER, session_id TEXT, role TEXT, content TEXT, timestamp REAL)",
      "messages.tool_calls",
    ],
  ])("GivenAnIncompatibleDbWith%s_WhenExtracting_ShouldNamePathAndSchema", (_name, sql, expected) => {
    // Given
    const db = new Database(dbPath);
    db.exec(sql);
    db.close();

    // When / Then
    expect(() => extractBundle(new HermesConversationReader({ dbPath }), SPEC)).toThrow(
      expect.objectContaining({ message: expect.stringContaining(dbPath) })
    );
    expect(() => extractBundle(new HermesConversationReader({ dbPath }), SPEC)).toThrow(
      expect.objectContaining({ message: expect.stringContaining(expected) })
    );
  });

  it("GivenAWarpFixture_WhenTheCliUsesWarpSource_ShouldWriteAWarpBundle", () => {
    // Given
    const warpPath = path.join(tempDir, "warp.sqlite");
    const fixture = createFixture(warpPath);
    try {
      seedWarpMarker(fixture, {
        capture_id: SPEC,
        label: "specify",
        start_ts: "2026-07-15 10:00:00.000000",
        conversation_id: "warp-cid",
      });
      seedQuery(fixture, {
        conversation_id: "warp-cid",
        start_ts: "2026-07-15 10:00:01.000000",
        text: "warp prompt",
      });
      const outputDir = path.join(tempDir, "warp-out");

      // When
      const result = runCli(
        ["--source", "warp", "--id", SPEC, "--db-path", warpPath, "--out", outputDir],
        TOOL_DIR
      );

      // Then
      expect(result.status).toBe(0);
      expect(readJsonl(path.join(outputDir, `${SPEC}.jsonl`))[0]).toMatchObject({ source: "warp" });
    } finally {
      fixture.close();
    }
  });

  it("GivenAnUnknownSource_WhenTheCliRuns_ShouldListHermesWithoutChangingUsageExitCode", () => {
    // Given / When
    const result = runCli(["--source", "unknown", "--id", SPEC], TOOL_DIR);

    // Then
    expect(result.status).toBe(2);
    expect(result.stderr).toContain('"warp", "claude-code", or "hermes"');
  });
});

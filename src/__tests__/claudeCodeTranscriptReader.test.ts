import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { extractBundle } from "../usecases/extractBundle.js";
import { ClaudeCodeTranscriptReader } from "../adapters/claudeCodeTranscriptReader.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(TEST_DIR, "..", "..");
const TSX = path.join(ROOT, "node_modules", ".bin", "tsx");
const SPEC = "2026-06-30-multiquote-limit-5";

type Json = Record<string, unknown>;

function writeTranscript(root: string, sessionId: string, entries: Json[]): void {
  const projectDir = path.join(root, "-Users-dev-acme-app");
  fs.mkdirSync(projectDir, { recursive: true });
  const transcriptPath = path.join(projectDir, `${sessionId}.jsonl`);
  fs.writeFileSync(transcriptPath, `${entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`, "utf8");
}

function writeSubagentTranscript(root: string, sessionId: string, agentId: string, entries: Json[]): void {
  const subagentsDir = path.join(root, "-Users-dev-acme-app", sessionId, "subagents");
  fs.mkdirSync(subagentsDir, { recursive: true });
  const transcriptPath = path.join(subagentsDir, `agent-${agentId}.jsonl`);
  fs.writeFileSync(transcriptPath, `${entries.map((entry) => JSON.stringify(entry)).join("\n")}\n`, "utf8");
}

function subagentEntry(agentId: string, entry: Json): Json {
  return { ...entry, agentId, isSidechain: true };
}

function userEntry(sessionId: string, uuid: string, timestamp: string, content: unknown): Json {
  return {
    type: "user",
    uuid,
    parentUuid: null,
    sessionId,
    cwd: "/Users/dev/acme/app",
    gitBranch: "main",
    version: "2.1.202",
    message: { role: "user", content },
    timestamp,
  };
}

function assistantEntry(sessionId: string, uuid: string, timestamp: string, content: unknown[]): Json {
  return {
    type: "assistant",
    uuid,
    parentUuid: null,
    sessionId,
    cwd: "/Users/dev/acme/app",
    gitBranch: "main",
    version: "2.1.202",
    message: {
      id: `msg_${uuid}`,
      type: "message",
      role: "assistant",
      model: "claude-sonnet-4-5-20250929",
      content,
      usage: { input_tokens: 42, output_tokens: 7 },
    },
    timestamp,
  };
}

function toolResultEntry(sessionId: string, uuid: string, timestamp: string, toolUseId: string, content = "done"): Json {
  return userEntry(sessionId, uuid, timestamp, [
    { type: "tool_result", tool_use_id: toolUseId, content },
  ]);
}

function markerCommand(label: string): string {
  return `: CAPTURE_MARKER v=1 id=${SPEC} label=${label}`;
}

describe("ClaudeCodeTranscriptReader", () => {
  let tmp: string;
  let claudeRoot: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-claude-"));
    claudeRoot = path.join(tmp, ".claude", "projects");
  });

  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given Claude Code JSONL transcripts with spec markers, When extracted, Then conversations are grouped by label with Claude events preserved", () => {
    // Given — one Claude Code session per label, each bound by the existing CAPTURE_MARKER no-op command.
    writeTranscript(claudeRoot, "session-specify", [
      userEntry("session-specify", "u1", "2026-06-30T09:00:00.000Z", `please specify ${SPEC}`),
      assistantEntry("session-specify", "a1", "2026-06-30T09:00:01.000Z", [
        { type: "text", text: "I will write the spec." },
        { type: "tool_use", id: "toolu_spec", name: "Bash", input: { command: markerCommand("specify") } },
      ]),
      toolResultEntry("session-specify", "r1", "2026-06-30T09:00:02.000Z", "toolu_spec"),
    ]);
    writeTranscript(claudeRoot, "session-implement", [
      userEntry("session-implement", "u2", "2026-06-30T10:00:00.000Z", `implement ${SPEC}`),
      assistantEntry("session-implement", "a2", "2026-06-30T10:00:01.000Z", [
        { type: "tool_use", id: "toolu_impl", name: "Bash", input: { command: markerCommand("implement") } },
        { type: "tool_use", id: "toolu_test", name: "Bash", input: { command: "npm test" } },
      ]),
      toolResultEntry("session-implement", "r2", "2026-06-30T10:00:02.000Z", "toolu_test", "all tests passed"),
    ]);
    writeTranscript(claudeRoot, "session-gate", [
      userEntry("session-gate", "u3", "2026-06-30T11:00:00.000Z", `validate ${SPEC}`),
      assistantEntry("session-gate", "a3", "2026-06-30T11:00:01.000Z", [
        { type: "text", text: "Gate is green." },
        { type: "tool_use", id: "toolu_gate", name: "Bash", input: { command: markerCommand("review") } },
      ]),
    ]);

    // When
    const { bundle, summary } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(bundle).not.toBeNull();
    expect(bundle!.header.source).toBe("claude-code");
    expect(bundle!.header.conversation_ids).toEqual(["session-specify", "session-implement", "session-gate"]);
    expect(bundle!.header.conversations_per_label).toEqual({
      specify: 1,
      implement: 1,
      review: 1,
    });
    expect(summary.skipped_rows).toEqual([]);

    const events = bundle!.events;
    expect(events.map((event) => event.seq)).toEqual(events.map((_, index) => index + 1));
    expect(events.map((event) => event.ts)).toEqual([...events.map((event) => event.ts)].sort());

    expect(events).toContainEqual(expect.objectContaining({
      conversation_id: "session-specify",
      label: "specify",
      role: "user",
      kind: "query",
      content: `please specify ${SPEC}`,
      meta: expect.objectContaining({ cwd: "/Users/dev/acme/app", git_branch: "main" }),
    }));
    expect(events).toContainEqual(expect.objectContaining({
      conversation_id: "session-implement",
      label: "implement",
      role: "assistant",
      kind: "tool_call",
      content: "npm test",
      meta: expect.objectContaining({ tool: "Bash", tool_use_id: "toolu_test" }),
    }));
    expect(events).toContainEqual(expect.objectContaining({
      conversation_id: "session-implement",
      label: "implement",
      role: "tool",
      kind: "tool_result",
      content: "all tests passed",
      meta: expect.objectContaining({ tool_use_id: "toolu_test" }),
    }));
  });

  it("Given the CLI is asked for Claude Code source, When it runs, Then it writes the same JSONL bundle format with source claude-code", () => {
    // Given
    const outDir = path.join(tmp, "out");
    writeTranscript(claudeRoot, "session-specify", [
      userEntry("session-specify", "u1", "2026-06-30T09:00:00.000Z", `specify ${SPEC}`),
      assistantEntry("session-specify", "a1", "2026-06-30T09:00:01.000Z", [
        { type: "tool_use", id: "toolu_spec", name: "Bash", input: { command: markerCommand("specify") } },
      ]),
    ]);

    // When
    const stdout = execFileSync(TSX, [
      "src/cli.ts",
      "--source",
      "claude-code",
      "--claude-root",
      claudeRoot,
      "--id",
      SPEC,
      "--out",
      outDir,
    ], {
      cwd: ROOT,
      encoding: "utf8",
      timeout: 30000,
    });

    // Then
    const raw = fs.readFileSync(path.join(outDir, `${SPEC}.jsonl`), "utf8");
    const [headerLine, ...eventLines] = raw.trimEnd().split("\n");
    const header = JSON.parse(headerLine!);
    expect(stdout).toContain("wrote");
    expect(header).toEqual(expect.objectContaining({
      type: "bundle_header",
      capture_id: SPEC,
      source: "claude-code",
      labels: ["specify"],
    }));
    expect(eventLines.map((line) => JSON.parse(line)).some((event) => event.kind === "tool_call")).toBe(true);
  });

  it("Given a Claude Code transcript only mentions the marker in a prompt, When extracted, Then it is not bound as a captured label", () => {
    // Given — Claude Code records failed unauthenticated prompts too; a text mention
    // must not count as the label marker unless Claude actually emitted the Bash tool_use.
    writeTranscript(claudeRoot, "session-mention-only", [
      userEntry("session-mention-only", "u1", "2026-06-30T09:00:00.000Z", markerCommand("specify")),
      assistantEntry("session-mention-only", "a1", "2026-06-30T09:00:01.000Z", [
        { type: "text", text: "Not logged in · Please run /login" },
      ]),
    ]);

    // When
    const { bundle, summary } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(bundle).not.toBeNull();
    expect(bundle!.header.source).toBe("claude-code");
    expect(bundle!.header.conversation_ids).toEqual([]);
    expect(summary.conversations).toBe(0);
    expect(summary.events).toBe(0);
  });

  it("Given a marker-shaped command field on a non-Bash tool_use, When extracted, Then it is not bound", () => {
    // Given — CAPTURE_MARKER is a shell no-op; only Bash tool_use blocks prove it was emitted.
    writeTranscript(claudeRoot, "session-non-bash", [
      assistantEntry("session-non-bash", "a1", "2026-06-30T09:00:00.000Z", [
        { type: "tool_use", id: "toolu_write", name: "Write", input: { command: markerCommand("specify"), file_path: "notes.md" } },
      ]),
    ]);

    // When
    const { summary } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(summary.conversations).toBe(0);
    expect(summary.events).toBe(0);
  });

  it("Given a marker-shaped Bash tool_use outside an assistant turn, When extracted, Then it is not bound", () => {
    // Given — Claude emits tool_use blocks from assistant turns; user/tool-result turns must not seed labels.
    writeTranscript(claudeRoot, "session-user-tool-use", [
      userEntry("session-user-tool-use", "u1", "2026-06-30T09:00:00.000Z", [
        { type: "tool_use", id: "toolu_user", name: "Bash", input: { command: markerCommand("specify") } },
      ]),
    ]);

    // When
    const { summary } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(summary.conversations).toBe(0);
    expect(summary.events).toBe(0);
  });
  it("Given a bound session interleaved with timestamp-less metadata records, When extracted, Then only message records lacking a timestamp are reported as skipped", () => {
    // Given — Claude Code appends session metadata (custom-title, last-prompt,
    // file-history-snapshot, …) without a timestamp; they are not events.
    const sessionId = "session-metadata";
    writeTranscript(claudeRoot, sessionId, [
      assistantEntry(sessionId, "a1", "2026-06-30T09:00:00.000Z", [
        { type: "tool_use", id: "toolu_spec", name: "Bash", input: { command: markerCommand("specify") } },
      ]),
      { type: "custom-title", sessionId, customTitle: "Specify multiquote limit" },
      { type: "last-prompt", sessionId, lastPrompt: "/specify multiquote" },
      { type: "file-history-snapshot", messageId: "m1", snapshot: {}, isSnapshotUpdate: false },
      { ...userEntry(sessionId, "u2", "unused", "a prompt with no timestamp"), timestamp: undefined },
    ]);

    // When
    const { summary } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(summary.conversations).toBe(1);
    expect(summary.skipped_rows.map((row) => row.reason)).toEqual(["missing timestamp"]);
  });

  it("Given one Claude Code session whose implement and review subagents emit their own label markers, When extracted, Then each subagent transcript is bound to its label", () => {
    // Given — an autonomous-workflow run: both subagents share the parent sessionId
    // and differ only by their agentId.
    const sessionId = "session-autonomous";
    writeTranscript(claudeRoot, sessionId, [
      userEntry(sessionId, "u1", "2026-06-30T10:00:00.000Z", `run the workflow on ${SPEC}`),
    ]);
    writeSubagentTranscript(claudeRoot, sessionId, "impl", [
      subagentEntry("impl", assistantEntry(sessionId, "a1", "2026-06-30T10:00:01.000Z", [
        { type: "tool_use", id: "toolu_impl", name: "Bash", input: { command: markerCommand("implement") } },
      ])),
    ]);
    writeSubagentTranscript(claudeRoot, sessionId, "rev", [
      subagentEntry("rev", assistantEntry(sessionId, "a2", "2026-06-30T11:00:01.000Z", [
        { type: "tool_use", id: "toolu_rev", name: "Bash", input: { command: markerCommand("review") } },
      ])),
    ]);

    // When
    const { bundle, summary } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(bundle).not.toBeNull();
    expect(bundle!.header.conversations_per_label).toEqual({ implement: 1, review: 1 });
    expect(bundle!.header.conversation_ids).toEqual([`${sessionId}/agent-impl`, `${sessionId}/agent-rev`]);
    expect(summary.skipped_rows.map((row) => row.reason)).not.toContain("same session has markers for multiple labels");
  });

  it("Given a single-label Claude Code session with a marker-less subagent, When extracted, Then the subagent transcript binds under the plain sessionId", () => {
    // Given — a manual implement session delegating a refactoring pass to a subagent.
    const sessionId = "session-manual-implement";
    writeTranscript(claudeRoot, sessionId, [
      assistantEntry(sessionId, "a1", "2026-06-30T10:00:00.000Z", [
        { type: "tool_use", id: "toolu_impl", name: "Bash", input: { command: markerCommand("implement") } },
      ]),
    ]);
    writeSubagentTranscript(claudeRoot, sessionId, "refactor", [
      subagentEntry("refactor", assistantEntry(sessionId, "a2", "2026-06-30T10:30:00.000Z", [
        { type: "text", text: "Refactoring pass done." },
      ])),
    ]);

    // When
    const { bundle } = extractBundle(new ClaudeCodeTranscriptReader({ rootDir: claudeRoot }), SPEC);

    // Then
    expect(bundle!.header.conversation_ids).toEqual([sessionId]);
    expect(bundle!.events).toContainEqual(expect.objectContaining({
      conversation_id: sessionId,
      label: "implement",
      content: "Refactoring pass done.",
    }));
  });
});

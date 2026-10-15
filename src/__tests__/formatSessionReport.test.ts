import { describe, it, expect } from "vitest";
import { formatSessionReport } from "../formatRunReport.js";
import type { SessionRunSummary } from "../domain/models.js";

function sessionSummary(overrides: Partial<SessionRunSummary> = {}): SessionRunSummary {
  return {
    source: "warp",
    since: null,
    conversation: null,
    enumerated: 3,
    written: [
      { conversation_id: "c1", path: "/tmp/out/sessions/c1.jsonl", events: 12 },
      { conversation_id: "c2", path: "/tmp/out/sessions/c2.jsonl", events: 3 },
    ],
    unchanged: ["c3"],
    untouched: ["/tmp/out/sessions/c9.jsonl"],
    skipped_rows: [],
    warnings: [],
    ...overrides,
  };
}

describe("§9.22 formatSessionReport — session-mode CLI report and exit codes", () => {
  it("Given a summary with written, unchanged and untouched sessions, When formatSessionReport, Then one wrote line per file, a final tally line, and exit 0", () => {
    // Given
    const summary = sessionSummary();

    // When
    const report = formatSessionReport(summary);

    // Then
    expect(report.exitCode).toBe(0);
    expect(report.stdout).toEqual([
      "wrote /tmp/out/sessions/c1.jsonl (12 events)",
      "wrote /tmp/out/sessions/c2.jsonl (3 events)",
      "3 sessions: 2 written, 1 unchanged, 1 untouched",
    ]);
    expect(report.stderr).toEqual([]);
  });

  it("Given a summary where every session is already up to date, When formatSessionReport, Then only the tally line prints and the exit code stays 0", () => {
    // Given
    const summary = sessionSummary({
      written: [],
      unchanged: ["c1", "c2", "c3"],
      untouched: [],
    });

    // When
    const report = formatSessionReport(summary);

    // Then
    expect(report.exitCode).toBe(0);
    expect(report.stdout).toEqual(["3 sessions: 0 written, 3 unchanged, 0 untouched"]);
    expect(report.stderr).toEqual([]);
  });

  it("Given zero enumerated sessions after a --since filter, When formatSessionReport, Then the run fails with exit 1 and names the reason", () => {
    // Given
    const summary = sessionSummary({
      since: "2100-01-01",
      enumerated: 0,
      written: [],
      unchanged: [],
      untouched: [],
    });

    // When
    const report = formatSessionReport(summary);

    // Then
    expect(report.exitCode).toBe(1);
    expect(report.stdout).toEqual([]);
    expect(report.stderr[0]).toBe(
      "no sessions enumerated (--since 2100-01-01 too recent) — nothing written."
    );
  });

  it("Given zero enumerated sessions after a --conversation filter, When formatSessionReport, Then the run fails with exit 1 and names the unknown conversation", () => {
    // Given
    const summary = sessionSummary({
      conversation: "nope",
      enumerated: 0,
      written: [],
      unchanged: [],
      untouched: [],
    });

    // When
    const report = formatSessionReport(summary);

    // Then
    expect(report.exitCode).toBe(1);
    expect(report.stderr[0]).toBe(
      "no sessions enumerated (unknown --conversation nope) — nothing written."
    );
  });

  it("Given guard warnings and skipped rows alongside a successful run, When formatSessionReport, Then every anomaly prints one per line under a warnings header, never as bare counts", () => {
    // Given
    const summary = sessionSummary({
      warnings: [
        'replacing stored session (--no-merge): corrupt session file "/tmp/out/sessions/c2.jsonl": header line is not valid JSON',
      ],
      skipped_rows: [
        { table: "agent_tasks", reason: "empty task", detail: "task_id=t-1" },
        { table: "agent_tasks", reason: "walk threw", detail: "task_id=t-2: truncated varint" },
      ],
    });

    // When
    const report = formatSessionReport(summary);

    // Then — two skipped rows => two distinct lines, not "2 skipped rows"
    expect(report.exitCode).toBe(0);
    expect(report.stderr).toEqual([
      "warnings:",
      'replacing stored session (--no-merge): corrupt session file "/tmp/out/sessions/c2.jsonl": header line is not valid JSON',
      'skipped row: table=agent_tasks reason="empty task" (task_id=t-1)',
      'skipped row: table=agent_tasks reason="walk threw" (task_id=t-2: truncated varint)',
    ]);
    expect(report.stderr.join("\n")).not.toMatch(/\b\d+ skipped rows\b/);
  });
});

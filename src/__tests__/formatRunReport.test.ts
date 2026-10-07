import { describe, it, expect } from "vitest";
import type { RunSummary, SeedMatch, SkippedRow } from "../domain/models.js";
import { formatRunReport } from "../formatRunReport.js";

const SPEC = "add-feature-x";

function baseSummary(overrides: Partial<RunSummary> = {}): RunSummary {
  return {
    capture_id: SPEC,
    conversations: 1,
    events: 3,
    labels: ["specify", "implement", "review"],
    unbindable: [],
    collisions: [],
    heuristic_bindings: [],
    skipped_rows: [],
    fresh_read_error: null,
    output_path: null,
    ...overrides,
  };
}

function heuristicMarker(label: SeedMatch["label"], start_ts: string): SeedMatch {
  return {
    conversation_id: "c-heuristic",
    label,
    marker_command: `: CAPTURE_MARKER v=1 id=${SPEC} label=${label}`,
    start_ts,
    status: "bound",
    confidence: "heuristic",
  };
}

function unbindableMarker(label: SeedMatch["label"], start_ts: string): SeedMatch {
  return {
    conversation_id: null,
    label,
    marker_command: `: CAPTURE_MARKER v=1 id=${SPEC} label=${label}`,
    start_ts,
    status: "unbindable",
  };
}

function collisionMarker(label: SeedMatch["label"], start_ts: string): SeedMatch {
  return {
    conversation_id: null,
    label,
    marker_command: `: CAPTURE_MARKER v=1 id=${SPEC} label=${label}`,
    start_ts,
    status: "collision",
  };
}

describe("§9.13 formatRunReport — CLI diagnostics", () => {
  it("Given a fully bound complete spec with a written bundle, When formatted, Then it succeeds with no anomaly lines", () => {
    // Given
    const summary = baseSummary();
    const outPath = "out/add-feature-x.jsonl";

    // When
    const report = formatRunReport(summary, outPath);

    // Then
    expect(report.exitCode).toBe(0);
    expect(report.stdout).toHaveLength(1);
    expect(report.stdout[0]).toContain("wrote");
    expect(report.stdout[0]).toContain(outPath);
    expect(report.stderr).toEqual([]);
  });

  it("Given zero bound conversations with an unbindable marker, When formatted, Then nothing is written, exit is 1, and the binding-decay hint plus per-marker detail are surfaced", () => {
    // Given — the marker was found but its binding block has decayed
    const summary = baseSummary({
      conversations: 0,
      events: 0,
      labels: [],
      unbindable: [unbindableMarker("implement", "2026-06-30 11:00:00.000000")],
    });

    // When
    const report = formatRunReport(summary, null);

    // Then
    expect(report.exitCode).toBe(1);
    expect(report.stdout).toEqual([]);
    const joined = report.stderr.join("\n");
    expect(joined).toContain("no conversations bound");
    expect(joined).toContain(SPEC);
    expect(joined).toContain("label=implement");
    expect(joined).toContain("2026-06-30 11:00:00.000000");
    expect(joined.toLowerCase()).toContain("binding");
  });

  it("Given a heuristic-bound marker (local orchestrated subagent), When formatted, Then the warning names the label, timestamp, and fallback strategy", () => {
    // Given
    const summary = baseSummary({
      heuristic_bindings: [heuristicMarker("implement", "2026-09-11 02:58:24.952686")],
    });
    const outPath = "out/add-feature-x.jsonl";

    // When
    const report = formatRunReport(summary, outPath);

    // Then
    expect(report.exitCode).toBe(0);
    expect(report.stdout[0]).toContain("wrote");
    const joined = report.stderr.join("\n");
    expect(joined).toContain("heuristic binding");
    expect(joined).toContain("label=implement");
    expect(joined).toContain("2026-09-11 02:58:24.952686");
    expect(joined.toLowerCase()).toContain("ai_queries");
  });

  it("Given a written bundle alongside an unbindable marker and a collision, When formatted, Then it still succeeds but every anomaly is detailed instead of a bare count", () => {
    // Given
    const summary = baseSummary({
      unbindable: [unbindableMarker("specify", "2026-06-30 09:00:00.000000")],
      collisions: [collisionMarker("implement", "2026-06-30 10:00:00.000000")],
      skipped_rows: [
        { table: "ai_queries", reason: "input failed schema", detail: "cid=c9 ts=x" } satisfies SkippedRow,
      ],
    });
    const outPath = "out/add-feature-x.jsonl";

    // When
    const report = formatRunReport(summary, outPath);

    // Then
    expect(report.exitCode).toBe(0);
    expect(report.stdout[0]).toContain("wrote");
    const joined = report.stderr.join("\n");
    expect(joined).toContain("label=specify");
    expect(joined).toContain("2026-06-30 09:00:00.000000");
    expect(joined).toContain("label=implement");
    expect(joined).toContain("2026-06-30 10:00:00.000000");
    expect(joined).toContain("ai_queries");
    expect(joined).toContain("input failed schema");
    // no bare "N unbindable, N collisions" style summary line
    expect(joined).not.toContain("1 unbindable, 1 collisions");
  });
});

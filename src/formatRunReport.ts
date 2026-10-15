import type { RunSummary, SessionRunSummary } from "./domain/models.js";

/** What the CLI should print and exit with for one extraction run. */
export interface RunReport {
  exitCode: number;
  stdout: string[];
  stderr: string[];
}

const BINDING_DECAY_HINT =
  "a marker was found but its binding block is gone — Warp can evict blocks rows over time; extract soon after the session, before the binding decays.";

/**
 * One line per anomaly (never a bare count): a marker binding failure or a
 * skipped row hides its cause behind a number, which is exactly what made a
 * silent, empty extraction hard to diagnose. Detail is cheap since these rows
 * are rare in practice.
 */
function anomalyLines(summary: RunSummary): string[] {
  const lines: string[] = [];
  if (summary.fresh_read_error) {
    lines.push(
      `fresh read failed: ${summary.fresh_read_error} — using stored bundle`
    );
  }
  for (const s of summary.heuristic_bindings) {
    lines.push(
      `heuristic binding: label=${s.label} start_ts=${s.start_ts} — bound via ai_queries temporal fallback (local orchestrated subagent); verify conversation is correct`
    );
  }
  for (const s of summary.unbindable) {
    lines.push(`unbindable marker: label=${s.label} start_ts=${s.start_ts}`);
  }
  for (const s of summary.collisions) {
    lines.push(
      `collision marker: label=${s.label} start_ts=${s.start_ts} (bound to multiple conversations)`
    );
  }
  for (const r of summary.skipped_rows) {
    lines.push(`skipped row: table=${r.table} reason="${r.reason}" (${r.detail})`);
  }
  return lines;
}

/**
 * Format the CLI's report for one extraction run.
 *
 * `outPath` is the file actually written, or null when nothing was written
 * because zero conversations bound.
 */
export function formatRunReport(summary: RunSummary, outPath: string | null): RunReport {
  const anomalies = anomalyLines(summary);

  if (outPath !== null) {
    const stdout = [
      `wrote ${outPath} (${summary.events} events, ${summary.conversations} conversations)`,
    ];
    const stderr = anomalies.length > 0 ? ["warnings:", ...anomalies] : [];
    return { exitCode: 0, stdout, stderr };
  }

  const stderr = [
    `no conversations bound for capture id "${summary.capture_id}" — nothing written.`,
    ...anomalies,
  ];
  if (summary.unbindable.length > 0) stderr.push(BINDING_DECAY_HINT);
  return { exitCode: 1, stdout: [], stderr };
}

/**
 * Format the CLI's report for one `--sessions` run.
 *
 * stdout: one `wrote <path> (N events)` line per written session file, then a
 * final `N sessions: W written, S unchanged, U untouched` tally line — the
 * untouched count is the only summary number (stored files are the durable
 * record, never GC'd). Anomalies (guard warnings, skipped rows) stay
 * one-per-line, never a bare count.
 *
 * exit 0: at least one file written, or everything already up to date.
 * exit 1: zero sessions enumerated (`--since` too recent, unknown
 * `--conversation`) — nothing written. Usage errors are exit 2 and never
 * reach the report (the CLI rejects them before any I/O).
 */
export function formatSessionReport(summary: SessionRunSummary): RunReport {
  const warnings = [
    ...summary.warnings,
    ...summary.skipped_rows.map(
      (r) => `skipped row: table=${r.table} reason="${r.reason}" (${r.detail})`
    ),
  ];

  if (summary.enumerated === 0) {
    const reasons: string[] = [];
    if (summary.since !== null) reasons.push(`--since ${summary.since} too recent`);
    if (summary.conversation !== null) {
      reasons.push(`unknown --conversation ${summary.conversation}`);
    }
    const reasonText = reasons.length > 0 ? ` (${reasons.join(", ")})` : "";
    return {
      exitCode: 1,
      stdout: [],
      stderr: [`no sessions enumerated${reasonText} — nothing written.`, ...warnings],
    };
  }

  const stdout = [
    ...summary.written.map((w) => `wrote ${w.path} (${w.events} events)`),
    `${summary.enumerated} sessions: ${summary.written.length} written, ${summary.unchanged.length} unchanged, ${summary.untouched.length} untouched`,
  ];
  return {
    exitCode: 0,
    stdout,
    stderr: warnings.length > 0 ? ["warnings:", ...warnings] : [],
  };
}

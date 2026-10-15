/**
 * Pure timestamp parsing shared by the session-mode adapters and the CLI.
 *
 * Two formats matter:
 * - source-native stamps (Warp/SQLite: UTC "YYYY-MM-DD HH:MM:SS(.ffffff)"),
 *   compared for skip-unchanged as RAW strings (no parsing) but parsed as UTC
 *   for the `--since` cutoff;
 * - `--since` values (ISO-8601 date or datetime), validated by the CLI (exit 2
 *   when unparseable) and parsed to the same UTC epoch scale.
 */

/** Matches a SQLite-style UTC stamp: "2026-06-30 12:00:00" (fraction optional). */
const STAMP_RE = /^(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2}:\d{2})(?:\.(\d+))?$/;

/** Matches a --since value that is a bare date (midnight UTC cutoff). */
const DATE_ONLY_RE = /^\d{4}-\d{2}-\d{2}$/;

const ZONE_SUFFIX_RE = /(Z|[+-]\d{2}:?\d{2})$/;

/**
 * Parse a source-native stamp as UTC epoch milliseconds, or null when the
 * stamp does not match the expected shape. Fractional seconds beyond
 * milliseconds are truncated (JS Date holds only milliseconds).
 */
export function parseStampUtc(stamp: string): number | null {
  const match = STAMP_RE.exec(stamp.trim());
  if (match === null) return null;
  const fraction = (match[3] ?? "").slice(0, 3).padEnd(3, "0");
  const epoch = Date.parse(`${match[1]!}T${match[2]!}.${fraction}Z`);
  return Number.isNaN(epoch) ? null : epoch;
}

/**
 * Parse a `--since` value (ISO-8601 date or datetime) as UTC epoch
 * milliseconds, or null when unparseable (a CLI usage error).
 *
 * A bare date means midnight UTC. A datetime without a zone suffix is read as
 * UTC (a silent local-time reading would shift the cutoff by the machine's
 * offset); `Z` and `±HH:MM` suffixes are honored.
 */
export function parseSinceUtc(value: string): number | null {
  if (DATE_ONLY_RE.test(value)) return Date.parse(`${value}T00:00:00.000Z`);
  if (!/^\d{4}-\d{2}-\d{2}T/.test(value)) return null;
  const withZone = ZONE_SUFFIX_RE.test(value) ? value : `${value}Z`;
  const epoch = Date.parse(withZone);
  return Number.isNaN(epoch) ? null : epoch;
}

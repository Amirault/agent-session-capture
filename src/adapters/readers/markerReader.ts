import { DEFAULT_LABEL, type Label, type SeedMatch, type SeedStatus } from "../../domain/models.js";
import type { ReadableDb } from "../../domain/ports.js";

/**
 * Fallback binding strategy for local orchestrated subagents.
 *
 * When a CAPTURE_MARKER is in `commands` but has no matching `blocks` row (so the
 * standard ai_metadata JOIN produces no conversation_id), query `ai_queries` for
 * the most recent AI exchange that completed at or before the marker's start_ts
 * within a 10-minute look-back window.  The last AI query before the marker is
 * very likely from the subagent that just ran it (it reads the skill, then emits
 * the marker as one of its first commands).
 *
 * Returns the conversation_id string, or null when no qualifying row is found.
 */
function fallbackViaAiQueries(db: ReadableDb, markerTs: string): string | null {
  interface AiQueryRow {
    conversation_id: string;
  }
  try {
    const rows = db.all<AiQueryRow>(
      `SELECT conversation_id
         FROM ai_queries
        WHERE start_ts BETWEEN datetime(?, '-600 seconds') AND ?
        ORDER BY start_ts DESC
        LIMIT 1`,
      markerTs,
      markerTs
    );
    return rows[0]?.conversation_id ?? null;
  } catch {
    // ai_queries absent in older DB snapshots — degrade gracefully.
    return null;
  }
}

/**
 * Marker selection & binding.
 *
 * The correlation marker `: CAPTURE_MARKER v=1 id=<id> [label=<label>]` is a
 * shell no-op recorded cleanly in commands.command (greppable), and creates a
 * blocks row at the SAME start_ts carrying ai_metadata.conversation_id. We bind
 * the marker to its conversation by joining on start_ts (verified 1:1).
 *
 * The `: CAPTURE_MARKER` anchor avoids false positives from any text that merely
 * mentions the marker (e.g. diagnostic scripts). Selection is on commands.command
 * only — NOT on blocks.stylized_command (ANSI-exploded, not greppable).
 */

/** Parse a marker command into its capture_id + label, or null if not a valid marker. */
export function parseMarker(
  command: string
): { capture_id: string; label: Label } | null {
  // Warp records the marker's `: CAPTURE_MARKER ...` no-op and the real command that
  // follows it as ONE multi-line commands.command value. Parse only the first
  // line — otherwise the next line's tokens bleed into `label=...`, and a
  // marker-shaped heredoc/file body line would count as an emission.
  const firstLine = command.split("\n", 1)[0]!;
  // Agents sometimes chain the marker (`cd app; : CAPTURE_MARKER ...`,
  // `build.sh && : CAPTURE_MARKER ...`): it still ran, so accept it
  // as any unquoted `;` / `&&` segment of the first line.
  for (const segment of unquotedSegments(firstLine)) {
    const parsed = parseMarkerSegment(segment);
    if (parsed !== null) return parsed;
  }
  return null;
}

function unquotedSegments(line: string): string[] {
  const segments: string[] = [];
  let current = "";
  let quote: string | null = null;
  for (let i = 0; i < line.length; i++) {
    const char = line[i]!;
    if (quote !== null) {
      if (char === quote) quote = null;
      current += char;
      continue;
    }
    if (char === "'" || char === '"') {
      quote = char;
      current += char;
      continue;
    }
    if (char === ";" || (char === "&" && line[i + 1] === "&")) {
      segments.push(current);
      current = "";
      if (char === "&") i++;
      continue;
    }
    current += char;
  }
  segments.push(current);
  return segments;
}

function parseMarkerSegment(
  segment: string
): { capture_id: string; label: Label } | null {
  const tokens = segment.trim().split(" ").filter((t) => t.length > 0);
  if (tokens[0] !== ":" || tokens[1] !== "CAPTURE_MARKER") return null;

  if (tokens[2] !== "v=1") return null;

  let capture_id: string | null = null;
  let label: Label = DEFAULT_LABEL;
  for (const t of tokens) {
    if (t.startsWith("id=")) {
      capture_id = t.slice("id=".length);
    } else if (t.startsWith("label=")) {
      const v = t.slice("label=".length);
      if (v.length > 0) label = v;
    }
  }
  if (capture_id === null || capture_id.length === 0) return null;
  return { capture_id, label };
}

interface MarkerJoinRow {
  command: string;
  start_ts: string;
  cid: string | null;
}

/**
 * Find every marker emission for `captureId`, bound to a conversation where possible.
 * Returns seeds in start_ts order. Unbindable / collision emissions are included
 * (reported in the run summary) but carry conversation_id = null.
 */
export function findSeeds(db: ReadableDb, captureId: string): SeedMatch[] {
  const rows = db.all<MarkerJoinRow>(
    `SELECT c.command, c.start_ts,
            json_extract(b.ai_metadata, '$.conversation_id') AS cid
       FROM commands c
       LEFT JOIN blocks b ON b.start_ts = c.start_ts
      WHERE c.command LIKE ': CAPTURE_MARKER%'
      ORDER BY c.start_ts`
  );

  // Group rows by marker command (keyed by start_ts = one emission); collect the
  // distinct conversation ids each emission binds to. Rows are already start_ts
  // ordered, so Map insertion order is chronological.
  const byEmission = new Map<
    string,
    { command: string; cids: string[] }
  >();
  for (const r of rows) {
    const entry = byEmission.get(r.start_ts) ?? { command: r.command, cids: [] };
    if (r.cid) entry.cids.push(r.cid);
    byEmission.set(r.start_ts, entry);
  }

  const seeds: SeedMatch[] = [];
  const boundConversations = new Set<string>();

  for (const [start_ts, { command, cids }] of byEmission) {
    const parsed = parseMarker(command);
    if (!parsed || parsed.capture_id !== captureId) continue;

    const distinct = [...new Set(cids)];
    let status: SeedStatus;
    let conversation_id: string | null;

    if (distinct.length === 0) {
      const fallbackCid = fallbackViaAiQueries(db, start_ts);
      if (fallbackCid !== null) {
        status = "bound";
        conversation_id = fallbackCid;
      } else {
        status = "unbindable";
        conversation_id = null;
      }
    } else if (distinct.length === 1) {
      status = "bound";
      conversation_id = distinct[0]!;
    } else {
      status = "collision";
      conversation_id = null;
    }

    if (status === "bound") {
      if (boundConversations.has(conversation_id!)) continue; // dedup re-emissions
      boundConversations.add(conversation_id!);
    }

    seeds.push({
      conversation_id,
      label: parsed.label,
      marker_command: command,
      start_ts,
      status,
      ...(status === "bound" && distinct.length === 0 ? { confidence: "heuristic" as const } : {}),
    });
  }

  return seeds;
}

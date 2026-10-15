import fs from "node:fs";
import path from "node:path";
import type { BundleSource, SessionEvent, SessionFile, SessionHeader } from "../domain/models.js";
import { sessionFileName } from "./sessionFileName.js";

/** One stored session file discovered by the scan. */
export interface StoredSessionEntry {
  /** File basename without the `.jsonl` extension. */
  fileName: string;
  /** Full path of the file. */
  path: string;
  header: SessionHeader;
}

/** A stored file as discovered by the directory scan. */
interface ScanOutcome {
  fileName: string;
  path: string;
}

/** Enough bytes for any well-formed session_header line, then some. */
const FIRST_LINE_BUDGET = 64 * 1024;

/**
 * Read only a file's first physical line (session files can be MBs; a full read
 * is only needed for merging). Returns null for an empty file.
 */
function readFirstLine(filePath: string): string | null {
  const fd = fs.openSync(filePath, "r");
  try {
    const chunk = Buffer.alloc(FIRST_LINE_BUDGET);
    const bytesRead = fs.readSync(fd, chunk, 0, chunk.length, 0);
    if (bytesRead === 0) return null;
    const text = chunk.subarray(0, bytesRead).toString("utf8");
    const newline = text.indexOf("\n");
    if (newline === -1) return text; // no newline: whole (short) file is the header
    return text.slice(0, newline);
  } finally {
    fs.closeSync(fd);
  }
}

function parseHeaderLine(line: string | null, filePath: string): SessionHeader {
  if (line === null || line.trim().length === 0) {
    throw new Error(`corrupt session file "${filePath}": empty file`);
  }
  let parsed: unknown;
  try {
    parsed = JSON.parse(line);
  } catch {
    throw new Error(`corrupt session file "${filePath}": header line is not valid JSON`);
  }
  const header = parsed as SessionHeader;
  if (
    header === null ||
    typeof header !== "object" ||
    header.type !== "session_header" ||
    typeof header.conversation_id !== "string" ||
    (typeof header.last_modified !== "string" && header.last_modified !== null)
  ) {
    throw new Error(`corrupt session file "${filePath}": first line is not a session_header`);
  }
  return header;
}

function guard(
  outcome: ScanOutcome,
  source: BundleSource
): { ok: true; header: SessionHeader } | { ok: false; error: string } {
  let header: SessionHeader;
  try {
    header = parseHeaderLine(readFirstLine(outcome.path), outcome.path);
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
  if (header.source !== source) {
    return {
      ok: false,
      error:
        `session file source mismatch: file "${outcome.path}" has source "${header.source}", ` +
        `run requested "${source}" — refusing to merge across sources`,
    };
  }
  if (sessionFileName(header.conversation_id) !== outcome.fileName) {
    return {
      ok: false,
      error:
        `session file conversation mismatch: file "${outcome.fileName}.jsonl" holds conversation ` +
        `"${header.conversation_id}" — this looks like a collision or manual tampering`,
    };
  }
  return { ok: true, header };
}

/**
 * Scan `<outDir>/sessions/*.jsonl`, reading each file's FIRST LINE only
 * (files can be MBs; a full parse is only needed for merging).
 *
 * Guards mirror the marker mode's stored-bundle guards: a corrupt file, a
 * source mismatch, or a header-vs-filename conversation mismatch fails the
 * run loudly (nothing is silently dropped) — unless `noMerge`, where each
 * violation is downgraded to a warning and the entry is skipped (the file is
 * replaced by the fresh write when its conversation is enumerated).
 */
export function scanStoredSessions(
  sessionsDir: string,
  opts: { source: BundleSource; noMerge: boolean }
): { byCid: Map<string, StoredSessionEntry>; warnings: string[] } {
  const byCid = new Map<string, StoredSessionEntry>();
  const warnings: string[] = [];

  if (!fs.existsSync(sessionsDir)) return { byCid, warnings };

  const fileNames = fs
    .readdirSync(sessionsDir)
    .filter((name) => name.endsWith(".jsonl"))
    .sort();

  for (const name of fileNames) {
    const filePath = path.join(sessionsDir, name);
    const outcome: ScanOutcome = { fileName: name.slice(0, -".jsonl".length), path: filePath };

    const result = guard(outcome, opts.source);
    if (!result.ok) {
      if (!opts.noMerge) {
        throw new Error(
          `${result.error} — refusing to continue; fix or remove the file, or re-run with --no-merge to replace it`
        );
      }
      warnings.push(`replacing stored session (--no-merge): ${result.error}`);
      continue;
    }

    const header = result.header;
    const previous = byCid.get(header.conversation_id);
    if (previous !== undefined) {
      const error =
        `duplicate session files for conversation "${header.conversation_id}": ` +
        `"${previous.fileName}.jsonl" and "${outcome.fileName}.jsonl"`;
      if (!opts.noMerge) {
        throw new Error(`${error} — refusing to continue; fix or remove one, or re-run with --no-merge`);
      }
      warnings.push(`replacing stored session (--no-merge): ${error}`);
      continue;
    }

    byCid.set(header.conversation_id, { fileName: outcome.fileName, path: filePath, header });
  }

  return { byCid, warnings };
}

/**
 * Fully load one stored session file (header + events) for the decay-safe
 * merge. Throws on any unparseable line — corrupt stored data must fail loudly
 * rather than be silently dropped. The mirror of `JsonlSink.writeSession`.
 */
export function loadStoredSession(filePath: string): SessionFile {
  const raw = fs.readFileSync(filePath, "utf8");
  const lines = raw.split("\n");
  // Trailing newline => last physical element is "".
  if (lines.length > 0 && lines[lines.length - 1] === "") {
    lines.pop();
  }
  if (lines.length === 0) {
    throw new Error(`corrupt session file "${filePath}": empty file`);
  }
  const header = parseHeaderLine(lines[0]!, filePath);

  const events: SessionEvent[] = [];
  for (let i = 1; i < lines.length; i++) {
    const line = lines[i]!;
    if (line === "") continue;
    try {
      events.push(JSON.parse(line) as SessionEvent);
    } catch {
      throw new Error(`corrupt session file "${filePath}": line ${i + 1} is not valid JSON`);
    }
  }
  return { header, events };
}

#!/usr/bin/env node
import { parseArgs as parseNodeArgs } from "node:util";
import { WarpSqliteAdapter } from "./adapters/warpSqliteAdapter.js";
import { WarpConversationReader } from "./adapters/warpConversationReader.js";
import { ClaudeCodeTranscriptReader } from "./adapters/claudeCodeTranscriptReader.js";
import { HermesConversationReader } from "./adapters/hermesConversationReader.js";
import { LangfuseConversationReader } from "./adapters/langfuseConversationReader.js";
import { LangfuseObservationClient } from "./adapters/langfuseObservationClient.js";
import { JsonlSink } from "./adapters/jsonlSink.js";
import { JsonlBundleReader } from "./adapters/jsonlBundleReader.js";
import { extractBundle } from "./usecases/extractBundle.js";
import type { ExtractResult } from "./usecases/extractBundle.js";
import { extractSessions } from "./usecases/extractSessions.js";
import { formatRunReport, formatSessionReport } from "./formatRunReport.js";
import type { ConversationReader, SessionReader } from "./domain/ports.js";
import type { CaptureBundle, SessionRunSummary } from "./domain/models.js";
import { parseSinceUtc } from "./domain/stamps.js";
import { SourceConfigurationError } from "./domain/sourceErrors.js";

type CliSource = "warp" | "claude-code" | "hermes" | "langfuse";

interface CliArgs {
  id?: string;
  noMerge: boolean;
  out?: string;
  dbPath?: string;
  hermesDbPath?: string;
  source?: CliSource;
  claudeRoot?: string;
  langfuseEnv?: string;
  sessions: boolean;
  since?: string;
  conversation?: string;
}

/** Attempt the parse; return the Error instead of throwing so callers can report it uniformly. */
function safeParseNodeArgs(argv: string[]) {
  try {
    return parseNodeArgs({
      args: argv,
      options: {
        id: { type: "string" },
        "no-merge": { type: "boolean", default: false },
        out: { type: "string" },
        "db-path": { type: "string" },
        "hermes-db-path": { type: "string" },
        source: { type: "string" },
        "claude-root": { type: "string" },
        "langfuse-env": { type: "string" },
        sessions: { type: "boolean", default: false },
        since: { type: "string" },
        conversation: { type: "string" },
        help: { type: "boolean", short: "h", default: false },
      },
      strict: true,
    });
  } catch (e) {
    return e as Error;
  }
}

function parseArgs(argv: string[]): CliArgs {
  const result = safeParseNodeArgs(argv);
  if (result instanceof Error) {
    console.error(`error: ${result.message}`);
    printUsage();
    process.exit(2);
  }
  const { values } = result;

  if (values.help) {
    printUsage();
    process.exit(0);
  }

  const source = values.source === undefined ? undefined : parseSource(values.source);

  return {
    id: values.id,
    noMerge: values["no-merge"],
    out: values.out,
    dbPath: values["db-path"],
    hermesDbPath: values["hermes-db-path"],
    source,
    claudeRoot: values["claude-root"],
    langfuseEnv: values["langfuse-env"],
    sessions: values.sessions,
    since: values.since,
    conversation: values.conversation,
  };
}

const SOURCE_REQUIRED =
  'error: --source is required for extraction ("warp", "claude-code", "hermes", or "langfuse")';

function parseSource(value: string): CliSource {
  if (value === "warp") return "warp";
  if (value === "claude-code") return "claude-code";
  if (value === "hermes") return "hermes";
  if (value === "langfuse") return "langfuse";
  console.error(
    `error: --source must be "warp", "claude-code", "hermes", or "langfuse" (got "${value}")`
  );
  printUsage();
  process.exit(2);
}

/** The id becomes a filename: refuse anything that could escape the output directory. */
function isSafeCaptureId(id: string): boolean {
  if (id.length === 0 || id.length > 200) return false;
  const first = id.charCodeAt(0);
  const alnum = (c: number) =>
    (c >= 48 && c <= 57) || (c >= 65 && c <= 90) || (c >= 97 && c <= 122);
  if (!alnum(first)) return false;
  for (let i = 1; i < id.length; i++) {
    const c = id.charCodeAt(i);
    if (!alnum(c) && c !== 46 && c !== 95 && c !== 45) return false;
  }
  return true;
}

function printUsage(): void {
  console.error(`usage: capture.sh --id <capture-id> --source warp|claude-code|hermes|langfuse [--no-merge] [--out dir] [--db-path path] [--claude-root dir] [--hermes-db-path path] [--langfuse-env file]
       capture.sh --sessions --source warp|claude-code|hermes|langfuse [--since <ISO-8601>] [--conversation <cid>] [--no-merge] [--out dir] [--db-path path] [--claude-root dir] [--hermes-db-path path] [--langfuse-env file]`);
}

/** Every source implements both reader ports (marker mode and `--sessions`). */
function readerFor(args: CliArgs & { source: CliSource }): ConversationReader & SessionReader {
  if (args.source === "claude-code") {
    return new ClaudeCodeTranscriptReader({ rootDir: args.claudeRoot });
  }
  if (args.source === "hermes") {
    return new HermesConversationReader({ dbPath: args.hermesDbPath });
  }
  if (args.source === "langfuse") {
    return new LangfuseConversationReader(
      new LangfuseObservationClient({ envFile: args.langfuseEnv })
    );
  }
  return new WarpConversationReader(
    new WarpSqliteAdapter({ liveDbPath: args.dbPath })
  );
}

/**
 * Detect a previously-captured bundle for decay-safe merge. Returns the loaded
 * bundle (or null when none exists) and whether --no-merge is replacing one.
 * Guards: a corrupt prior file or a capture_id mismatch errors loudly instead of
 * silently overwriting (default merge only; --no-merge skips loading entirely).
 */
function loadExistingBundle(
  captureId: string,
  outDir: string,
  noMerge: boolean,
  source: CliSource
): { existingBundle: CaptureBundle | null; replacedExisting: boolean } {
  const bundleReader = new JsonlBundleReader(outDir);
  if (!bundleReader.exists(captureId)) {
    return { existingBundle: null, replacedExisting: false };
  }
  if (noMerge) {
    return { existingBundle: null, replacedExisting: true };
  }
  let loaded: CaptureBundle | null;
  try {
    loaded = bundleReader.load(captureId);
  } catch (e) {
    console.error(`error: ${e instanceof Error ? e.message : String(e)}`);
    console.error(
      `refusing to overwrite "${bundleReader.pathFor(captureId)}" — fix or remove the corrupt bundle, or re-run with --no-merge to replace it.`
    );
    process.exit(1);
  }
  if (loaded && loaded.header.capture_id !== captureId) {
    console.error(
      `error: existing bundle capture_id mismatch: file has "${loaded.header.capture_id}", run expects "${captureId}"`
    );
    console.error(
      `refusing to overwrite "${bundleReader.pathFor(captureId)}" — this looks like a collision or manual tampering.`
    );
    process.exit(1);
  }
  if (loaded && loaded.header.source !== source) {
    console.error(
      `error: existing bundle source mismatch: file has "${loaded.header.source}", run requested "${source}"`
    );
    console.error(
      `refusing to merge "${bundleReader.pathFor(captureId)}" across sources — re-run with --no-merge to replace it.`
    );
    process.exit(1);
  }
  return { existingBundle: loaded, replacedExisting: false };
}

async function main(): Promise<void> {
  const args = parseArgs(process.argv.slice(2));

  // Cross-mode flag validation — usage errors before any I/O. Neither check
  // can fire on a marker-mode invocation without the new flags, so marker
  // mode's behavior stays byte-identical.
  if (!args.sessions && (args.since !== undefined || args.conversation !== undefined)) {
    console.error("error: --since and --conversation are only valid together with --sessions");
    printUsage();
    process.exit(2);
  }
  if (args.sessions && args.id !== undefined) {
    console.error(
      "error: --id cannot be combined with --sessions — session mode exports every conversation, no capture id"
    );
    printUsage();
    process.exit(2);
  }

  if (args.sessions) {
    if (!args.source) {
      console.error(SOURCE_REQUIRED);
      printUsage();
      process.exit(2);
    }
    if (args.since !== undefined && parseSinceUtc(args.since) === null) {
      console.error(
        `error: --since "${args.since}" is not a parseable ISO-8601 date or datetime`
      );
      printUsage();
      process.exit(2);
    }
    await runSessionsMode({ ...args, sessions: true, source: args.source });
    return;
  }

  if (!args.id) {
    console.error("error: --id <capture-id> is required");
    printUsage();
    process.exit(2);
  }

  if (!isSafeCaptureId(args.id)) {
    console.error(
      `error: --id "${args.id}" is not allowed: use letters, digits, ".", "_" or "-" (it names the output file), starting with a letter or digit`
    );
    process.exit(2);
  }

  const source = args.source;
  if (!source) {
    console.error(SOURCE_REQUIRED);
    printUsage();
    process.exit(2);
  }

  const outDir = args.out ?? "out";
  const { existingBundle, replacedExisting } = loadExistingBundle(
    args.id,
    outDir,
    args.noMerge,
    source
  );

  const reader = readerFor({ ...args, source });
  let result: ExtractResult;
  try {
    result = await extractBundle(reader, args.id, {
      noMerge: args.noMerge,
      existingBundle,
    });
  } catch (e) {
    console.error(`error: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(e instanceof SourceConfigurationError ? 2 : 1);
  }

  const { bundle, summary } = result;

  // Never write a header-only file: zero bound conversations means there is
  // nothing worth persisting.
  let outPath: string | null = null;
  if (summary.conversations > 0) {
    outPath = new JsonlSink(outDir).write(bundle);
  }

  const report = formatRunReport(summary, outPath);
  if (replacedExisting && outPath !== null) {
    report.stderr.unshift(`replacing existing bundle (--no-merge): ${outPath}`);
  }
  for (const line of report.stdout) console.log(line);
  for (const line of report.stderr) console.error(line);
  process.exit(report.exitCode);
}

/**
 * The `--sessions` run: dispatch through the same readerFor, then extract +
 * report. Exit codes: 2 usage (handled above) or source configuration (missing
 * Langfuse credentials), 1 fresh-read failure or zero sessions enumerated,
 * 0 otherwise.
 */
async function runSessionsMode(
  args: CliArgs & { sessions: true; source: CliSource }
): Promise<void> {
  const outDir = args.out ?? "out";
  let summary: SessionRunSummary;
  try {
    ({ summary } = await extractSessions(readerFor(args), {
      outDir,
      source: args.source,
      since: args.since,
      conversation: args.conversation,
      noMerge: args.noMerge,
    }));
  } catch (e) {
    console.error(`error: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(e instanceof SourceConfigurationError ? 2 : 1);
  }

  const report = formatSessionReport(summary);
  for (const line of report.stdout) console.log(line);
  for (const line of report.stderr) console.error(line);
  process.exit(report.exitCode);
}

await main();

#!/usr/bin/env node
import { parseArgs as parseNodeArgs } from "node:util";
import { WarpSqliteAdapter } from "./adapters/warpSqliteAdapter.js";
import { WarpConversationReader } from "./adapters/warpConversationReader.js";
import { ClaudeCodeTranscriptReader } from "./adapters/claudeCodeTranscriptReader.js";
import { HermesConversationReader } from "./adapters/hermesConversationReader.js";
import { JsonlSink } from "./adapters/jsonlSink.js";
import { JsonlBundleReader } from "./adapters/jsonlBundleReader.js";
import { extractBundle } from "./usecases/extractBundle.js";
import type { ExtractResult } from "./usecases/extractBundle.js";
import { formatRunReport } from "./formatRunReport.js";
import type { ConversationReader } from "./domain/ports.js";
import type { CaptureBundle } from "./domain/models.js";

type CliSource = "warp" | "claude-code" | "hermes";

interface CliArgs {
  id?: string;
  noMerge: boolean;
  out?: string;
  dbPath?: string;
  hermesDbPath?: string;
  source?: CliSource;
  claudeRoot?: string;
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
  };
}

function parseSource(value: string): CliSource {
  if (value === "warp") return "warp";
  if (value === "claude-code") return "claude-code";
  if (value === "hermes") return "hermes";
  console.error(
    `error: --source must be "warp", "claude-code", or "hermes" (got "${value}")`
  );
  printUsage();
  process.exit(2);
}

function printUsage(): void {
  console.error(`usage: tsx src/cli.ts --id <capture-id> --source warp|claude-code|hermes [--no-merge] [--out dir] [--db-path path] [--claude-root dir] [--hermes-db-path path]`);
}

function readerFor(args: CliArgs & { source: CliSource }): ConversationReader {
  if (args.source === "claude-code") {
    return new ClaudeCodeTranscriptReader({ rootDir: args.claudeRoot });
  }
  if (args.source === "hermes") {
    return new HermesConversationReader({ dbPath: args.hermesDbPath });
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

function main(): void {
  const args = parseArgs(process.argv.slice(2));

  if (!args.id) {
    console.error("error: --id <capture-id> is required");
    printUsage();
    process.exit(2);
  }

  const source = args.source;
  if (!source) {
    console.error(
      'error: --source is required for extraction ("warp", "claude-code", or "hermes")'
    );
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
    result = extractBundle(reader, args.id, {
      noMerge: args.noMerge,
      existingBundle,
    });
  } catch (e) {
    console.error(`error: ${e instanceof Error ? e.message : String(e)}`);
    process.exit(1);
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

main();

import fs from "node:fs";
import path from "node:path";
import type { BundleHeader, ConversationEvent, CaptureBundle } from "../domain/models.js";

/**
 * Reads a previously-captured JSONL bundle back from disk — the mirror of
 * {@link JsonlSink}. Line 1 is the bundle_header; every subsequent line is one
 * time-ordered event.
 *
 * Returns `null` when the file does not exist (first capture). Throws when the
 * file exists but is unparseable (corrupt/truncated): the caller decides whether
 * to error loudly (default merge) or replace via `--no-merge`.
 */
export class JsonlBundleReader {
  constructor(private readonly outDir: string) {}

  pathFor(captureId: string): string {
    return path.join(this.outDir, `${captureId}.jsonl`);
  }

  exists(captureId: string): boolean {
    return fs.existsSync(this.pathFor(captureId));
  }

  load(captureId: string): CaptureBundle | null {
    const outPath = this.pathFor(captureId);
    if (!fs.existsSync(outPath)) return null;

    const raw = fs.readFileSync(outPath, "utf8");
    const lines = raw.split("\n");
    // Trailing newline => last physical element is "".
    if (lines.length > 0 && lines[lines.length - 1] === "") {
      lines.pop();
    }
    if (lines.length === 0) {
      throw new Error(`corrupt bundle "${outPath}": empty file`);
    }

    let header: BundleHeader;
    try {
      header = JSON.parse(lines[0]!) as BundleHeader;
    } catch {
      throw new Error(`corrupt bundle "${outPath}": header line is not valid JSON`);
    }
    if (header.type !== "bundle_header") {
      throw new Error(
        `corrupt bundle "${outPath}": first line is not a bundle_header`
      );
    }

    const events: ConversationEvent[] = [];
    for (let i = 1; i < lines.length; i++) {
      const line = lines[i]!;
      if (line === "") continue;
      try {
        events.push(JSON.parse(line) as ConversationEvent);
      } catch {
        throw new Error(
          `corrupt bundle "${outPath}": line ${i + 1} is not valid JSON`
        );
      }
    }
    return { header, events };
  }
}

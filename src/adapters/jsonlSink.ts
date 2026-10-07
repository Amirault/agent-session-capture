import fs from "node:fs";
import path from "node:path";
import type { ConversationSink } from "../domain/ports.js";
import type { CaptureBundle } from "../domain/models.js";

/**
 * Strict NDJSON (JSONL) sink: one compact JSON object per line.
 *
 * Line 1 is the bundle_header; every subsequent line is one time-ordered event.
 * `JSON.stringify` escapes newlines and quotes inside string values, so each
 * serialized object is guaranteed to be a single physical line — no raw
 * newlines leak into the output. The file ends with a trailing newline.
 */
export class JsonlSink implements ConversationSink {
  constructor(private readonly outDir: string) {}

  write(bundle: CaptureBundle): string {
    fs.mkdirSync(this.outDir, { recursive: true });
    const outPath = path.join(this.outDir, `${bundle.header.capture_id}.jsonl`);
    const lines = [
      JSON.stringify(bundle.header),
      ...bundle.events.map((e) => JSON.stringify(e)),
    ];
    const content = `${lines.join("\n")}\n`;
    // Atomic write: stage to a temp file in the same directory, then rename.
    // rename is atomic on the same filesystem, so a torn write (interrupt,
    // crash) never leaves a half-written bundle visible at the target path —
    // the prior bundle stays intact until the new one is fully staged.
    const tmpPath = `${outPath}.${process.pid}.${Date.now()}.tmp`;
    fs.writeFileSync(tmpPath, content, "utf8");
    fs.renameSync(tmpPath, outPath);
    return outPath;
  }
}

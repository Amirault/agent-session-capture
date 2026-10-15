import fs from "node:fs";
import path from "node:path";
import type { ConversationSink } from "../domain/ports.js";
import type { CaptureBundle, SessionEvent, SessionHeader } from "../domain/models.js";
import { sessionFileName } from "./sessionFileName.js";

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

  /**
   * Write one exported session (the `--sessions` mode) as strict JSONL under
   * `<outDir>/sessions/<file>.jsonl` — the subdir keeps marker bundles and
   * session exports disjoint. Line 1 is the session_header; every subsequent
   * line is one time-ordered session event. Same atomic write mechanics as
   * `write` (stage to a temp file in the same directory, then rename).
   */
  writeSession(header: SessionHeader, events: SessionEvent[]): string {
    const sessionsDir = path.join(this.outDir, "sessions");
    fs.mkdirSync(sessionsDir, { recursive: true });
    const fileName = sessionFileName(header.conversation_id);
    const outPath = path.join(sessionsDir, `${fileName}.jsonl`);
    const lines = [
      JSON.stringify(header),
      ...events.map((e) => JSON.stringify(e)),
    ];
    const content = `${lines.join("\n")}\n`;
    const tmpPath = `${outPath}.${process.pid}.${Date.now()}.tmp`;
    fs.writeFileSync(tmpPath, content, "utf8");
    fs.renameSync(tmpPath, outPath);
    return outPath;
  }
}

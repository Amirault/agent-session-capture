import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { JsonlSink } from "../adapters/jsonlSink.js";
import type { ConversationEvent, CaptureBundle } from "../domain/models.js";

function header(captureId: string): CaptureBundle["header"] {
  return {
    type: "bundle_header",
    capture_id: captureId,
    labels: ["specify", "implement", "review"],
    conversations_per_label: { specify: 1, implement: 1, "review": 1 },
    conversation_ids: ["c1", "c2", "c3"],
    extracted_at: "2026-06-30T12:00:00.000Z",
    source: "warp",
  };
}

function event(seq: number, content: string): ConversationEvent {
  return {
    capture_id: "add-feature-x",
    label: "specify",
    conversation_id: "c1",
    seq,
    ts: `2026-06-30 10:0${seq}:00.000000`,
    role: "assistant",
    kind: "agent_message",
    content,
    meta: { field_path: "1", note: 'has "quotes" and \n newline' },
  };
}

describe("§9.9 jsonlSink JSONL validity", () => {
  let tmp: string;
  let outDir: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-99-"));
    outDir = path.join(tmp, "out");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a bundle with values containing newlines and quotes, When written, Then every line parses, line 1 is the header, no raw newline leaks, and it round-trips", () => {
    // Given
    const events = [
      event(1, 'line one\nwith newline and "quotes"'),
      event(2, "plain content"),
      event(3, "multi\nline\ncontent"),
    ];
    const bundle: CaptureBundle = { header: header("add-feature-x"), events };

    // When
    const sink = new JsonlSink(outDir);
    const outPath = sink.write(bundle);
    expect(outPath).toBe(path.join(outDir, "add-feature-x.jsonl"));

    // Then — read back the raw file and split into physical lines
    const raw = fs.readFileSync(outPath, "utf8");
    const lines = raw.split("\n");
    // trailing newline => last element is ""
    expect(lines[lines.length - 1]).toBe("");
    const jsonLines = lines.slice(0, -1);

    // every line is valid JSON (a raw newline in a value would break this)
    const parsed = jsonLines.map((l) => JSON.parse(l));

    // line 1 is the bundle_header
    expect(parsed[0]!.type).toBe("bundle_header");
    expect(parsed[0]!.capture_id).toBe("add-feature-x");

    // exactly 1 header + N event lines
    expect(jsonLines).toHaveLength(1 + events.length);

    // no raw newline inside any value: each serialized line contains no literal \n
    for (const line of jsonLines) {
      expect(line.includes("\n")).toBe(false);
    }

    // round-trip: header + events reconstructed
    const roundTripped: CaptureBundle = {
      header: parsed[0] as CaptureBundle["header"],
      events: parsed.slice(1) as ConversationEvent[],
    };
    expect(roundTripped).toEqual(bundle);
  });

  it("Given a bundle written atomically, When the write completes, Then no temp file remains in the output directory", () => {
    // Given
    const bundle: CaptureBundle = { header: header("add-feature-x"), events: [event(1, "plain")] };

    // When
    new JsonlSink(outDir).write(bundle);

    // Then — only the target .jsonl exists; no leftover .tmp staging files
    const files = fs.readdirSync(outDir);
    expect(files).toEqual(["add-feature-x.jsonl"]);
    expect(files.filter((f) => f.endsWith(".tmp"))).toEqual([]);
  });
});

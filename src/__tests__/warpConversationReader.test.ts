import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createFixture, seedMarker, seedQuery } from "./fixtures/fixtureDb.js";
import { fakeSource } from "./fixtures/fakeSource.js";
import { WarpConversationReader } from "../adapters/warpConversationReader.js";

const SPEC = "add-feature-x";

describe("§9.12 WarpConversationReader — spec binding + event gathering", () => {
  let tmp: string;
  let dbPath: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-912-"));
    dbPath = path.join(tmp, "f.db");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a bound conversation plus an unbindable marker, When readCapture, Then labelByCid binds the conversation, drafts hold its events, and the unbindable marker is surfaced", () => {
    // Given — one bound specify conversation (c1) with a query, plus an unbindable implement marker
    const db = createFixture(dbPath);
    seedMarker(db, { capture_id: SPEC, label: "specify", conversation_id: "c1", start_ts: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 10:05:00.000000", text: "spec it" });
    db.prepare(`INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)`).run(
      `: CAPTURE_MARKER v=1 id=${SPEC} label=implement`,
      "2026-06-30 11:00:00.000000"
    );

    // When
    const read = new WarpConversationReader(fakeSource(db)).readCapture(SPEC);

    // Then — c1 bound to specify; its query plus the marker's binding block are
    // gathered (queries before blocks), and the implement marker is unbindable.
    expect(read.labelByCid.get("c1")).toBe("specify");
    expect(read.labelByCid.size).toBe(1);
    expect(read.drafts).toHaveLength(2);
    expect(read.drafts[0]!.conversation_id).toBe("c1");
    expect(read.drafts[0]!.kind).toBe("query");
    expect(read.drafts[0]!.content).toBe("spec it");
    expect(read.drafts[1]!.conversation_id).toBe("c1");
    expect(read.drafts[1]!.kind).toBe("command");
    expect(read.drafts[1]!.content).toBe(
      ": CAPTURE_MARKER v=1 id=add-feature-x label=specify"
    );
    expect(read.unbindable).toHaveLength(1);
    expect(read.unbindable[0]!.label).toBe("implement");
    expect(read.collisions).toEqual([]);
    expect(read.skipped).toEqual([]);
    db.close();
  });
});

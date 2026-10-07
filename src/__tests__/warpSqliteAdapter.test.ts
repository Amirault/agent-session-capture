import { describe, it, expect, beforeEach, afterEach } from "vitest";
import crypto from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createFixture, seedMarker } from "./fixtures/fixtureDb.js";
import {
  WarpDbNotFoundError,
  WarpSqliteAdapter,
} from "../adapters/warpSqliteAdapter.js";

function hashFile(p: string): string {
  if (!fs.existsSync(p)) return "<missing>";
  return crypto.createHash("sha256").update(fs.readFileSync(p)).digest("hex");
}

describe("§9.1 warpSqliteAdapter — snapshot & DB safety", () => {
  let tmp: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-91-"));
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a WAL-mode DB with committed -wal data, When snapshotted, Then the snapshot includes the committed rows and the live files are never modified", () => {
    // Given — writer stays open so the committed row lives in the -wal (not checkpointed)
    const livePath = path.join(tmp, "live.db");
    const writer = createFixture(livePath, "wal");
    seedMarker(writer, {
      capture_id: "smoke",
      label: "specify",
      conversation_id: "c-seed",
      start_ts: "2026-06-30 19:34:14.579587",
    });
    const beforeDb = hashFile(livePath);
    const beforeWal = hashFile(`${livePath}-wal`);

    // When
    const adapter = new WarpSqliteAdapter({ liveDbPath: livePath });
    let rows: { command: string }[] = [];
    adapter.withSnapshot((db) => {
      rows = db.all<{ command: string }>("SELECT command FROM commands");
    });

    // Then
    expect(rows.map((r) => r.command)).toContain(
      ": CAPTURE_MARKER v=1 id=smoke label=specify"
    );
    expect(hashFile(livePath)).toBe(beforeDb);
    expect(hashFile(`${livePath}-wal`)).toBe(beforeWal);
    writer.close();
  });

  it("Given the live DB path does not exist, When the adapter resolves it, Then it fails with a clear error listing probed paths", () => {
    const missing = path.join(tmp, "does-not-exist.db");
    const adapter = new WarpSqliteAdapter({ liveDbPath: missing });

    expect(() => adapter.resolveLiveDbPath()).toThrow(WarpDbNotFoundError);
    expect(() => adapter.resolveLiveDbPath()).toThrow(/warp\.sqlite not found/);
  });

  it("Given a snapshot, When opened, Then it is read-only and any write attempt throws", () => {
    const livePath = path.join(tmp, "live.db");
    const writer = createFixture(livePath);
    seedMarker(writer, {
      capture_id: "smoke",
      label: "specify",
      conversation_id: "c-seed",
      start_ts: "2026-06-30 19:34:14.579587",
    });
    const adapter = new WarpSqliteAdapter({ liveDbPath: livePath });

    adapter.withSnapshot((db) => {
      expect(() => db.all("CREATE TABLE should_fail(id INTEGER)")).toThrow();
    });
    writer.close();
  });
});

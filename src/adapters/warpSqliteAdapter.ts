import Database from "better-sqlite3";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { ConversationSource, ReadableDb } from "../domain/ports.js";
import { wrapReadableDb } from "./sqliteReadableDb.js";

/** Known relative locations of the Warp SQLite DB (macOS). */
const PROBE_REL_PATHS: readonly string[] = [
  "Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.Warp-Stable/warp.sqlite",
];

/** Thrown when the live Warp DB cannot be located. Lists every probed path. */
export class WarpDbNotFoundError extends Error {
  constructor(public readonly probedPaths: readonly string[]) {
    super(
      `warp.sqlite not found. Probed:\n${probedPaths.map((p) => `  - ${p}`).join("\n")}`
    );
    this.name = "WarpDbNotFoundError";
  }
}

export interface WarpSqliteAdapterOptions {
  /** Override the live DB path (used by tests / explicit CLI usage). */
  liveDbPath?: string;
  /** Where to place the ephemeral snapshot (default: a fresh temp dir). */
  snapshotDir?: string;
  /** Keep the snapshot on disk after the run (debugging). */
  keepSnapshot?: boolean;
}

/**
 * ConversationSource backed by Warp's local SQLite DB.
 *
 * Safety: the live DB is opened read-only and never written. Work runs against a
 * VACUUM INTO snapshot (captures committed WAL pages) that is itself opened
 * read-only and deleted once the run finishes.
 */
export class WarpSqliteAdapter implements ConversationSource {
  constructor(private readonly opts: WarpSqliteAdapterOptions = {}) {}

  resolveLiveDbPath(): string {
    const override = this.opts.liveDbPath;
    if (override) {
      if (!fs.existsSync(override)) throw new WarpDbNotFoundError([override]);
      return override;
    }
    const home = os.homedir();
    const probed = PROBE_REL_PATHS.map((p) => path.join(home, p));
    const found = probed.find((p) => fs.existsSync(p));
    if (!found) throw new WarpDbNotFoundError(probed);
    return found;
  }

  withSnapshot<T>(fn: (db: ReadableDb) => T): T {
    const livePath = this.resolveLiveDbPath();
    const snapDir =
      this.opts.snapshotDir ?? fs.mkdtempSync(path.join(os.tmpdir(), "warp-snap-"));
    const snapPath = path.join(snapDir, `snap-${process.pid}-${Date.now()}.db`);

    const cleanup = (): void => {
      if (this.opts.keepSnapshot) return;
      fs.rmSync(snapPath, { force: true });
      if (!this.opts.snapshotDir) {
        fs.rmSync(snapDir, { force: true, recursive: true });
      }
    };

    try {
      this.createSnapshot(livePath, snapPath);
      if (!fs.existsSync(snapPath)) {
        throw new Error(`VACUUM INTO did not produce a snapshot at ${snapPath}`);
      }
      const snap = new Database(snapPath, { readonly: true, fileMustExist: true });
      try {
        return fn(wrapReadableDb(snap));
      } finally {
        snap.close();
      }
    } finally {
      cleanup();
    }
  }

  /**
   * Copy the live DB (committed WAL pages included) into a fresh file without
   * modifying the source. The source connection is read-only as a hard guarantee.
   */
  private createSnapshot(livePath: string, snapPath: string): void {
    const live = new Database(livePath, { readonly: true, fileMustExist: true });
    try {
      live.pragma("busy_timeout = 5000");
      // Escape single quotes for the SQL string literal (no regex).
      const escaped = snapPath.split("'").join("''");
      live.exec(`VACUUM INTO '${escaped}'`);
    } finally {
      live.close();
    }
  }
}

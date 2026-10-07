import type { ConversationSource, ReadableDb } from "../../domain/ports.js";
import { wrapReadableDb } from "../../adapters/sqliteReadableDb.js";
import type { FixtureDb } from "./fixtureDb.js";

/**
 * A ConversationSource that runs directly against an already-open fixture DB
 * (no VACUUM INTO copy). Used by tests that exercise the use-case + sink pipeline
 * without touching the real Warp DB.
 */
export function fakeSource(db: FixtureDb): ConversationSource {
  return {
    withSnapshot<T>(fn: (db: ReadableDb) => T): T {
      return fn(wrapReadableDb(db));
    },
    resolveLiveDbPath(): string {
      return "/fake/warp.sqlite";
    },
  };
}

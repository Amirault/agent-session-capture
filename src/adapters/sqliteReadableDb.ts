import Database from "better-sqlite3";
import type { ReadableDb } from "../domain/ports.js";

/** Wrap a better-sqlite3 connection as the port ReadableDb. Shared by adapter and tests. */
export function wrapReadableDb(db: Database.Database): ReadableDb {
  return {
    all<T = unknown>(sql: string, ...params: unknown[]): T[] {
      return db.prepare(sql).all(...params) as T[];
    },
  };
}

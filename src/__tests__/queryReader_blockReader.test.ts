import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createFixture, seedBlock, seedQuery } from "./fixtures/fixtureDb.js";
import { wrapReadableDb } from "../adapters/sqliteReadableDb.js";
import { readQueries } from "../adapters/readers/queryReader.js";
import { readBlocks } from "../adapters/readers/blockReader.js";

const CID = "c-1";

describe("§9.5 queryReader & blockReader", () => {
  let tmp: string;
  let dbPath: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-95-"));
    dbPath = path.join(tmp, "f.db");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given an ai_queries.input with Query.text, When parsed, Then a kind:query user event is produced with cwd/git/model metadata", () => {
    // Given — a block before the query provides git_branch context
    const db = createFixture(dbPath);
    seedBlock(db, {
      conversation_id: CID,
      start_ts: "2026-06-30 10:00:00.000000",
      command: "git status",
      git_branch: "feature-x",
    });
    seedQuery(db, {
      conversation_id: CID,
      start_ts: "2026-06-30 10:05:00.000000",
      text: "fix the bug",
      working_directory: "/repo",
      model_id: "claude-x",
    });

    // When
    const { drafts, skipped } = readQueries(wrapReadableDb(db), [CID]);

    // Then
    expect(skipped).toEqual([]);
    expect(drafts).toHaveLength(1);
    expect(drafts[0]).toMatchObject({
      role: "user",
      kind: "query",
      conversation_id: CID,
      content: "fix the bug",
    });
    expect(drafts[0]!.meta).toMatchObject({
      cwd: "/repo",
      model: "claude-x",
      git_branch: "feature-x",
      exchange_id: `${CID}-ex`,
    });
    db.close();
  });

  it("Given ai_queries.input that fails the zod schema, When parsed, Then the row is logged and skipped and extraction continues", () => {
    // Given — a malformed input row plus a valid one
    const db = createFixture(dbPath);
    db.prepare(
      `INSERT INTO ai_queries (exchange_id, conversation_id, start_ts, input, working_directory, output_status, model_id)
       VALUES (?, ?, ?, ?, ?, '', ?)`
    ).run(
      "ex-bad",
      CID,
      "2026-06-30 10:00:00.000000",
      JSON.stringify([{ NotQuery: {} }]),
      "/repo",
      "m"
    );
    seedQuery(db, {
      conversation_id: CID,
      start_ts: "2026-06-30 10:05:00.000000",
      text: "good query",
    });

    // When
    const { drafts, skipped } = readQueries(wrapReadableDb(db), [CID]);

    // Then
    expect(drafts).toHaveLength(1);
    expect(drafts[0]!.content).toBe("good query");
    expect(skipped).toHaveLength(1);
    expect(skipped[0]!.table).toBe("ai_queries");
    db.close();
  });

  it("Given a blocks row with ANSI stylized_output, When parsed, Then ANSI is stripped and a kind:command tool event with clean output is produced", () => {
    // Given
    const db = createFixture(dbPath);
    seedBlock(db, {
      conversation_id: CID,
      start_ts: "2026-06-30 10:00:00.000000",
      command: "git pull origin main",
      output: "Already up to date.\nDone",
      git_branch: "main",
    });

    // When
    const { drafts, skipped } = readBlocks(wrapReadableDb(db), [CID]);

    // Then
    expect(skipped).toEqual([]);
    expect(drafts).toHaveLength(1);
    expect(drafts[0]).toMatchObject({
      role: "tool",
      kind: "command",
      conversation_id: CID,
    });
    expect(drafts[0]!.content).toBe("git pull origin main");
    expect(drafts[0]!.meta.output).toBe("Already up to date.\nDone");
    expect(drafts[0]!.meta.block_id).toBe(1);
    db.close();
  });

  it("Given no conversation ids, When reading, Then empty results (no empty IN clause)", () => {
    const db = createFixture(dbPath);
    expect(readQueries(wrapReadableDb(db), [])).toEqual({ drafts: [], skipped: [] });
    expect(readBlocks(wrapReadableDb(db), [])).toEqual({ drafts: [], skipped: [] });
    db.close();
  });
});

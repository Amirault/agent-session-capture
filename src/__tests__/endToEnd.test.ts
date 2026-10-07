import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { execSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { createFixture, seedMarker, seedQuery, seedBlock, seedTask } from "./fixtures/fixtureDb.js";
import { encodeString } from "./fixtures/protobuf.js";
import type { Label } from "../domain/models.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(TEST_DIR, "..", "..");
const TSX = path.join(ROOT, "node_modules", ".bin", "tsx");
const SPEC = "add-feature-x";

describe("CLI source selection", () => {
  it("Given a spec extraction without --source, When the CLI runs, Then it fails before reading or writing a bundle", () => {
    // Given
    const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "capture-source-required-"));
    const outDir = path.join(tmp, "out");
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    const sentinel = "existing bundle must not be read or overwritten\n";
    fs.mkdirSync(outDir, { recursive: true });
    fs.writeFileSync(outPath, sentinel, "utf8");

    try {
      // When
      const result = spawnSync(
        TSX,
        ["src/cli.ts", "--id", SPEC, "--out", outDir],
        {
          cwd: ROOT,
          encoding: "utf8",
          timeout: 30000,
        }
      );

      // Then
      expect(result.status).toBe(2);
      expect(result.stderr).toContain(
        'error: --source is required for extraction ("warp", "claude-code", or "hermes")'
      );
      expect(fs.readFileSync(outPath, "utf8")).toBe(sentinel);
    } finally {
      fs.rmSync(tmp, { recursive: true, force: true });
    }
  });
});

describe("§9.10 end-to-end (CLI -> snapshot adapter -> use-case -> sink)", () => {
  let tmp: string;
  let dbPath: string;
  let outDir: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-910-"));
    dbPath = path.join(tmp, "warp.sqlite");
    outDir = path.join(tmp, "out");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a fixture DB with one spec across 3 labels incl. subagent + re-run gate, When the CLI runs, Then out/<spec>.jsonl is valid, complete, time-ordered, and includes subagent rows", () => {
    // Given — a file-backed fixture DB (the real adapter will VACUUM INTO it)
    const db = createFixture(dbPath);
    // specify (c1)
    seedMarker(db, { capture_id: SPEC, label: "specify", conversation_id: "c1", start_ts: "2026-06-30 09:00:00.000000" });
    seedQuery(db, { conversation_id: "c1", start_ts: "2026-06-30 09:10:00.000000", text: "spec it" });
    // implement (c2) with a subagent block + subagent task sharing the parent conversation_id
    seedMarker(db, { capture_id: SPEC, label: "implement", conversation_id: "c2", start_ts: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: "c2", start_ts: "2026-06-30 10:10:00.000000", text: "implement it" });
    seedBlock(db, { conversation_id: "c2", start_ts: "2026-06-30 10:20:00.000000", command: "npm test", subagent_task_id: "sub-1" });
    seedTask(db, { conversation_id: "c2", task: encodeString(1, "subagent message"), last_modified_at: "2026-06-30 10:30:00.000000" });
    // review (c3) + a re-run gate (c4)
    seedMarker(db, { capture_id: SPEC, label: "review", conversation_id: "c3", start_ts: "2026-06-30 11:00:00.000000" });
    seedQuery(db, { conversation_id: "c3", start_ts: "2026-06-30 11:10:00.000000", text: "gate run 1" });
    seedMarker(db, { capture_id: SPEC, label: "review", conversation_id: "c4", start_ts: "2026-06-30 12:00:00.000000" });
    seedQuery(db, { conversation_id: "c4", start_ts: "2026-06-30 12:10:00.000000", text: "gate run 2" });
    db.close();

    // When — run the actual CLI against the fixture DB
    const stdout = execSync(`"${TSX}" src/cli.ts --id ${SPEC} --source warp --db-path "${dbPath}" --out "${outDir}"`, {
      cwd: ROOT,
      encoding: "utf8",
      timeout: 30000,
    });

    // Then — the bundle file was written
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    expect(fs.existsSync(outPath)).toBe(true);
    expect(stdout).toContain("wrote");

    // JSONL validity: every line parses, no raw newline leaks into a value
    const raw = fs.readFileSync(outPath, "utf8");
    const lines = raw.split("\n").slice(0, -1); // drop trailing "" from final newline
    const parsed = lines.map((l) => JSON.parse(l));
    for (const line of lines) expect(line.includes("\n")).toBe(false);

    // line 1 is the header — complete, all 3 labels, gate has 2 conversations
    const header = parsed[0]!;
    expect(header.type).toBe("bundle_header");
    expect(header.capture_id).toBe(SPEC);
    expect(header.source).toBe("warp");
    expect(header.labels).toEqual(["specify", "implement", "review"]);
    expect(header.conversations_per_label["review"]).toBe(2);

    const events = parsed.slice(1);

    // subagent rows are included: the subagent block carries subagent_task_id,
    // and the subagent task's walked string is present
    expect(events.find((e) => e.meta?.subagent_task_id === "sub-1")).toBeDefined();
    expect(events.some((e) => e.content === "subagent message")).toBe(true);

    // all three labels appear among events
    expect(new Set(events.map((e) => e.label))).toEqual(
      new Set(["specify", "implement", "review"])
    );

    // time-ordered with a monotonic 1..N sequence
    const ts = events.map((e) => e.ts);
    expect([...ts].sort()).toEqual(ts);
    expect(events.map((e) => e.seq)).toEqual(events.map((_, i) => i + 1));
  });
});

describe("§9.18 end-to-end merge + guards", () => {
  let tmp: string;
  let outDir: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-918-"));
    outDir = path.join(tmp, "out");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  function fixtureDbWithLabel(label: Label, cid: string): string {
    const dbPath = path.join(tmp, `${label}.sqlite`);
    const db = createFixture(dbPath);
    seedMarker(db, { capture_id: SPEC, label, conversation_id: cid, start_ts: "2026-06-30 10:00:00.000000" });
    seedQuery(db, { conversation_id: cid, start_ts: "2026-06-30 10:10:00.000000", text: `${label} it` });
    db.close();
    return dbPath;
  }

  function runCli(dbPath: string, extraArgs: string[] = []): {
    stdout: string;
    stderr: string;
    exitCode: number;
  } {
    const result = spawnSync(TSX, ["src/cli.ts", "--id", SPEC, "--source", "warp", "--db-path", dbPath, "--out", outDir, ...extraArgs], {
      cwd: ROOT,
      encoding: "utf8",
      timeout: 30000,
    });
    return {
      stdout: result.stdout ?? "",
      stderr: result.stderr ?? "",
      exitCode: result.status ?? 1,
    };
  }

  function readBundle(): { header: Record<string, unknown>; events: Record<string, unknown>[] } {
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    const raw = fs.readFileSync(outPath, "utf8");
    const lines = raw.split("\n").slice(0, -1);
    const parsed = lines.map((l) => JSON.parse(l));
    return { header: parsed[0]!, events: parsed.slice(1) };
  }

  it("Given a first capture (specify) then a second (implement), When the CLI runs twice (default merge), Then the merged bundle has both labels", () => {
    // Given — first run writes specify
    runCli(fixtureDbWithLabel("specify", "c1"));
    // second run writes implement (merges with existing specify)
    runCli(fixtureDbWithLabel("implement", "c2"));

    // Then — merged bundle has both labels (specify from disk, implement from
    // live), specify not lost, and no duplicate events (dedup by natural key)
    const { header, events } = readBundle();
    expect([...(header.labels as string[])].sort()).toEqual(["implement", "specify"]);
    expect(new Set(events.map((e) => e.label))).toEqual(new Set(["specify", "implement"]));
    expect(events.some((e) => e.label === "specify")).toBe(true);
    const keys = events.map((e) => `${e.conversation_id}:${e.ts}:${e.kind}:${e.content}`);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("Given two captures of one task row with different last_modified_at values, When merged, Then the task event is not duplicated and keeps the earlier timestamp", () => {
    // Given
    const task = encodeString(1, "same task event");
    const firstDbPath = path.join(tmp, "task-first.sqlite");
    const firstDb = createFixture(firstDbPath);
    seedMarker(firstDb, {
      capture_id: SPEC,
      label: "review",
      conversation_id: "c1",
      start_ts: "2026-06-30 08:00:00.000000",
    });
    seedTask(firstDb, {
      conversation_id: "c1",
      task_id: "stable-task",
      task,
      last_modified_at: "2026-06-30 09:00:00.000000",
    });
    firstDb.close();

    const secondDbPath = path.join(tmp, "task-second.sqlite");
    const secondDb = createFixture(secondDbPath);
    seedMarker(secondDb, {
      capture_id: SPEC,
      label: "review",
      conversation_id: "c1",
      start_ts: "2026-06-30 08:00:00.000000",
    });
    seedTask(secondDb, {
      conversation_id: "c1",
      task_id: "stable-task",
      task,
      last_modified_at: "2026-06-30 10:00:00.000000",
    });
    secondDb.close();
    runCli(firstDbPath);

    // When
    runCli(secondDbPath);

    // Then
    const taskEvents = readBundle().events.filter((event) => event.content === "same task event");
    expect(taskEvents).toHaveLength(1);
    expect(taskEvents[0]!.ts).toBe("2026-06-30 09:00:00.000000");
  });

  it("Given an existing bundle and --no-merge, When the CLI runs, Then the existing bundle is replaced with a fresh write and a warning is emitted", () => {
    // Given — first run writes specify
    runCli(fixtureDbWithLabel("specify", "c1"));
    // second run with --no-merge writes implement (replaces, no merge)
    const result = runCli(fixtureDbWithLabel("implement", "c2"), ["--no-merge"]);

    // Then — warning emitted, bundle has implement only (specify gone)
    expect(result.exitCode).toBe(0);
    expect(result.stderr).toContain("replacing existing bundle");
    const { header } = readBundle();
    expect(header.labels).toEqual(["implement"]);
  });

  it("Given a corrupt existing bundle, When the CLI runs (default merge), Then it errors loudly and does not overwrite", () => {
    // Given — write a corrupt bundle file
    fs.mkdirSync(outDir, { recursive: true });
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    fs.writeFileSync(outPath, "not valid json\n", "utf8");

    // When
    const result = runCli(fixtureDbWithLabel("specify", "c1"));

    // Then — error, exit non-zero, file unchanged
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr).toContain("corrupt");
    expect(result.stderr).toContain("refusing to overwrite");
    expect(fs.readFileSync(outPath, "utf8")).toBe("not valid json\n");
  });

  it("Given a stored bundle and a fresh read that errors, When the CLI runs (default merge), Then it degrades to stored-only with a warning and does not abort", () => {
    // Given — first run writes specify
    runCli(fixtureDbWithLabel("specify", "c1"));
    // second run with a non-existent DB → fresh read fails → stored-only fallback
    const result = runCli(path.join(tmp, "nonexistent.sqlite"));

    // Then — does not abort (exit 0), warns about the fresh read, preserves stored
    expect(result.exitCode).toBe(0);
    expect(result.stderr).toContain("fresh read failed");
    const { header } = readBundle();
    expect(header.labels).toEqual(["specify"]);
  });

  it("Given an existing bundle from another source, When the CLI runs, Then it refuses to merge sources and does not overwrite", () => {
    // Given
    fs.mkdirSync(outDir, { recursive: true });
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    const foreignHeader = JSON.stringify({
      type: "bundle_header",
      capture_id: SPEC,
      labels: ["specify"],
      conversations_per_label: { specify: 1, implement: 0, review: 0 },
      conversation_ids: ["hermes-c1"],
      extracted_at: "2026-07-29T00:00:00.000Z",
      source: "hermes",
    });
    fs.writeFileSync(outPath, `${foreignHeader}\n`, "utf8");

    // When
    const result = runCli(fixtureDbWithLabel("specify", "warp-c1"));

    // Then
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr).toContain("bundle source mismatch");
    expect(result.stderr).toContain("--no-merge");
    expect(fs.readFileSync(outPath, "utf8")).toBe(`${foreignHeader}\n`);
  });

  it("Given an existing bundle from another source and --no-merge, When the CLI runs, Then it replaces the bundle with the requested source", () => {
    // Given
    fs.mkdirSync(outDir, { recursive: true });
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    const foreignHeader = JSON.stringify({
      type: "bundle_header",
      capture_id: SPEC,
      labels: ["review"],
      conversations_per_label: { specify: 0, implement: 0, review: 1 },
      conversation_ids: ["hermes-c1"],
      extracted_at: "2026-07-29T00:00:00.000Z",
      source: "hermes",
    });
    fs.writeFileSync(outPath, `${foreignHeader}\n`, "utf8");

    // When
    const result = runCli(fixtureDbWithLabel("specify", "warp-c1"), ["--no-merge"]);

    // Then
    expect(result.exitCode).toBe(0);
    expect(result.stderr).toContain("replacing existing bundle");
    const { header } = readBundle();
    expect(header.source).toBe("warp");
    expect(header.labels).toEqual(["specify"]);
    expect(header.conversation_ids).toEqual(["warp-c1"]);
    expect(fs.readFileSync(outPath, "utf8")).not.toBe(`${foreignHeader}\n`);
  });

  it("Given an existing bundle with a mismatched capture_id, When the CLI runs (default merge), Then it errors loudly and does not overwrite", () => {
    // Given — write a valid bundle with a different capture_id in the header
    fs.mkdirSync(outDir, { recursive: true });
    const outPath = path.join(outDir, `${SPEC}.jsonl`);
    const wrongHeader = JSON.stringify({
      type: "bundle_header",
      capture_id: "different-spec",
      labels: ["specify"],
      conversations_per_label: { specify: 1, implement: 0, "review": 0 },
      conversation_ids: ["c1"],
      extracted_at: "2026-07-14T00:00:00.000Z",
      source: "warp",
    });
    fs.writeFileSync(outPath, `${wrongHeader}\n`, "utf8");

    // When
    const result = runCli(fixtureDbWithLabel("specify", "c1"));

    // Then — error, exit non-zero, file unchanged
    expect(result.exitCode).not.toBe(0);
    expect(result.stderr).toContain("capture_id mismatch");
    expect(result.stderr).toContain("refusing to overwrite");
  });
});

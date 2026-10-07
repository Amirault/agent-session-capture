import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  ansiWrap,
  createFixture,
  seedMarker,
  seedQuery,
  seedUnbindableMarker,
} from "./fixtures/fixtureDb.js";
import { wrapReadableDb } from "../adapters/sqliteReadableDb.js";
import { findSeeds, parseMarker } from "../adapters/readers/markerReader.js";

describe("parseMarker", () => {
  it("parses a well-formed marker for each label", () => {
    expect(parseMarker(": CAPTURE_MARKER v=1 id=2026-06-30-x label=specify")).toEqual({
      capture_id: "2026-06-30-x",
      label: "specify",
    });
    expect(
      parseMarker(": CAPTURE_MARKER v=1 id=y label=review")
    ).toEqual({ capture_id: "y", label: "review" });
    expect(
      parseMarker(": CAPTURE_MARKER v=1 id=y label=step.2/retry")
    ).toEqual({ capture_id: "y", label: "step.2/retry" });
  });

  it("rejects non-marker text (diagnostic scripts that merely mention the marker)", () => {
    expect(parseMarker("python3 -c 'print(\"CAPTURE_MARKER\")'")).toBeNull();
    expect(parseMarker("grep CAPTURE_MARKER commands")).toBeNull();
  });

  it("defaults the label when the marker carries none", () => {
    expect(parseMarker(": CAPTURE_MARKER v=1 id=x")).toEqual({ capture_id: "x", label: "default" });
  });

  it("rejects a marker without the v=1 version token", () => {
    expect(parseMarker(": CAPTURE_MARKER id=x")).toBeNull();
    expect(parseMarker(": CAPTURE_MARKER v=2 id=x")).toBeNull();
  });

  it("rejects a marker without an id or with an empty id", () => {
    expect(parseMarker(": CAPTURE_MARKER v=1 label=review")).toBeNull();
    expect(parseMarker(": CAPTURE_MARKER v=1 id= label=review")).toBeNull();
  });

  it("parses a marker even when Warp records it chained with the real command on the next line", () => {
    expect(
      parseMarker(
        ": CAPTURE_MARKER v=1 id=2026-06-30-x label=specify\nscripts/validate-spec.sh specs/todo/2026-06-30-x.md"
      )
    ).toEqual({ capture_id: "2026-06-30-x", label: "specify" });
  });

  it.each([
    ["a `cd` prefix", "cd /repo/app; : CAPTURE_MARKER v=1 id=2026-06-30-x label=review", "review"],
    [
      "a `cd` prefix and a follow-up command on the next line",
      "cd /repo/app; : CAPTURE_MARKER v=1 id=2026-06-30-x label=review\ncat skills/review/SKILL.md",
      "review",
    ],
    [
      "a script chained with &&",
      "scripts/transition-spec.sh specs/todo/2026-06-30-x.md implementation-in-progress && : CAPTURE_MARKER v=1 id=2026-06-30-x label=implement",
      "implement",
    ],
  ])("Given a marker chained after %s on the first line, When parsed, Then the emission is recognized", (_, command, label) => {
    // When
    const parsed = parseMarker(command);

    // Then
    expect(parsed).toEqual({ capture_id: "2026-06-30-x", label });
  });

  it.each([
    ["a heredoc body line", "cat > notes.md <<'EOF'\n: CAPTURE_MARKER v=1 id=2026-06-30-x label=specify\nEOF"],
    ["a single-quoted echo argument", "echo ': CAPTURE_MARKER v=1 id=2026-06-30-x label=specify'"],
    ["a quoted grep pattern", "grep -c ': CAPTURE_MARKER v=1 id=2026-06-30-x' transcript.jsonl"],
    ["a `;` inside single quotes", "echo 'done; : CAPTURE_MARKER v=1 id=2026-06-30-x label=specify'"],
    ["a `&&` inside double quotes", 'echo "done && : CAPTURE_MARKER v=1 id=2026-06-30-x label=specify"'],
  ])("Given a marker-shaped text inside %s, When parsed, Then it is not an emission", (_, command) => {
    // When
    const parsed = parseMarker(command);

    // Then
    expect(parsed).toBeNull();
  });
});

describe("§9.2/§9.3 markerReader — selection & binding", () => {
  let tmp: string;
  let dbPath: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-93-"));
    dbPath = path.join(tmp, "f.db");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a marker command and a block at the same start_ts with conversation_id C1, When selecting, Then C1 is a bound seed with the marker label", () => {
    // Given
    const db = createFixture(dbPath);
    seedMarker(db, {
      capture_id: "X",
      label: "specify",
      conversation_id: "C1",
      start_ts: "2026-06-30 10:00:00.000000",
    });

    // When
    const seeds = findSeeds(wrapReadableDb(db), "X");

    // Then
    expect(seeds).toHaveLength(1);
    expect(seeds[0]).toMatchObject({
      conversation_id: "C1",
      label: "specify",
      status: "bound",
    });
    db.close();
  });

  it("Given two specs X and Y with distinct markers, When selecting X, Then only X's conversations are returned (no bleed from Y)", () => {
    // Given
    const db = createFixture(dbPath);
    seedMarker(db, {
      capture_id: "X",
      label: "specify",
      conversation_id: "CX",
      start_ts: "2026-06-30 10:00:00.000000",
    });
    seedMarker(db, {
      capture_id: "Y",
      label: "implement",
      conversation_id: "CY",
      start_ts: "2026-06-30 11:00:00.000000",
    });

    // When / Then
    expect(findSeeds(wrapReadableDb(db), "X").map((s) => s.conversation_id)).toEqual([
      "CX",
    ]);
    expect(findSeeds(wrapReadableDb(db), "Y").map((s) => s.conversation_id)).toEqual([
      "CY",
    ]);
    db.close();
  });

  it("Given a marker command with no matching block AND no ai_queries entry, When selecting, Then the conversation is reported as unbindable", () => {
    // Given
    const db = createFixture(dbPath);
    seedUnbindableMarker(db, {
      capture_id: "X",
      label: "specify",
      start_ts: "2026-06-30 10:00:00.000000",
    });
    // no ai_queries row seeded

    // When
    const seeds = findSeeds(wrapReadableDb(db), "X");

    // Then
    expect(seeds).toHaveLength(1);
    expect(seeds[0]!).toMatchObject({ status: "unbindable", conversation_id: null });
    db.close();
  });

  it("Given a marker command with no matching block but an ai_queries entry within 10 minutes before the marker, When selecting, Then the conversation is bound with heuristic confidence", () => {
    // Given — local orchestrated subagent: commands row, no blocks row, ai_queries entry 2 min before
    const db = createFixture(dbPath);
    seedUnbindableMarker(db, {
      capture_id: "X",
      label: "implement",
      start_ts: "2026-06-30 10:02:00.000000",
    });
    seedQuery(db, {
      conversation_id: "C-heuristic",
      start_ts: "2026-06-30 10:00:00.000000", // 2 min before marker
      text: "implement query",
    });

    // When
    const seeds = findSeeds(wrapReadableDb(db), "X");

    // Then
    expect(seeds).toHaveLength(1);
    expect(seeds[0]!).toMatchObject({
      status: "bound",
      conversation_id: "C-heuristic",
      label: "implement",
      confidence: "heuristic",
    });
    db.close();
  });

  it("Given a marker command with no matching block and an ai_queries entry MORE than 10 minutes before, When selecting, Then it remains unbindable (stale query ignored)", () => {
    // Given — ai_queries entry is 15 minutes before, outside the 10-min window
    const db = createFixture(dbPath);
    seedUnbindableMarker(db, {
      capture_id: "X",
      label: "implement",
      start_ts: "2026-06-30 10:15:00.000000",
    });
    seedQuery(db, {
      conversation_id: "C-stale",
      start_ts: "2026-06-30 10:00:00.000000", // 15 min before — outside 10-min window
      text: "stale query",
    });

    // When
    const seeds = findSeeds(wrapReadableDb(db), "X");

    // Then
    expect(seeds).toHaveLength(1);
    expect(seeds[0]!).toMatchObject({ status: "unbindable", conversation_id: null });
    db.close();
  });

  it("Given a normally-bound marker (blocks row present), When selecting, Then no confidence field is set (certain binding unchanged)", () => {
    // Given
    const db = createFixture(dbPath);
    seedMarker(db, {
      capture_id: "X",
      label: "specify",
      conversation_id: "C1",
      start_ts: "2026-06-30 10:00:00.000000",
    });

    // When
    const seeds = findSeeds(wrapReadableDb(db), "X");

    // Then
    expect(seeds).toHaveLength(1);
    expect(seeds[0]!.status).toBe("bound");
    expect(seeds[0]!.confidence).toBeUndefined(); // certain binding: no confidence field
    db.close();
  });

  it("Given the marker text appears only in blocks.stylized_command (ANSI), When selecting, Then it is NOT matched (grep is on commands.command only)", () => {
    // Given — a block whose stylized_command holds the marker text, but no commands row
    const db = createFixture(dbPath);
    db.prepare(
      `INSERT INTO blocks (pane_leaf_uuid, stylized_command, stylized_output, pwd, git_branch, exit_code, did_execute, start_ts, ai_metadata)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)`
    ).run(
      Buffer.from([1]),
      ansiWrap(": CAPTURE_MARKER v=1 id=X label=specify"),
      Buffer.from(""),
      "/repo",
      "main",
      0,
      1,
      "2026-06-30 10:00:00.000000",
      JSON.stringify({ conversation_id: "C-ansi" })
    );

    // When / Then
    expect(findSeeds(wrapReadableDb(db), "X")).toEqual([]);
    db.close();
  });

  it("Given a diagnostic command that merely mentions the marker, When selecting, Then it is NOT matched (anchored : CAPTURE_MARKER only)", () => {
    // Given — an exploration script whose command text contains "CAPTURE_MARKER"
    const db = createFixture(dbPath);
    db.prepare(
      `INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)`
    ).run(
      `python3 -c 'print("CAPTURE_MARKER v=1 id=X label=specify")'`,
      "2026-06-30 10:00:00.000000"
    );

    // When / Then
    expect(findSeeds(wrapReadableDb(db), "X")).toEqual([]);
    db.close();
  });

  it("Given no markers for the slug, When selecting, Then no seeds are returned", () => {
    const db = createFixture(dbPath);
    expect(findSeeds(wrapReadableDb(db), "missing")).toEqual([]);
    db.close();
  });
});

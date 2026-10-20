import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { LangfuseConversationReader } from "../adapters/langfuseConversationReader.js";
import type {
  ActivityQuery,
  LangfuseObservation,
  ObservationQuery,
} from "../adapters/langfuseObservationLister.js";
import type { SessionHeader } from "../domain/models.js";
import { extractSessions } from "../usecases/extractSessions.js";
import { generation, terminalCall, traceRoot } from "./fixtures/langfuseObservations.js";

type RecordedQuery = ["activity", ActivityQuery] | ["observations", ObservationQuery];

/** The fields `fields=core,basic,time` returns: no input, output nor metadata. */
function activityOf(observation: LangfuseObservation): LangfuseObservation {
  const { input: _input, output: _output, metadata: _metadata, ...activity } = observation;
  return activity;
}

/**
 * In-memory observation client filtering as the Langfuse v2 endpoint does
 * (`fromStartTime` on `startTime`, `sessionId` and `traceId` exact), recording
 * every query.
 */
function createReader(observations: LangfuseObservation[]) {
  const queries: RecordedQuery[] = [];
  const client = {
    async listActivity(query: ActivityQuery): Promise<LangfuseObservation[]> {
      queries.push(["activity", query]);
      return observations
        .filter((o) => query.fromStartTime === undefined || o.startTime >= query.fromStartTime)
        .filter((o) => query.sessionId === undefined || o.sessionId === query.sessionId)
        .filter((o) => query.traceId === undefined || o.traceId === query.traceId)
        .map(activityOf);
    },
    async listObservations(query: ObservationQuery): Promise<LangfuseObservation[]> {
      queries.push(["observations", query]);
      return observations.filter((o) => query.traceId === undefined || o.traceId === query.traceId);
    },
  };
  return { reader: new LangfuseConversationReader(client), queries };
}

/**
 * One trace as Langfuse v2 returns it: the root carries `sessionId` (`""` for a
 * session-less trace), every other observation `""`; `updatedAt` = `startTime`
 * unless given.
 */
function trace(options: {
  traceId: string;
  sessionId: string;
  startTime: string;
  prompt: string;
  answer: string;
  updatedAt?: string;
}): LangfuseObservation[] {
  const { traceId, sessionId, startTime, prompt, answer } = options;
  const later = new Date(Date.parse(startTime) + 1000).toISOString();
  return [
    traceRoot({ id: `${traceId}-root`, traceId, sessionId, prompt, startTime, updatedAt: startTime }),
    generation({
      id: `${traceId}-answer`,
      traceId,
      sessionId: "",
      parentObservationId: `${traceId}-root`,
      text: answer,
      startTime: later,
      updatedAt: options.updatedAt ?? later,
    }),
  ];
}

/** Example 4: `hermes-0412` last active 2026-09-28, `hermes-0419` (two traces) 2026-10-17. */
function exampleFourSessions(): LangfuseObservation[] {
  return [
    ...trace({ traceId: "tr-0412", sessionId: "hermes-0412", startTime: "2026-09-28T10:00:00.000Z", prompt: "old", answer: "old answer" }),
    ...trace({ traceId: "tr-0419a", sessionId: "hermes-0419", startTime: "2026-10-16T09:00:00.000Z", prompt: "first run", answer: "first answer" }),
    ...trace({
      traceId: "tr-0419b",
      sessionId: "hermes-0419",
      startTime: "2026-10-17T14:00:00.000Z",
      prompt: "second run",
      answer: "second answer",
      updatedAt: "2026-10-17T14:02:11.000Z",
    }),
  ];
}

function storedHeader(conversationId: string, lastModified: string): SessionHeader {
  return {
    type: "session_header",
    conversation_id: conversationId,
    source: "langfuse",
    last_modified: lastModified,
    extracted_at: "2026-10-18T00:00:00.000Z",
  };
}

function contentsByConversation(drafts: { conversation_id: string; content: string }[]) {
  return drafts.map((d) => [d.conversation_id, d.content]);
}

describe("LangfuseConversationReader.readSessions", () => {
  it("Given sessions last active before and after 2026-10-01 (Example 4), When readSessions with --since 2026-10-01, Then it enumerates only the recent session, its traces grouped under the root's sessionId", async () => {
    // Given
    const { reader, queries } = createReader(exampleFourSessions());

    // When
    const result = await reader.readSessions({ since: "2026-10-01", storedByCid: new Map() });

    // Then
    expect(result.source).toBe("langfuse");
    expect(result.sessions).toEqual([{ conversation_id: "hermes-0419", last_modified: "2026-10-17T14:02:11.000Z" }]);
    expect(contentsByConversation(result.reads.get("hermes-0419")!.drafts)).toEqual([
      ["hermes-0419", "first run"],
      ["hermes-0419", "first answer"],
      ["hermes-0419", "second run"],
      ["hermes-0419", "second answer"],
    ]);
    expect(queries[0]).toEqual(["activity", { fromStartTime: "2026-10-01T00:00:00.000Z" }]);
  });

  it("Given a session with one trace before and one after 2026-10-01, When readSessions with --since 2026-10-01, Then its file holds every trace of the session, not only the recent one", async () => {
    // Given
    const { reader, queries } = createReader([
      ...trace({ traceId: "tr-0419-old", sessionId: "hermes-0419", startTime: "2026-09-20T09:00:00.000Z", prompt: "first run", answer: "first answer" }),
      ...trace({ traceId: "tr-0419-new", sessionId: "hermes-0419", startTime: "2026-10-17T14:00:00.000Z", prompt: "second run", answer: "second answer" }),
    ]);

    // When
    const result = await reader.readSessions({ since: "2026-10-01", storedByCid: new Map() });

    // Then
    expect(result.sessions).toEqual([{ conversation_id: "hermes-0419", last_modified: "2026-10-17T14:00:01.000Z" }]);
    expect(contentsByConversation(result.reads.get("hermes-0419")!.drafts)).toEqual([
      ["hermes-0419", "first run"],
      ["hermes-0419", "first answer"],
      ["hermes-0419", "second run"],
      ["hermes-0419", "second answer"],
    ]);
    expect(queries).toEqual([
      ["activity", { fromStartTime: "2026-10-01T00:00:00.000Z" }],
      ["activity", { sessionId: "hermes-0419" }],
      ["observations", { traceId: "tr-0419-old" }],
      ["observations", { traceId: "tr-0419-new" }],
    ]);
  });

  it("Given a trace whose root started before 2026-10-01 and whose answer came after, in a session with a recent trace, When readSessions with --since 2026-10-01, Then the cut trace joins its session's file once and gets no file of its own", async () => {
    // Given
    const { reader, queries } = createReader([
      ...trace({ traceId: "tr-0419-cut", sessionId: "hermes-0419", startTime: "2026-09-30T23:59:59.500Z", prompt: "first run", answer: "first answer" }),
      ...trace({ traceId: "tr-0419-new", sessionId: "hermes-0419", startTime: "2026-10-17T14:00:00.000Z", prompt: "second run", answer: "second answer" }),
    ]);

    // When
    const result = await reader.readSessions({ since: "2026-10-01", storedByCid: new Map() });

    // Then
    expect(result.sessions).toEqual([{ conversation_id: "hermes-0419", last_modified: "2026-10-17T14:00:01.000Z" }]);
    expect(contentsByConversation(result.reads.get("hermes-0419")!.drafts)).toEqual([
      ["hermes-0419", "first run"],
      ["hermes-0419", "first answer"],
      ["hermes-0419", "second run"],
      ["hermes-0419", "second answer"],
    ]);
    expect(queries).toEqual([
      ["activity", { fromStartTime: "2026-10-01T00:00:00.000Z" }],
      ["activity", { traceId: "tr-0419-cut" }],
      ["activity", { sessionId: "hermes-0419" }],
      ["observations", { traceId: "tr-0419-cut" }],
      ["observations", { traceId: "tr-0419-new" }],
    ]);
  });

  it("Given two sessions, When readSessions with --conversation hermes-0412, Then it gathers exactly that one even with an unchanged stored stamp", async () => {
    // Given
    const { reader, queries } = createReader(exampleFourSessions());
    const storedByCid = new Map([["hermes-0412", storedHeader("hermes-0412", "2026-09-28T10:00:01.000Z")]]);

    // When
    const result = await reader.readSessions({ conversation: "hermes-0412", storedByCid });

    // Then
    expect(result.sessions).toEqual([{ conversation_id: "hermes-0412", last_modified: "2026-09-28T10:00:01.000Z" }]);
    expect([...result.reads.keys()]).toEqual(["hermes-0412"]);
    expect(contentsByConversation(result.reads.get("hermes-0412")!.drafts)).toEqual([
      ["hermes-0412", "old"],
      ["hermes-0412", "old answer"],
    ]);
    expect(queries).toEqual([
      ["activity", {}],
      ["observations", { traceId: "tr-0412" }],
    ]);
  });

  it("Given a stored file whose stamp equals the session's latest updatedAt (Example 9), When readSessions, Then the session is enumerated but its observations are never fetched, while a changed session is fetched trace by trace", async () => {
    // Given
    const { reader, queries } = createReader([
      ...exampleFourSessions(),
      ...trace({ traceId: "tr-0420", sessionId: "hermes-0420", startTime: "2026-10-18T08:00:00.000Z", prompt: "new", answer: "new answer" }),
    ]);
    const storedByCid = new Map([
      ["hermes-0419", storedHeader("hermes-0419", "2026-10-17T14:02:11.000Z")],
      ["hermes-0420", storedHeader("hermes-0420", "2026-10-18T07:00:00.000Z")],
    ]);

    // When
    const result = await reader.readSessions({ since: "2026-10-01", storedByCid });

    // Then
    expect(result.sessions.map((s) => s.conversation_id)).toEqual(["hermes-0419", "hermes-0420"]);
    expect([...result.reads.keys()]).toEqual(["hermes-0420"]);
    expect(queries).toEqual([
      ["activity", { fromStartTime: "2026-10-01T00:00:00.000Z" }],
      ["activity", { sessionId: "hermes-0420" }],
      ["observations", { traceId: "tr-0420" }],
    ]);
  });

  it("Given a trace without session, When readSessions, Then it is its own session named by its traceId", async () => {
    // Given
    const { reader } = createReader(
      trace({ traceId: "tr-solo", sessionId: "", startTime: "2026-10-18T08:00:00.000Z", prompt: "solo", answer: "solo answer" })
    );

    // When
    const result = await reader.readSessions({ storedByCid: new Map() });

    // Then
    expect(result.sessions).toEqual([{ conversation_id: "tr-solo", last_modified: "2026-10-18T08:00:01.000Z" }]);
    expect(contentsByConversation(result.reads.get("tr-solo")!.drafts)).toEqual([
      ["tr-solo", "solo"],
      ["tr-solo", "solo answer"],
    ]);
  });

  it("Given a trace without session wholly after 2026-10-01, When readSessions with --since 2026-10-01, Then its root is in the window and no lookup is made", async () => {
    // Given
    const { reader, queries } = createReader(
      trace({ traceId: "tr-solo", sessionId: "", startTime: "2026-10-18T08:00:00.000Z", prompt: "solo", answer: "solo answer" })
    );

    // When
    const result = await reader.readSessions({ since: "2026-10-01", storedByCid: new Map() });

    // Then
    expect(result.sessions.map((s) => s.conversation_id)).toEqual(["tr-solo"]);
    expect(queries).toEqual([
      ["activity", { fromStartTime: "2026-10-01T00:00:00.000Z" }],
      ["observations", { traceId: "tr-solo" }],
    ]);
  });

  it("Given a session whose tool call ran in a child observation (Example 8 mapping), When readSessions, Then its events map as in marker mode with the agent of the trace metadata", async () => {
    // Given
    const { reader } = createReader([
      traceRoot({ id: "obs-1", traceId: "tr-0419", sessionId: "hermes-0419", prompt: "implement", metadata: { agent: "hermes" } }),
      terminalCall({
        id: "obs-3",
        traceId: "tr-0419",
        sessionId: "",
        command: "cat spec.md",
        output: JSON.stringify("# Spec: …"),
        startTime: "2026-10-19T09:00:05.000Z",
      }),
    ]);

    // When
    const result = await reader.readSessions({ storedByCid: new Map() });

    // Then
    expect(
      result.reads.get("hermes-0419")!.drafts.map((d) => [d.kind, d.meta.tool_call_id, d.meta.session_source])
    ).toEqual([
      ["query", undefined, "hermes"],
      ["tool_call", "obs-3", "hermes"],
      ["tool_result", "obs-3", "hermes"],
    ]);
  });

  it("Given observations without updatedAt, When readSessions, Then the session has no stamp and is always gathered", async () => {
    // Given
    const root = traceRoot({ id: "obs-1", traceId: "tr-x", sessionId: "hermes-x", prompt: "no stamp" });
    const { reader } = createReader([root]);
    const storedByCid = new Map([["hermes-x", { ...storedHeader("hermes-x", "x"), last_modified: null }]]);

    // When
    const result = await reader.readSessions({ storedByCid });

    // Then
    expect(result.sessions).toEqual([{ conversation_id: "hermes-x", last_modified: null }]);
    expect([...result.reads.keys()]).toEqual(["hermes-x"]);
  });
});

describe("extractSessions over Langfuse", () => {
  let outDir: string;

  beforeEach(() => {
    outDir = fs.mkdtempSync(path.join(os.tmpdir(), "lf-sessions-"));
  });
  afterEach(() => {
    fs.rmSync(outDir, { recursive: true, force: true });
  });

  it("Given Example 4 already exported once, When extractSessions re-runs, Then the first run wrote one langfuse session file stamped with the latest updatedAt, and the re-run reports it unchanged without fetching observations (Example 9)", async () => {
    // Given
    const { reader, queries } = createReader(exampleFourSessions());
    const options = { outDir, source: "langfuse" as const, since: "2026-10-01" };
    const first = await extractSessions(reader, options);

    // When
    const second = await extractSessions(reader, options);

    // Then
    const file = path.join(outDir, "sessions", "hermes-0419.jsonl");
    expect(first.summary.written).toEqual([{ conversation_id: "hermes-0419", path: file, events: 4 }]);
    expect(JSON.parse(fs.readFileSync(file, "utf8").split("\n")[0]!)).toMatchObject({
      conversation_id: "hermes-0419",
      source: "langfuse",
      last_modified: "2026-10-17T14:02:11.000Z",
    });
    expect(second.summary).toMatchObject({ written: [], unchanged: ["hermes-0419"] });
    expect(queries.filter(([kind]) => kind === "observations")).toEqual([
      ["observations", { traceId: "tr-0419a" }],
      ["observations", { traceId: "tr-0419b" }],
    ]);
  });
});

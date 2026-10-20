import { describe, it, expect } from "vitest";
import { LangfuseConversationReader } from "../adapters/langfuseConversationReader.js";
import type { LangfuseObservation, ObservationQuery } from "../adapters/langfuseObservationLister.js";
import { extractBundle } from "../usecases/extractBundle.js";
import {
  anyObservation,
  generation,
  markerCommand,
  terminalCall,
  traceRoot,
} from "./fixtures/langfuseObservations.js";

const CAPTURE_ID = "checkout-bug";
const NOW = new Date("2026-10-19T12:00:00.000Z");

/** In-memory observation client: returns the fixture, records every query. */
function createReader(observations: LangfuseObservation[]) {
  const queries: ObservationQuery[] = [];
  const client = {
    async listObservations(query: ObservationQuery): Promise<LangfuseObservation[]> {
      queries.push(query);
      return observations;
    },
    async listActivity(): Promise<LangfuseObservation[]> {
      throw new Error("marker mode never lists activity");
    },
  };
  return { reader: new LangfuseConversationReader(client, { now: () => NOW }), queries };
}

/** Example 8: one Hermes session, a prompt, an answer and one terminal call. */
function exampleEightSession(): LangfuseObservation[] {
  return [
    traceRoot({
      id: "obs-1",
      traceId: "tr-0419",
      prompt: "implement increment 2",
      startTime: "2026-10-19T09:00:00.000Z",
      metadata: { agent: "hermes" },
    }),
    terminalCall({
      id: "obs-3",
      traceId: "tr-0419",
      command: "cat spec.md",
      output: JSON.stringify("# Spec: …"),
      startTime: "2026-10-19T09:00:05.000Z",
    }),
    generation({
      id: "obs-2",
      traceId: "tr-0419",
      text: "I will read the spec first",
      startTime: "2026-10-19T09:00:04.000Z",
    }),
    terminalCall({
      id: "obs-91",
      traceId: "tr-0419",
      command: markerCommand(CAPTURE_ID, "implement"),
      startTime: "2026-10-19T09:00:01.000Z",
    }),
  ];
}

/**
 * A trace as Langfuse v2 returns it for an OTLP export setting `session.id`
 * on the root span only: the root carries `sessionId`, every other
 * observation of the trace an empty string (`""` = no session).
 */
function sessionOnRootOnly(sessionId: string, traceId: string, label: string): LangfuseObservation[] {
  return [
    traceRoot({ id: `${traceId}-root`, traceId, sessionId, prompt: "run the workflow" }),
    terminalCall({
      id: `${traceId}-marker`,
      traceId,
      sessionId: "",
      parentObservationId: `${traceId}-root`,
      command: markerCommand(CAPTURE_ID, label),
      startTime: "2026-10-19T09:00:01.000Z",
    }),
    generation({
      id: `${traceId}-answer`,
      traceId,
      sessionId: "",
      parentObservationId: `${traceId}-root`,
      text: "done",
      startTime: "2026-10-19T09:00:02.000Z",
    }),
  ];
}

describe("LangfuseConversationReader marker mode (binding)", () => {
  it("Given an exact marker as the command of a TOOL named terminal (Example 5), When extracting the capture id, Then the session binds to the marker's label and all its observations become langfuse events", async () => {
    // Given
    const answers = Array.from({ length: 36 }, (_, i) =>
      generation({
        id: `obs-gen-${i}`,
        text: `answer ${i}`,
        startTime: `2026-10-19T09:01:${String(i).padStart(2, "0")}.000Z`,
      })
    );
    const { reader } = createReader([
      traceRoot({ id: "obs-root", prompt: "implement the spec" }),
      terminalCall({
        id: "obs-91",
        command: markerCommand(CAPTURE_ID, "implement"),
        startTime: "2026-10-19T09:00:30.000Z",
      }),
      ...answers,
      generation({ id: "obs-other", traceId: "tr-other", sessionId: "hermes-other", text: "unrelated session" }),
    ]);

    // When
    const { bundle, summary } = await extractBundle(reader, CAPTURE_ID);

    // Then
    expect(summary.events).toBe(38);
    expect(bundle!.header).toMatchObject({
      source: "langfuse",
      labels: ["implement"],
      conversation_ids: ["hermes-0419"],
    });
    expect(new Set(bundle!.events.map((e) => e.conversation_id))).toEqual(new Set(["hermes-0419"]));
  });

  it.each([
    ["Bash", "TOOL"],
    ["run_shell_command", "TOOL"],
    ["terminal", "SPAN"],
  ])(
    "Given an exact marker as the command of a %s %s observation, When reading the capture id, Then the session binds",
    async (name, type) => {
      // Given
      const { reader } = createReader([
        terminalCall({ name, type, command: markerCommand(CAPTURE_ID, "review") }),
      ]);

      // When
      const read = await reader.readCapture(CAPTURE_ID);

      // Then
      expect([...read.labelByCid]).toEqual([["hermes-0419", "review"]]);
    }
  );

  it("Given the marker only in a GENERATION input, a TOOL output, an echo command, a read_file input and a GENERATION named terminal (Example 6), When extracting the capture id, Then nothing binds", async () => {
    // Given
    const marker = markerCommand(CAPTURE_ID, "specify");
    const { reader } = createReader([
      { ...anyObservation, id: "gen", type: "GENERATION", input: JSON.stringify(`run ${marker}`) },
      terminalCall({ id: "out", command: "cat skill.md", output: JSON.stringify(marker) }),
      terminalCall({ id: "echo", command: `echo '${marker}'` }),
      terminalCall({ id: "read", name: "read_file", command: marker }),
      terminalCall({ id: "llm-tool", type: "GENERATION", command: marker }),
    ]);

    // When
    const { bundle, summary } = await extractBundle(reader, CAPTURE_ID);

    // Then
    expect(summary.conversations).toBe(0);
    expect(bundle!.events).toEqual([]);
  });

  it("Given an exact marker without label, When reading the capture id, Then the session binds to the default label", async () => {
    // Given
    const { reader } = createReader([terminalCall({ command: `: CAPTURE_MARKER v=1 id=${CAPTURE_ID}` })]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect([...read.labelByCid]).toEqual([["hermes-0419", "default"]]);
  });

  it("Given a marker of another capture id, When reading the capture id, Then nothing binds", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({ command: markerCommand("another-capture", "implement") }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect(read.labelByCid.size).toBe(0);
  });

  it("Given a bound marker observation without sessionId (Example 7), When reading the capture id, Then the conversation is its traceId", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({
        id: "obs-12",
        traceId: "tr-5f0c",
        sessionId: null,
        command: markerCommand(CAPTURE_ID, "specify"),
      }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect([...read.labelByCid]).toEqual([["tr-5f0c", "specify"]]);
    expect(read.drafts.map((d) => d.conversation_id)).toEqual(["tr-5f0c"]);
  });

  it("Given a trace whose sessionId is set only on its root and is an empty string on its other observations (real v2 shape), When reading the capture id, Then the marker binds the session and every observation of the trace joins it", async () => {
    // Given
    const { reader } = createReader(sessionOnRootOnly("capture-probe", "tr-a", "implement"));

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect([...read.labelByCid]).toEqual([["capture-probe", "implement"]]);
    expect(new Set(read.drafts.map((d) => d.conversation_id))).toEqual(new Set(["capture-probe"]));
  });

  it("Given a session trace and a session-less trace whose sessionIds are empty strings (real v2 shape), When reading the capture id, Then they stay two conversations: the session and the session-less trace's traceId", async () => {
    // Given
    const { reader } = createReader([
      ...sessionOnRootOnly("capture-probe", "tr-a", "implement"),
      ...sessionOnRootOnly("", "tr-b", "review"),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect([...read.labelByCid]).toEqual([
      ["capture-probe", "implement"],
      ["tr-b", "review"],
    ]);
    expect(read.skipped).toEqual([]);
  });

  it("Given the same label marker run in two sessions (Example 18), When extracting the capture id, Then both sessions are kept as two implement conversations, in marker order", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({
        id: "rerun",
        traceId: "tr-0422",
        sessionId: "hermes-0422",
        command: markerCommand(CAPTURE_ID, "implement"),
        startTime: "2026-10-19T15:00:00.000Z",
      }),
      terminalCall({
        id: "first",
        sessionId: "hermes-0419",
        command: markerCommand(CAPTURE_ID, "implement"),
        startTime: "2026-10-19T09:00:00.000Z",
      }),
    ]);

    // When
    const { bundle, summary } = await extractBundle(reader, CAPTURE_ID);

    // Then
    expect(bundle!.header.conversations_per_label.implement).toBe(2);
    expect(bundle!.header.conversation_ids).toEqual(["hermes-0419", "hermes-0422"]);
    expect(summary.collisions).toEqual([]);
  });

  it("Given one session holding markers of two labels of the capture id, When reading the capture id, Then the session is not bound and is reported as skipped", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({ id: "a", command: markerCommand(CAPTURE_ID, "implement") }),
      terminalCall({ id: "b", command: markerCommand(CAPTURE_ID, "review") }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect(read.labelByCid.size).toBe(0);
    expect(read.skipped).toEqual([
      {
        table: "observations",
        reason: "same session has markers for multiple labels",
        detail: "hermes-0419: implement, review",
      },
    ]);
  });

  it("Given a reader, When reading a capture id, Then it asks the client for the observations of the last 30 days", async () => {
    // Given
    const { reader, queries } = createReader([]);

    // When
    await reader.readCapture(CAPTURE_ID);

    // Then
    expect(queries).toEqual([{ fromStartTime: "2026-09-19T12:00:00.000Z" }]);
  });
});

describe("LangfuseConversationReader marker mode (mapping)", () => {
  it("Given one session (Example 8), When it is mapped, Then the events are query, agent_message, tool_call and tool_result ordered by ts", async () => {
    // Given
    const { reader } = createReader(exampleEightSession());

    // When
    const { bundle } = await extractBundle(reader, CAPTURE_ID);

    // Then
    expect(
      bundle!.events.map((e) => [e.seq, e.ts, e.role, e.kind, e.content])
    ).toEqual([
      [1, "2026-10-19T09:00:00.000Z", "user", "query", "implement increment 2"],
      [2, "2026-10-19T09:00:01.000Z", "assistant", "tool_call", markerCommand(CAPTURE_ID, "implement")],
      [3, "2026-10-19T09:00:04.000Z", "assistant", "agent_message", "I will read the spec first"],
      [4, "2026-10-19T09:00:05.000Z", "assistant", "tool_call", "cat spec.md"],
      [5, "2026-10-19T09:00:05.000Z", "tool", "tool_result", "# Spec: …"],
    ]);
  });

  it("Given a TOOL observation running from 09:00:05 to 09:00:36, When it is mapped, Then its call is stamped at its startTime and its result at its endTime", async () => {
    // Given
    const { reader } = createReader([
      traceRoot({ id: "obs-root", traceId: "tr-0419", prompt: "implement increment 2" }),
      terminalCall({
        id: "obs-test",
        traceId: "tr-0419",
        command: markerCommand(CAPTURE_ID, "implement"),
        output: JSON.stringify("Tests 12 passed"),
        startTime: "2026-10-19T09:00:05.000Z",
        endTime: "2026-10-19T09:00:36.000Z",
      }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    const tool = read.drafts.filter((d) => d.meta.observation_id === "obs-test");
    expect(tool.map((d) => [d.kind, d.ts])).toEqual([
      ["tool_call", "2026-10-19T09:00:05.000Z"],
      ["tool_result", "2026-10-19T09:00:36.000Z"],
    ]);
  });

  it("Given a GENERATION running from 09:01:00 to 09:02:09, When it is mapped, Then its answer is stamped at its endTime, when the answer exists", async () => {
    // Given
    const { reader } = createReader([
      ...exampleEightSession(),
      generation({
        id: "obs-report",
        traceId: "tr-0419",
        text: "# Workflow Report",
        startTime: "2026-10-19T09:01:00.000Z",
        endTime: "2026-10-19T09:02:09.000Z",
      }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    const answer = read.drafts.find((d) => d.meta.observation_id === "obs-report");
    expect(answer?.ts).toBe("2026-10-19T09:02:09.000Z");
  });

  it("Given a TOOL observation whose endTime is null (still running when read), When it is mapped, Then its result falls back to its startTime", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({
        id: "obs-open",
        command: markerCommand(CAPTURE_ID, "implement"),
        output: JSON.stringify("partial"),
        startTime: "2026-10-19T09:00:05.000Z",
        endTime: null,
      }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    const result = read.drafts.find((d) => d.kind === "tool_result");
    expect(result?.ts).toBe("2026-10-19T09:00:05.000Z");
  });

  it("Given a TOOL observation with input and output (Example 8), When it is mapped, Then its call and its result share the observation id as tool_call_id", async () => {
    // Given
    const { reader } = createReader(exampleEightSession());

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    const obs3 = read.drafts.filter((d) => d.meta.observation_id === "obs-3");
    expect(obs3.map((d) => [d.kind, d.meta.tool_call_id, d.meta.tool])).toEqual([
      ["tool_call", "obs-3", "terminal"],
      ["tool_result", "obs-3", "terminal"],
    ]);
  });

  it("Given trace metadata naming the agent (Example 8), When the session is mapped, Then every event records it as meta.session_source", async () => {
    // Given
    const { reader } = createReader(exampleEightSession());

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect(read.drafts.map((d) => d.meta.session_source)).toEqual([
      "hermes",
      "hermes",
      "hermes",
      "hermes",
      "hermes",
    ]);
  });

  it("Given a trace whose root names no agent, When the session is mapped, Then no event carries meta.session_source", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({ command: markerCommand(CAPTURE_ID, "implement"), metadata: { agent: "not-the-trace" } }),
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect(read.drafts.map((d) => "session_source" in d.meta)).toEqual([false]);
  });

  it("Given observations that carry no prompt, answer or tool payload, When the session is mapped, Then they produce no event", async () => {
    // Given
    const { reader } = createReader([
      terminalCall({ id: "marker", command: markerCommand(CAPTURE_ID, "implement") }),
      { ...anyObservation, id: "prompt", type: "GENERATION", input: JSON.stringify("system prompt") },
      { ...anyObservation, id: "event", type: "EVENT", input: JSON.stringify("log line") },
      { ...anyObservation, id: "empty", type: "TOOL", name: "read_file" },
    ]);

    // When
    const read = await reader.readCapture(CAPTURE_ID);

    // Then
    expect(read.drafts.map((d) => d.meta.observation_id)).toEqual(["marker"]);
  });
});

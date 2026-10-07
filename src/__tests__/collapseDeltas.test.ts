import { describe, it, expect } from "vitest";
import { walkProtobuf } from "../adapters/protobufWalk.js";
import { compactNodes } from "../adapters/compact.js";
import { applySchemaNames } from "../adapters/schemaOverlay.js";
import { collapseDeltas } from "../adapters/collapseDeltas.js";
import { encodeString, encodeMessage } from "./fixtures/protobuf.js";

/**
 * Builds fixtures that mirror the audit's streaming-delta pattern: Warp emits
 * one Task.messages (field 5) occurrence per field, not per logical call/turn.
 * These helpers each produce ONE such occurrence.
 */

/** One Message.tool_call (field 4) delta: tool_call_id + an optional extra leaf. */
function toolCallDelta(toolCallId: string, extra?: Buffer): Buffer {
  const parts = [encodeString(1, toolCallId)];
  if (extra) parts.push(extra);
  const toolCall = Buffer.concat(parts);
  return encodeMessage(5, encodeMessage(4, toolCall));
}

/** One Message.tool_call_result (field 5) delta: tool_call_id + an optional extra leaf. */
function toolCallResultDelta(toolCallId: string, extra?: Buffer): Buffer {
  const parts = [encodeString(1, toolCallId)];
  if (extra) parts.push(extra);
  const toolCallResult = Buffer.concat(parts);
  return encodeMessage(5, encodeMessage(5, toolCallResult));
}

/** RunShellCommand.command (ToolCall field 2 -> RunShellCommand field 1). */
function runShellCommandCommand(command: string): Buffer {
  return encodeMessage(2, encodeString(1, command));
}

/** RunShellCommandResult.output (ToolCallResult field 2 -> RunShellCommandResult field 1). */
function runShellCommandResultOutput(output: string): Buffer {
  return encodeMessage(2, encodeString(1, output));
}

/** RunShellCommandResult.command echo (ToolCallResult field 2 -> RunShellCommandResult field 3). */
function runShellCommandResultCommand(command: string): Buffer {
  return encodeMessage(2, encodeString(3, command));
}

/**
 * ToolCallResult.context(11) -> InputContext.updated_skills_context(12) ->
 * SkillsContext.available_skills(1) -> SkillDescriptor.path(1)|name(2).
 */
function skillLeaf(field: 1 | 2, value: string): Buffer {
  const skillDescriptor = encodeString(field, value);
  const availableSkills = encodeMessage(1, skillDescriptor);
  const skillsContext = encodeMessage(12, availableSkills);
  return encodeMessage(11, skillsContext);
}

/**
 * ToolCall.apply_file_diffs(6) -> ApplyFileDiffs.diffs(2, repeated) ->
 * FileDiff.file_path(1)|search(2)|replace(3).
 */
function applyFileDiffsLeaf(field: 1 | 2 | 3, value: string): Buffer {
  const fileDiff = encodeString(field, value);
  const diffs = encodeMessage(2, fileDiff);
  return encodeMessage(6, diffs);
}

/** One Message.agent_output (field 3) delta: text(field 1). */
function agentOutputDelta(text: string): Buffer {
  return encodeMessage(5, encodeMessage(3, encodeString(1, text)));
}

/** One Message.agent_reasoning (field 15) delta: reasoning(field 1). */
function agentReasoningDelta(text: string): Buffer {
  return encodeMessage(5, encodeMessage(15, encodeString(1, text)));
}

/** One Message.user_query (field 2) delta: query(field 1). */
function userQueryDelta(text: string): Buffer {
  return encodeMessage(5, encodeMessage(2, encodeString(1, text)));
}

/**
 * Message.update_todos(10) -> UpdateTodos.create_todo_list(1) ->
 * CreateTodoList.initial_todos(1) -> TodoItem.title(2).
 */
function updateTodosDelta(title: string): Buffer {
  const todoItem = encodeString(2, title); // TodoItem.title
  const initialTodos = encodeMessage(1, todoItem); // CreateTodoList.initial_todos
  const createTodoList = encodeMessage(1, initialTodos); // UpdateTodos.create_todo_list
  return encodeMessage(5, encodeMessage(10, createTodoList));
}

/**
 * Message.messages_received_from_agents(24) -> MessagesReceivedFromAgents.messages(1)
 * -> ReceivedMessage.subject(4).
 */
function messagesReceivedFromAgentsDelta(subject: string): Buffer {
  const receivedMessage = encodeString(4, subject);
  const messages = encodeMessage(1, receivedMessage);
  return encodeMessage(5, encodeMessage(24, messages));
}

/** Runs the full walk -> compact -> name pipeline (pre-collapse), as taskReader does. */
function named(blob: Buffer) {
  const { nodes } = walkProtobuf(blob);
  return applySchemaNames(compactNodes(nodes));
}

describe("§9.15 collapseDeltas — streaming field-delta collapse + dedupe", () => {
  it("Given a tool_call_result streamed as separate per-field deltas sharing one tool_call_id, When collapsed, Then exactly one event carries a fields map and the tool_call_id", () => {
    // Given — 3 streamed deltas for the same call: id, output, command
    const blob = Buffer.concat([
      toolCallResultDelta("tc-1", runShellCommandResultOutput("line 1 output")),
      toolCallResultDelta("tc-1", runShellCommandResultCommand("echo hi")),
      toolCallResultDelta("tc-1"),
    ]);

    // When
    const out = collapseDeltas(named(blob));

    // Then — one grouped event, not three
    const results = out.filter((n) => n.message_kind === "tool_call_result");
    expect(results).toHaveLength(1);
    expect(results[0]!.tool_call_id).toBe("tc-1");
    expect(results[0]!.fields).toEqual({
      "run_shell_command.output": "line 1 output",
      "run_shell_command.command": "echo hi",
    });
  });

  it("Given a single tool_call streamed as per-field deltas covering TWO files in one apply_file_diffs call, When collapsed, Then both files' relative field values survive (as an array), not just the last one", () => {
    // Given — one apply_file_diffs call touching two files, each file_path streamed as its own delta
    const blob = Buffer.concat([
      toolCallDelta("tc-multi", applyFileDiffsLeaf(1, "src/a.ts")),
      toolCallDelta("tc-multi", applyFileDiffsLeaf(1, "src/b.ts")),
    ]);

    // When
    const out = collapseDeltas(named(blob));

    // Then — one tool_call event, but BOTH file paths are present, not just the last delta's
    const toolCalls = out.filter((n) => n.message_kind === "tool_call");
    expect(toolCalls).toHaveLength(1);
    const filePaths = toolCalls[0]!.fields?.["apply_file_diffs.diffs.file_path"];
    expect(filePaths).toEqual(["src/a.ts", "src/b.ts"]);
  });

  it("Given tool_call_result deltas for two distinct tool_call_ids back-to-back, When collapsed, Then two separate events are produced (no cross-call merge)", () => {
    // Given — call A's delta, then call B's delta, contiguous in the walk
    const blob = Buffer.concat([
      toolCallResultDelta("tc-A", runShellCommandResultOutput("A output")),
      toolCallResultDelta("tc-B", runShellCommandResultOutput("B output")),
    ]);

    // When
    const out = collapseDeltas(named(blob));

    // Then
    const results = out.filter((n) => n.message_kind === "tool_call_result");
    expect(results).toHaveLength(2);
    expect(results.map((r) => r.tool_call_id)).toEqual(["tc-A", "tc-B"]);
  });

  it("Given an updated_skills_context fan-out streamed as one field per delta across N skills, When collapsed, Then one summary event lists all N skills (path+name)", () => {
    // Given — 3 skills, each streamed as a separate path delta then a separate name delta
    const blob = Buffer.concat([
      toolCallResultDelta("tc-2", skillLeaf(1, "skills/a/SKILL.md")),
      toolCallResultDelta("tc-2", skillLeaf(2, "a-skill")),
      toolCallResultDelta("tc-2", skillLeaf(1, "skills/b/SKILL.md")),
      toolCallResultDelta("tc-2", skillLeaf(2, "b-skill")),
      toolCallResultDelta("tc-2", skillLeaf(1, "skills/c/SKILL.md")),
      toolCallResultDelta("tc-2", skillLeaf(2, "c-skill")),
    ]);

    // When
    const out = collapseDeltas(named(blob));

    // Then — one event for the whole group, carrying all 3 reconstructed skills
    const results = out.filter((n) => n.message_kind === "tool_call_result");
    expect(results).toHaveLength(1);
    expect(results[0]!.skills).toEqual([
      { path: "skills/a/SKILL.md", name: "a-skill" },
      { path: "skills/b/SKILL.md", name: "b-skill" },
      { path: "skills/c/SKILL.md", name: "c-skill" },
    ]);
    // fields map has no skills leaves leaked into it
    expect(results[0]!.fields ?? {}).not.toHaveProperty("context.updated_skills_context.available_skills.path");
  });

  it("Given the same field_path streamed with growing values across consecutive same-message_kind deltas, When collapsed, Then one node survives with the final value and a merged_count", () => {
    // Given — agent_output.text streamed as 3 cumulative chunks
    const blob = Buffer.concat([
      agentOutputDelta("Hello"),
      agentOutputDelta("Hello, world"),
      agentOutputDelta("Hello, world!"),
    ]);

    // When
    const out = collapseDeltas(named(blob));

    // Then
    expect(out).toHaveLength(1);
    expect(out[0]!.message_kind).toBe("agent_output");
    expect(out[0]!.value).toBe("Hello, world!");
    expect(out[0]!.merged_count).toBe(3);
  });

  it("Given a Bash tool_call and its tool_call_result each streamed across multiple deltas containing a marker string, When collapsed, Then the marker text survives on exactly one tool_call event and one tool_call_result event (not scattered)", () => {
    // Given — marker text spread across tool_call (command) and tool_call_result (output + command echo)
    const marker = "CAPTURE_MARKER v=1 id=demo label=specify";
    const blob = Buffer.concat([
      toolCallDelta("tc-marker"),
      toolCallDelta("tc-marker", runShellCommandCommand(`echo '${marker}'`)),
      toolCallResultDelta("tc-marker"),
      toolCallResultDelta("tc-marker", runShellCommandResultOutput(marker)),
      toolCallResultDelta("tc-marker", runShellCommandResultCommand(`echo '${marker}'`)),
    ]);
    const before = named(blob);
    const beforeMarkerNodes = before.filter((n) => n.value.includes(marker));
    // Sanity: pre-collapse, the marker text is indeed spread across multiple leaf nodes
    expect(beforeMarkerNodes.length).toBeGreaterThan(1);

    // When
    const out = collapseDeltas(before);

    // Then — exactly one tool_call and one tool_call_result event, each carrying the marker once
    const toolCalls = out.filter((n) => n.message_kind === "tool_call");
    const toolCallResults = out.filter((n) => n.message_kind === "tool_call_result");
    expect(toolCalls).toHaveLength(1);
    expect(toolCallResults).toHaveLength(1);

    const containsMarker = (fields: Record<string, string | string[]> | undefined) =>
      Object.values(fields ?? {}).some((v) => (Array.isArray(v) ? v.some((s) => s.includes(marker)) : v.includes(marker)));
    expect(containsMarker(toolCalls[0]!.fields)).toBe(true);
    expect(containsMarker(toolCallResults[0]!.fields)).toBe(true);
  });

  it("Given agent_reasoning, user_query, update_todos, and messages_received_from_agents each occurring once, When collapsed, Then every one of them survives unchanged (no signal loss)", () => {
    // Given — one occurrence per kind, none of which should ever be grouped/merged
    const blob = Buffer.concat([
      userQueryDelta("implement the feature"),
      agentReasoningDelta("I should check the existing tests first"),
      updateTodosDelta("Write failing test"),
      messagesReceivedFromAgentsDelta("status update from subagent"),
    ]);

    // When
    const out = collapseDeltas(named(blob));

    // Then — 4 nodes in, 4 nodes out, values intact
    expect(out).toHaveLength(4);
    expect(out.map((n) => n.value)).toEqual([
      "implement the feature",
      "I should check the existing tests first",
      "Write failing test",
      "status update from subagent",
    ]);
    expect(out.map((n) => n.message_kind)).toEqual([
      "user_query",
      "agent_reasoning",
      "update_todos",
      "messages_received_from_agents",
    ]);
  });

  it("Given Task-level scalar fields with no message_kind, When collapsed, Then they pass through untouched (grouping never applies outside a Message oneof)", () => {
    // Given — Task.id(1) and Task.summary(6), both message_kind-less
    const blob = Buffer.concat([encodeString(1, "task-id"), encodeString(6, "a summary")]);

    // When
    const out = collapseDeltas(named(blob));

    // Then
    expect(out).toHaveLength(2);
    expect(out.map((n) => n.value)).toEqual(["task-id", "a summary"]);
    expect(out.every((n) => n.message_kind === undefined)).toBe(true);
  });
});

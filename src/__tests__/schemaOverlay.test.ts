import { describe, it, expect } from "vitest";
import { walkProtobuf } from "../adapters/protobufWalk.js";
import { applySchemaNames } from "../adapters/schemaOverlay.js";
import { encodeString, encodeMessage } from "./fixtures/protobuf.js";

/**
 * Builds a Task blob: messages(field 5) -> ToolCall(field 4) -> RunShellCommand(field 2) -> command(field 1).
 * Walker yields nodes "5.4.1" (tool_call_id) and "5.4.2.1" (command).
 */
function taskWithRunShellCommand(): Buffer {
  const runShellCommand = encodeString(1, "ls -la");
  const toolCall = Buffer.concat([
    encodeString(1, "tc-1"),
    encodeMessage(2, runShellCommand),
  ]);
  const message = encodeMessage(4, toolCall);
  return encodeMessage(5, message);
}

describe("§schemaOverlay applySchemaNames", () => {
  it("Given a Task with a tool_call/run_shell_command, When overlaid, Then paths are named and carry message_kind and tool", () => {
    // Given
    const { nodes } = walkProtobuf(taskWithRunShellCommand());

    // When
    const named = applySchemaNames(nodes);

    // Then
    const command = named.find((n) => n.value === "ls -la");
    expect(command).toBeDefined();
    expect(command!.field_path).toBe("messages.tool_call.run_shell_command.command");
    expect(command!.message_kind).toBe("tool_call");
    expect(command!.tool).toBe("run_shell_command");
    expect(command!.schema_mismatch).toBeUndefined();

    const toolCallId = named.find((n) => n.value === "tc-1");
    expect(toolCallId).toBeDefined();
    expect(toolCallId!.field_path).toBe("messages.tool_call.tool_call_id");
    expect(toolCallId!.message_kind).toBe("tool_call");
    // tool_call_id is NOT a tool variant -> tool must be absent
    expect(toolCallId!.tool).toBeUndefined();
  });

  it("Given a user_query message, When overlaid, Then message_kind is user_query and tool is absent (exclusion)", () => {
    // Given — messages(field 5) -> user_query(field 2) -> query(field 1)
    const blob = encodeMessage(5, encodeMessage(2, encodeString(1, "implement it")));
    const { nodes } = walkProtobuf(blob);

    // When
    const named = applySchemaNames(nodes);

    // Then
    expect(named).toHaveLength(1);
    expect(named[0]!.field_path).toBe("messages.user_query.query");
    expect(named[0]!.message_kind).toBe("user_query");
    expect(named[0]!.tool).toBeUndefined();
    expect(named[0]!.schema_mismatch).toBeUndefined();
  });

  it("Given an agent_output message, When overlaid, Then message_kind is agent_output (exclusion: not tool_call)", () => {
    // Given — messages(5) -> agent_output(3) -> text(1)
    const blob = encodeMessage(5, encodeMessage(3, encodeString(1, "done")));
    const { nodes } = walkProtobuf(blob);

    // When
    const named = applySchemaNames(nodes);

    // Then
    expect(named[0]!.field_path).toBe("messages.agent_output.text");
    expect(named[0]!.message_kind).toBe("agent_output");
    expect(named[0]!.tool).toBeUndefined();
  });

  it("Given a field number absent from the schema, When overlaid, Then the numbered path is kept and schema_mismatch is set", () => {
    // Given — messages(5) -> field 99 (not in Message schema)
    const blob = encodeMessage(5, encodeString(99, "unknown"));
    const { nodes } = walkProtobuf(blob);

    // When
    const named = applySchemaNames(nodes);

    // Then — never silently relabelled
    expect(named[0]!.field_path).toBe("5.99");
    expect(named[0]!.schema_mismatch).toBe(true);
    expect(named[0]!.message_kind).toBeUndefined();
    expect(named[0]!.tool).toBeUndefined();
  });

  it("Given a leaf field the walker recursed into (conflict), When overlaid, Then numbered path kept and schema_mismatch set", () => {
    // Given — Task.id is field 1 (a leaf string). Encode field 1 with bytes that
    // parse as a nested message, so the walker recurses to "1.5".
    const blob = encodeMessage(1, encodeString(5, "looks nested"));
    const { nodes } = walkProtobuf(blob);

    // When
    const named = applySchemaNames(nodes);

    // Then — schema says Task.1 is a leaf; the walker recursed past it -> mismatch
    expect(named[0]!.field_path).toBe("1.5");
    expect(named[0]!.schema_mismatch).toBe(true);
  });

  it("Given a top-level Task scalar field, When overlaid, Then it is named with no message_kind", () => {
    // Given — Task.summary is field 6 (leaf)
    const blob = encodeString(6, "a summary");
    const { nodes } = walkProtobuf(blob);

    // When
    const named = applySchemaNames(nodes);

    // Then
    expect(named[0]!.field_path).toBe("summary");
    expect(named[0]!.message_kind).toBeUndefined();
    expect(named[0]!.schema_mismatch).toBeUndefined();
  });
});

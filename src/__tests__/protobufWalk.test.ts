import { describe, it, expect } from "vitest";
import { walkProtobuf } from "../adapters/protobufWalk.js";
import { encodeString, encodeMessage, encodeVarint, encodeTag } from "./fixtures/protobuf.js";

describe("§9.6 protobufWalk", () => {
  it("Given a task BLOB with title, nested tool call, and output, When walked, Then readable strings are emitted in document order with field-path metadata", () => {
    // Given — field 1 = title, field 2 = nested {field 1 = tool call}, field 3 = output
    const blob = Buffer.concat([
      encodeString(1, "Task title"),
      encodeMessage(2, encodeString(1, "tool call")),
      encodeString(3, "command output"),
    ]);

    // When
    const { nodes, complete } = walkProtobuf(blob);

    // Then
    expect(complete).toBe(true);
    expect(nodes.map((n) => n.value)).toEqual([
      "Task title",
      "tool call",
      "command output",
    ]);
    expect(nodes.map((n) => n.field_path)).toEqual(["1", "2.1", "3"]);
  });

  it("Given a leaf that is base64-wrapped protobuf, When walked, Then it is decoded and recursed and the inner text is recovered", () => {
    // Given — an inner message {field 1 = "inner secret"}, base64-encoded, stored as field 5
    const inner = encodeString(1, "inner secret");
    const b64 = Buffer.from(inner).toString("base64");
    const blob = encodeString(5, b64);

    // When
    const { nodes } = walkProtobuf(blob);

    // Then — the inner text is recovered with a path that descends into the decoded leaf
    const recovered = nodes.find((n) => n.value === "inner secret");
    expect(recovered).toBeDefined();
    expect(recovered!.field_path).toBe("5.1");
    // the raw base64 blob is not emitted as a string in its place
    expect(nodes.map((n) => n.value)).not.toContain(b64);
  });

  it("Given a malformed/truncated task BLOB, When walked, Then partial results are returned without throwing, marked via complete:false", () => {
    // Given — a good string field, then a length-delimited field whose declared length exceeds the buffer
    const blob = Buffer.concat([
      encodeString(1, "recovered text"),
      Buffer.concat([encodeTag(2, 2), encodeVarint(100), Buffer.from("abc")]),
    ]);

    // When
    const { nodes, complete } = walkProtobuf(blob);

    // Then — recovered without throwing, partial node present, walk flagged incomplete
    expect(complete).toBe(false);
    expect(nodes.map((n) => n.value)).toContain("recovered text");
  });

  it("Given raw mode, When walking strings that look like secrets, Then UUIDs/tokens are retained (no filtering)", () => {
    // Given — strings that look like identifiers/secrets
    const uuid = "550e8400-e29b-41d4-a716-446655440000";
    const token = "sk-ant-api03-xxxxxxxxxxxxxxxx";
    const blob = Buffer.concat([
      encodeString(1, uuid),
      encodeString(2, token),
    ]);

    // When
    const { nodes } = walkProtobuf(blob);

    // Then — both retained verbatim
    expect(nodes.map((n) => n.value)).toEqual([uuid, token]);
  });
});

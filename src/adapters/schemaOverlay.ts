/**
 * Schema-aware name overlay for walked protobuf nodes.
 *
 * The walker recovers field paths as dotted numbers (e.g. "5.4.2.1") because it
 * does not know the schema. This pure pass renames those numbers to the
 * semantic field names captured in `protoSchema.ts` (generated from the public
 * `warp-proto-apis`), and extracts the two oneof variants that drive
 * decision-tracing:
 *   - `message_kind` — the `Message` oneof variant (user_query, tool_call, …)
 *   - `tool`         — the `ToolCall` oneof variant (run_shell_command, grep, …)
 *
 * Oneof variants are distinguished by their field NUMBER, which the walker
 * already records — so this needs no access to varint values.
 *
 * Safety: validation + honest fallback. At every path segment the walker's
 * interpretation must agree with the schema (message fields are descended into,
 * leaf fields terminate the path). On any disagreement — an absent field number,
 * a leaf the walker recursed past, or a message field the walker emitted as a
 * string — the ORIGINAL numbered path is kept and `schema_mismatch` is set.
 * The overlay never silently relabels. No regex is used.
 */
import type { WalkNode } from "./protobufWalk.js";
import { PROTO_SCHEMA, ROOT_TYPE, type FieldInfo } from "./protoSchema.js";

export interface NamedNode extends WalkNode {
  /** Named field path, or the original numbered path on mismatch. */
  field_path: string;
  /** The `Message` oneof variant this node belongs to, if identifiable. */
  message_kind?: string;
  /** The `ToolCall` oneof variant (tool name), if identifiable. */
  tool?: string;
  /** True when the path did not align with the schema (numbered path retained). */
  schema_mismatch?: boolean;
}

const MESSAGE_TYPE = "warp.multi_agent.v1.Message";
const TOOL_CALL_TYPE = "warp.multi_agent.v1.Message.ToolCall";

export function applySchemaNames(nodes: readonly WalkNode[]): NamedNode[] {
  return nodes.map((node) => nameNode(node));
}

function nameNode(node: WalkNode): NamedNode {
  const segments = node.field_path.split(".").map((s) => Number(s));
  if (segments.length === 0 || segments.some((n) => Number.isNaN(n))) {
    return mismatch(node);
  }

  const named: string[] = [];
  let messageKind: string | undefined;
  let tool: string | undefined;
  let currentType = ROOT_TYPE;

  for (let i = 0; i < segments.length; i++) {
    const isLast = i === segments.length - 1;
    const fields = PROTO_SCHEMA[currentType];
    const info: FieldInfo | undefined = fields?.[segments[i]!];
    if (info === undefined) {
      return mismatch(node);
    }
    named.push(info.name);

    if (info.oneof === "message" && currentType === MESSAGE_TYPE) {
      messageKind = info.name;
    } else if (info.oneof === "tool" && currentType === TOOL_CALL_TYPE) {
      tool = info.name;
    }

    if (isLast) {
      // The value lives here: the schema must call this a leaf. A message field
      // at the leaf means the walker emitted a string where the schema expects a
      // nested message (heuristic miss) — do not trust the name.
      if (info.kind === "message") {
        return mismatch(node);
      }
    } else {
      // Interior segments must be messages we can descend into. A leaf here means
      // the walker recursed past a scalar that happened to parse as a message.
      if (info.kind !== "message" || info.child === undefined) {
        return mismatch(node);
      }
      currentType = info.child;
    }
  }

  return {
    ...node,
    field_path: named.join("."),
    message_kind: messageKind,
    tool,
  };
}

/** Keep the original numbered path and flag the mismatch; never relabel. */
function mismatch(node: WalkNode): NamedNode {
  return { ...node, schema_mismatch: true };
}

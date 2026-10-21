//! Schema-aware name overlay for walked protobuf nodes.
//!
//! The walker recovers field paths as dotted numbers (e.g. "5.4.2.1"). This
//! pure pass renames those numbers to the semantic field names of the Warp
//! schema, and extracts the two oneof variants that drive decision tracing:
//!   - `message_kind` — the `Message` oneof variant (user_query, tool_call, …)
//!   - `tool`         — the `ToolCall` oneof variant (run_shell_command, grep, …)
//!
//! Safety: at every path segment the walker's interpretation must agree with
//! the schema (message fields are descended into, leaf fields terminate the
//! path). On any disagreement the ORIGINAL numbered path is kept and
//! `schema_mismatch` is set. The overlay never silently relabels.

use super::proto_schema::{Kind, ROOT_TYPE, Schema, active_schema};
use super::protobuf_walk::WalkNode;

/// A walked node after the overlay.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct NamedNode {
    pub node: WalkNode,
    /// The `Message` oneof variant this node belongs to, if identifiable.
    pub message_kind: Option<String>,
    /// The `ToolCall` oneof variant (tool name), if identifiable.
    pub tool: Option<String>,
    /// True when the path did not align with the schema (numbered path retained).
    pub schema_mismatch: bool,
}

const MESSAGE_TYPE: &str = "warp.multi_agent.v1.Message";
const TOOL_CALL_TYPE: &str = "warp.multi_agent.v1.Message.ToolCall";

pub fn apply_schema_names(nodes: Vec<WalkNode>) -> Vec<NamedNode> {
    apply_schema_names_with(nodes, active_schema())
}

pub fn apply_schema_names_with(nodes: Vec<WalkNode>, schema: &Schema) -> Vec<NamedNode> {
    nodes.into_iter().map(|node| name_node(node, schema)).collect()
}

fn mismatch(node: WalkNode) -> NamedNode {
    NamedNode {
        node,
        schema_mismatch: true,
        ..NamedNode::default()
    }
}

fn name_node(node: WalkNode, schema: &Schema) -> NamedNode {
    let segments: Option<Vec<u32>> = node.field_path.split('.').map(|s| s.parse().ok()).collect();
    let Some(segments) = segments else {
        return mismatch(node);
    };

    let mut named: Vec<&str> = Vec::with_capacity(segments.len());
    let mut message_kind = None;
    let mut tool = None;
    let mut current_type = ROOT_TYPE;
    for (i, &number) in segments.iter().enumerate() {
        let is_last = i == segments.len() - 1;
        let Some(info) = schema.field(current_type, number) else {
            return mismatch(node);
        };
        named.push(info.name);
        if info.oneof == Some("message") && current_type == MESSAGE_TYPE {
            message_kind = Some(info.name.to_string());
        } else if info.oneof == Some("tool") && current_type == TOOL_CALL_TYPE {
            tool = Some(info.name.to_string());
        }
        if is_last {
            // The value lives here: a message field at the leaf means the walker
            // emitted a string where the schema expects a nested message.
            if info.kind == Kind::Message {
                return mismatch(node);
            }
        } else {
            // Interior segments must be messages we can descend into.
            match (info.kind, info.child) {
                (Kind::Message, Some(child)) => current_type = child,
                _ => return mismatch(node),
            }
        }
    }
    let field_path = named.join(".");
    NamedNode {
        node: WalkNode { field_path, ..node },
        message_kind,
        tool,
        schema_mismatch: false,
    }
}

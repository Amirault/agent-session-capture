//! §schemaOverlay applySchemaNames.

use super::fixtures::protobuf::{encode_message, encode_string};
use crate::adapters::protobuf_walk::walk_protobuf;
use crate::adapters::schema_overlay::apply_schema_names;

/// Builds a Task blob: messages(field 5) -> ToolCall(field 4) -> RunShellCommand(field 2) -> command(field 1).
/// Walker yields nodes "5.4.1" (tool_call_id) and "5.4.2.1" (command).
fn task_with_run_shell_command() -> Vec<u8> {
    let run_shell_command = encode_string(1, "ls -la");
    let tool_call = [encode_string(1, "tc-1"), encode_message(2, &run_shell_command)].concat();
    let message = encode_message(4, &tool_call);
    encode_message(5, &message)
}

#[test]
fn tool_call_run_shell_command_paths_are_named_with_message_kind_and_tool() {
    // Given
    let blob = task_with_run_shell_command();
    let nodes = walk_protobuf(&blob).expect("walk").nodes;

    // When
    let named = apply_schema_names(nodes);

    // Then
    let command = named.iter().find(|n| n.node.value == "ls -la").expect("command");
    assert_eq!(command.node.field_path, "messages.tool_call.run_shell_command.command");
    assert_eq!(command.message_kind.as_deref(), Some("tool_call"));
    assert_eq!(command.tool.as_deref(), Some("run_shell_command"));
    assert!(!command.schema_mismatch);

    let tool_call_id = named.iter().find(|n| n.node.value == "tc-1").expect("tool_call_id");
    assert_eq!(tool_call_id.node.field_path, "messages.tool_call.tool_call_id");
    assert_eq!(tool_call_id.message_kind.as_deref(), Some("tool_call"));
    // tool_call_id is NOT a tool variant -> tool must be absent
    assert_eq!(tool_call_id.tool, None);
}

#[test]
fn user_query_message_has_user_query_kind_and_no_tool() {
    // Given — messages(field 5) -> user_query(field 2) -> query(field 1)
    let blob = encode_message(5, &encode_message(2, &encode_string(1, "implement it")));
    let nodes = walk_protobuf(&blob).expect("walk").nodes;

    // When
    let named = apply_schema_names(nodes);

    // Then
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].node.field_path, "messages.user_query.query");
    assert_eq!(named[0].message_kind.as_deref(), Some("user_query"));
    assert_eq!(named[0].tool, None);
    assert!(!named[0].schema_mismatch);
}

#[test]
fn agent_output_message_has_agent_output_kind() {
    // Given — messages(5) -> agent_output(3) -> text(1)
    let blob = encode_message(5, &encode_message(3, &encode_string(1, "done")));
    let nodes = walk_protobuf(&blob).expect("walk").nodes;

    // When
    let named = apply_schema_names(nodes);

    // Then
    assert_eq!(named[0].node.field_path, "messages.agent_output.text");
    assert_eq!(named[0].message_kind.as_deref(), Some("agent_output"));
    assert_eq!(named[0].tool, None);
}

#[test]
fn field_number_absent_from_schema_keeps_numbered_path_and_flags_mismatch() {
    // Given — messages(5) -> field 99 (not in Message schema)
    let blob = encode_message(5, &encode_string(99, "unknown"));
    let nodes = walk_protobuf(&blob).expect("walk").nodes;

    // When
    let named = apply_schema_names(nodes);

    // Then — never silently relabelled
    assert_eq!(named[0].node.field_path, "5.99");
    assert!(named[0].schema_mismatch);
    assert_eq!(named[0].message_kind, None);
    assert_eq!(named[0].tool, None);
}

#[test]
fn leaf_field_the_walker_recursed_into_keeps_numbered_path_and_flags_mismatch() {
    // Given — Task.id is field 1 (a leaf string). Encode field 1 with bytes that
    // parse as a nested message, so the walker recurses to "1.5".
    let blob = encode_message(1, &encode_string(5, "looks nested"));
    let nodes = walk_protobuf(&blob).expect("walk").nodes;

    // When
    let named = apply_schema_names(nodes);

    // Then — schema says Task.1 is a leaf; the walker recursed past it -> mismatch
    assert_eq!(named[0].node.field_path, "1.5");
    assert!(named[0].schema_mismatch);
}

#[test]
fn top_level_task_scalar_is_named_without_message_kind() {
    // Given — Task.summary is field 6 (leaf)
    let blob = encode_string(6, "a summary");
    let nodes = walk_protobuf(&blob).expect("walk").nodes;

    // When
    let named = apply_schema_names(nodes);

    // Then
    assert_eq!(named[0].node.field_path, "summary");
    assert_eq!(named[0].message_kind, None);
    assert!(!named[0].schema_mismatch);
}

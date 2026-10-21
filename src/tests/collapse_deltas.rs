//! §9.15 collapseDeltas — streaming field-delta collapse + dedupe.
//!
//! Builds fixtures that mirror the audit's streaming-delta pattern: Warp emits
//! one Task.messages (field 5) occurrence per field, not per logical call/turn.
//! These helpers each produce ONE such occurrence.

use super::fixtures::protobuf::{encode_message, encode_string};
use crate::adapters::collapse_deltas::{CollapsedNode, collapse_deltas};
use crate::adapters::compact::compact_nodes;
use crate::adapters::protobuf_walk::walk_protobuf;
use crate::adapters::schema_overlay::{NamedNode, apply_schema_names};
use crate::js::{Object, Value};

/// One Message.tool_call (field 4) delta: tool_call_id + an optional extra leaf.
fn tool_call_delta(tool_call_id: &str, extra: Option<Vec<u8>>) -> Vec<u8> {
    let mut tool_call = encode_string(1, tool_call_id);
    if let Some(extra) = extra {
        tool_call.extend(extra);
    }
    encode_message(5, &encode_message(4, &tool_call))
}

/// One Message.tool_call_result (field 5) delta: tool_call_id + an optional extra leaf.
fn tool_call_result_delta(tool_call_id: &str, extra: Option<Vec<u8>>) -> Vec<u8> {
    let mut tool_call_result = encode_string(1, tool_call_id);
    if let Some(extra) = extra {
        tool_call_result.extend(extra);
    }
    encode_message(5, &encode_message(5, &tool_call_result))
}

/// RunShellCommand.command (ToolCall field 2 -> RunShellCommand field 1).
fn run_shell_command_command(command: &str) -> Vec<u8> {
    encode_message(2, &encode_string(1, command))
}

/// RunShellCommandResult.output (ToolCallResult field 2 -> RunShellCommandResult field 1).
fn run_shell_command_result_output(output: &str) -> Vec<u8> {
    encode_message(2, &encode_string(1, output))
}

/// RunShellCommandResult.command echo (ToolCallResult field 2 -> RunShellCommandResult field 3).
fn run_shell_command_result_command(command: &str) -> Vec<u8> {
    encode_message(2, &encode_string(3, command))
}

/// ToolCallResult.context(11) -> InputContext.updated_skills_context(12) ->
/// SkillsContext.available_skills(1) -> SkillDescriptor.path(1)|name(2).
fn skill_leaf(field: u32, value: &str) -> Vec<u8> {
    let skill_descriptor = encode_string(field, value);
    let available_skills = encode_message(1, &skill_descriptor);
    let skills_context = encode_message(12, &available_skills);
    encode_message(11, &skills_context)
}

/// ToolCall.apply_file_diffs(6) -> ApplyFileDiffs.diffs(2, repeated) ->
/// FileDiff.file_path(1)|search(2)|replace(3).
fn apply_file_diffs_leaf(field: u32, value: &str) -> Vec<u8> {
    let file_diff = encode_string(field, value);
    let diffs = encode_message(2, &file_diff);
    encode_message(6, &diffs)
}

/// One Message.agent_output (field 3) delta: text(field 1).
fn agent_output_delta(text: &str) -> Vec<u8> {
    encode_message(5, &encode_message(3, &encode_string(1, text)))
}

/// One Message.agent_reasoning (field 15) delta: reasoning(field 1).
fn agent_reasoning_delta(text: &str) -> Vec<u8> {
    encode_message(5, &encode_message(15, &encode_string(1, text)))
}

/// One Message.user_query (field 2) delta: query(field 1).
fn user_query_delta(text: &str) -> Vec<u8> {
    encode_message(5, &encode_message(2, &encode_string(1, text)))
}

/// Message.update_todos(10) -> UpdateTodos.create_todo_list(1) ->
/// CreateTodoList.initial_todos(1) -> TodoItem.title(2).
fn update_todos_delta(title: &str) -> Vec<u8> {
    let todo_item = encode_string(2, title);
    let initial_todos = encode_message(1, &todo_item);
    let create_todo_list = encode_message(1, &initial_todos);
    encode_message(5, &encode_message(10, &create_todo_list))
}

/// Message.messages_received_from_agents(24) -> MessagesReceivedFromAgents.messages(1)
/// -> ReceivedMessage.subject(4).
fn messages_received_from_agents_delta(subject: &str) -> Vec<u8> {
    let received_message = encode_string(4, subject);
    let messages = encode_message(1, &received_message);
    encode_message(5, &encode_message(24, &messages))
}

/// Runs the full walk -> compact -> name pipeline (pre-collapse), as the task reader does.
fn named(blob: &[u8]) -> Vec<NamedNode> {
    let walked = walk_protobuf(blob).expect("walk");
    apply_schema_names(compact_nodes(walked.nodes))
}

fn of_kind<'a>(nodes: &'a [CollapsedNode], kind: &str) -> Vec<&'a CollapsedNode> {
    nodes
        .iter()
        .filter(|n| n.named.message_kind.as_deref() == Some(kind))
        .collect()
}

fn skill(path: &str, name: &str) -> Object {
    let mut object = Object::new();
    object.insert("path", path);
    object.insert("name", name);
    object
}

#[test]
fn tool_call_result_per_field_deltas_sharing_one_id_collapse_into_one_event_with_fields_and_id() {
    // Given — 3 streamed deltas for the same call: id, output, command
    let blob = [
        tool_call_result_delta("tc-1", Some(run_shell_command_result_output("line 1 output"))),
        tool_call_result_delta("tc-1", Some(run_shell_command_result_command("echo hi"))),
        tool_call_result_delta("tc-1", None),
    ]
    .concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then — one grouped event, not three
    let results = of_kind(&out, "tool_call_result");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].tool_call_id.as_deref(), Some("tc-1"));
    let mut expected = Object::new();
    expected.insert("run_shell_command.output", "line 1 output");
    expected.insert("run_shell_command.command", "echo hi");
    assert_eq!(results[0].fields, Some(expected));
}

#[test]
fn tool_call_deltas_covering_two_files_keep_both_file_paths_as_an_array() {
    // Given — one apply_file_diffs call touching two files, each file_path streamed as its own delta
    let blob = [
        tool_call_delta("tc-multi", Some(apply_file_diffs_leaf(1, "src/a.ts"))),
        tool_call_delta("tc-multi", Some(apply_file_diffs_leaf(1, "src/b.ts"))),
    ]
    .concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then — one tool_call event, but BOTH file paths are present, not just the last delta's
    let tool_calls = of_kind(&out, "tool_call");
    assert_eq!(tool_calls.len(), 1);
    let file_paths = tool_calls[0]
        .fields
        .as_ref()
        .and_then(|f| f.get("apply_file_diffs.diffs.file_path"));
    assert_eq!(
        file_paths,
        Some(&Value::Array(vec!["src/a.ts".into(), "src/b.ts".into()]))
    );
}

#[test]
fn tool_call_result_deltas_for_two_distinct_ids_produce_two_events() {
    // Given — call A's delta, then call B's delta, contiguous in the walk
    let blob = [
        tool_call_result_delta("tc-A", Some(run_shell_command_result_output("A output"))),
        tool_call_result_delta("tc-B", Some(run_shell_command_result_output("B output"))),
    ]
    .concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then
    let results = of_kind(&out, "tool_call_result");
    assert_eq!(results.len(), 2);
    let ids: Vec<Option<&str>> = results.iter().map(|r| r.tool_call_id.as_deref()).collect();
    assert_eq!(ids, vec![Some("tc-A"), Some("tc-B")]);
}

#[test]
fn updated_skills_context_fan_out_collapses_into_one_summary_listing_all_skills() {
    // Given — 3 skills, each streamed as a separate path delta then a separate name delta
    let blob = [
        tool_call_result_delta("tc-2", Some(skill_leaf(1, "skills/a/SKILL.md"))),
        tool_call_result_delta("tc-2", Some(skill_leaf(2, "a-skill"))),
        tool_call_result_delta("tc-2", Some(skill_leaf(1, "skills/b/SKILL.md"))),
        tool_call_result_delta("tc-2", Some(skill_leaf(2, "b-skill"))),
        tool_call_result_delta("tc-2", Some(skill_leaf(1, "skills/c/SKILL.md"))),
        tool_call_result_delta("tc-2", Some(skill_leaf(2, "c-skill"))),
    ]
    .concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then — one event for the whole group, carrying all 3 reconstructed skills
    let results = of_kind(&out, "tool_call_result");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].skills,
        Some(vec![
            skill("skills/a/SKILL.md", "a-skill"),
            skill("skills/b/SKILL.md", "b-skill"),
            skill("skills/c/SKILL.md", "c-skill"),
        ])
    );
    // fields map has no skills leaves leaked into it
    let leaked = results[0]
        .fields
        .as_ref()
        .is_some_and(|f| f.contains_key("context.updated_skills_context.available_skills.path"));
    assert!(!leaked);
}

#[test]
fn same_field_path_with_growing_values_merges_onto_final_value_with_merged_count() {
    // Given — agent_output.text streamed as 3 cumulative chunks
    let blob = [
        agent_output_delta("Hello"),
        agent_output_delta("Hello, world"),
        agent_output_delta("Hello, world!"),
    ]
    .concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].named.message_kind.as_deref(), Some("agent_output"));
    assert_eq!(out[0].named.node.value, "Hello, world!");
    assert_eq!(out[0].merged_count, Some(3));
}

#[test]
fn marker_text_streamed_across_call_and_result_deltas_survives_on_exactly_one_event_each() {
    // Given — marker text spread across tool_call (command) and tool_call_result (output + command echo)
    let marker = "CAPTURE_MARKER v=1 id=demo label=specify";
    let echo = format!("echo '{marker}'");
    let blob = [
        tool_call_delta("tc-marker", None),
        tool_call_delta("tc-marker", Some(run_shell_command_command(&echo))),
        tool_call_result_delta("tc-marker", None),
        tool_call_result_delta("tc-marker", Some(run_shell_command_result_output(marker))),
        tool_call_result_delta("tc-marker", Some(run_shell_command_result_command(&echo))),
    ]
    .concat();
    let before = named(&blob);
    let before_marker_nodes = before.iter().filter(|n| n.node.value.contains(marker)).count();
    // Sanity: pre-collapse, the marker text is indeed spread across multiple leaf nodes
    assert!(before_marker_nodes > 1);

    // When
    let out = collapse_deltas(&before);

    // Then — exactly one tool_call and one tool_call_result event, each carrying the marker once
    let tool_calls = of_kind(&out, "tool_call");
    let tool_call_results = of_kind(&out, "tool_call_result");
    assert_eq!(tool_calls.len(), 1);
    assert_eq!(tool_call_results.len(), 1);

    let contains_marker = |fields: &Option<Object>| {
        fields.as_ref().is_some_and(|f| {
            f.iter().any(|(_, v)| match v {
                Value::Array(items) => items.iter().any(|s| s.as_str().is_some_and(|s| s.contains(marker))),
                other => other.as_str().is_some_and(|s| s.contains(marker)),
            })
        })
    };
    assert!(contains_marker(&tool_calls[0].fields));
    assert!(contains_marker(&tool_call_results[0].fields));
}

#[test]
fn single_occurrences_of_non_grouped_kinds_survive_unchanged() {
    // Given — one occurrence per kind, none of which should ever be grouped/merged
    let blob = [
        user_query_delta("implement the feature"),
        agent_reasoning_delta("I should check the existing tests first"),
        update_todos_delta("Write failing test"),
        messages_received_from_agents_delta("status update from subagent"),
    ]
    .concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then — 4 nodes in, 4 nodes out, values intact
    assert_eq!(out.len(), 4);
    let values: Vec<&str> = out.iter().map(|n| n.named.node.value.as_str()).collect();
    assert_eq!(
        values,
        vec![
            "implement the feature",
            "I should check the existing tests first",
            "Write failing test",
            "status update from subagent",
        ]
    );
    let kinds: Vec<Option<&str>> = out.iter().map(|n| n.named.message_kind.as_deref()).collect();
    assert_eq!(
        kinds,
        vec![
            Some("user_query"),
            Some("agent_reasoning"),
            Some("update_todos"),
            Some("messages_received_from_agents"),
        ]
    );
}

#[test]
fn task_level_scalars_without_message_kind_pass_through_untouched() {
    // Given — Task.id(1) and Task.summary(6), both message_kind-less
    let blob = [encode_string(1, "task-id"), encode_string(6, "a summary")].concat();

    // When
    let out = collapse_deltas(&named(&blob));

    // Then
    assert_eq!(out.len(), 2);
    let values: Vec<&str> = out.iter().map(|n| n.named.node.value.as_str()).collect();
    assert_eq!(values, vec!["task-id", "a summary"]);
    assert!(out.iter().all(|n| n.named.message_kind.is_none()));
}

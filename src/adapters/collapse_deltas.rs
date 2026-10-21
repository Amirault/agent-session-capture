//! Collapses streaming field-deltas emitted for `tool_call` /
//! `tool_call_result` messages, and generically merges any other consecutive
//! same-`message_kind` deltas of one logical entity.
//!
//! Warp streams a call/result incrementally: the task blob holds many separate
//! `Message` occurrences, each setting a single leaf field. This pass:
//!  - groups a contiguous run of `tool_call`/`tool_call_result` nodes that share
//!    one `tool_call_id` into a single event carrying a `fields` map (relative
//!    field path -> value, or -> value[] when the same path recurs with a
//!    different value) plus the `tool_call_id`;
//!  - within such a group, reconstructs `updated_skills_context` fan-out into a
//!    `skills` summary list;
//!  - for any other `message_kind`, merges a run of consecutive nodes sharing
//!    the same `message_kind` AND `field_path` onto the last value, recording a
//!    `merged_count`.
//!
//! Nodes without a `message_kind` always pass through untouched.

use super::schema_overlay::NamedNode;
use crate::js::{Object, Value};

/// A named node after collapsing. `fields` maps a relative path to a string or
/// an array of strings; each skill is `{path?, name?}` in assignment order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CollapsedNode {
    pub named: NamedNode,
    pub tool_call_id: Option<String>,
    pub fields: Option<Object>,
    pub skills: Option<Vec<Object>>,
    pub merged_count: Option<usize>,
}

impl CollapsedNode {
    fn from_named(named: NamedNode) -> Self {
        Self {
            named,
            ..Self::default()
        }
    }
}

const GROUPED_KINDS: [&str; 2] = ["tool_call", "tool_call_result"];
/// Relative-path marker for skills fan-out, matched regardless of nesting depth.
const SKILLS_LEAF: &str = "updated_skills_context.available_skills.";

pub fn collapse_deltas(nodes: &[NamedNode]) -> Vec<CollapsedNode> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < nodes.len() {
        let node = &nodes[i];
        if let Some(kind) = &node.message_kind
            && GROUPED_KINDS.contains(&kind.as_str())
        {
            let end = group_end(nodes, i);
            out.push(collapse_group(&nodes[i..end]));
            i = end;
            continue;
        }
        let end = if node.message_kind.is_some() {
            same_field_run_end(nodes, i)
        } else {
            i + 1
        };
        if end > i + 1 {
            let mut merged = CollapsedNode::from_named(nodes[end - 1].clone());
            merged.merged_count = Some(end - i);
            out.push(merged);
        } else {
            out.push(CollapsedNode::from_named(node.clone()));
        }
        i = end;
    }
    out
}

/// End (exclusive) of a contiguous run sharing `message_kind` and (if present) `tool_call_id`.
fn group_end(nodes: &[NamedNode], start: usize) -> usize {
    let kind = &nodes[start].message_kind;
    let id_path = format!("messages.{}.tool_call_id", kind.as_deref().unwrap_or("undefined"));
    let mut tool_call_id: Option<&str> = None;
    let mut i = start;
    while i < nodes.len() && &nodes[i].message_kind == kind {
        if nodes[i].node.field_path == id_path {
            let id = nodes[i].node.value.as_str();
            if tool_call_id.is_some_and(|t| t != id) {
                break;
            }
            tool_call_id = Some(id);
        }
        i += 1;
    }
    i
}

/// End (exclusive) of a contiguous run sharing both `message_kind` and `field_path`.
fn same_field_run_end(nodes: &[NamedNode], start: usize) -> usize {
    let first = &nodes[start];
    let mut i = start + 1;
    while i < nodes.len()
        && nodes[i].message_kind == first.message_kind
        && nodes[i].node.field_path == first.node.field_path
    {
        i += 1;
    }
    i
}

fn collapse_group(group: &[NamedNode]) -> CollapsedNode {
    let kind = group[0].message_kind.clone().unwrap_or_default();
    let prefix = format!("messages.{kind}.");
    let mut fields = Object::new();
    let mut skill_nodes: Vec<&NamedNode> = Vec::new();
    let mut tool_call_id = None;
    for node in group {
        let path = &node.node.field_path;
        let rel = path.strip_prefix(&prefix).unwrap_or(path);
        if rel == "tool_call_id" {
            tool_call_id = Some(node.node.value.clone());
            continue;
        }
        if rel.contains(SKILLS_LEAF) {
            skill_nodes.push(node);
            continue;
        }
        add_field(&mut fields, rel, &node.node.value);
    }
    let skills = collapse_skills(&skill_nodes);

    let mut first = group[0].clone();
    first.node.field_path = format!("messages.{kind}");
    first.node.value = summarize_collapsed_node(&fields, &skills);
    if let Some(tool) = group.iter().find_map(|n| n.tool.clone()) {
        first.tool = Some(tool);
    }
    CollapsedNode {
        named: first,
        tool_call_id,
        fields: if fields.is_empty() { None } else { Some(fields) },
        skills: if skills.is_empty() { None } else { Some(skills) },
        merged_count: if group.len() > 1 { Some(group.len()) } else { None },
    }
}

/// Records a relative-field value without overwriting an earlier, distinct
/// value for the same path: a repeated field legitimately carries a different
/// value per occurrence.
fn add_field(fields: &mut Object, rel: &str, value: &str) {
    match fields.get_mut(rel) {
        None => fields.insert(rel, value),
        Some(Value::Array(items)) => {
            if items.last().and_then(Value::as_str) != Some(value) {
                items.push(Value::from(value));
            }
        }
        Some(existing) => {
            if existing.as_str() != Some(value) {
                let previous = existing.clone();
                *existing = Value::Array(vec![previous, Value::from(value)]);
            }
        }
    }
}

/// The event text of a collapsed node: the fields JSON and a skills summary.
pub fn summarize_collapsed_node(fields: &Object, skills: &[Object]) -> String {
    let mut parts = Vec::new();
    if !fields.is_empty() {
        parts.push(fields.to_json());
    }
    if !skills.is_empty() {
        let names: Vec<&str> = skills
            .iter()
            .map(|s| s.get_str("path").or_else(|| s.get_str("name")).unwrap_or("?"))
            .collect();
        parts.push(format!("skills_context({}): {}", skills.len(), names.join(", ")));
    }
    parts.join(" ")
}

/// Rebuild skill records from a flat run of `path`/`name` leaves: a new record
/// starts whenever a `path` arrives and the current record already has one.
fn collapse_skills(nodes: &[&NamedNode]) -> Vec<Object> {
    let mut skills = Vec::new();
    let mut current: Option<Object> = None;
    for node in nodes {
        let path = &node.node.field_path;
        let idx = path.rfind(SKILLS_LEAF).expect("skills leaf");
        let leaf = &path[idx + SKILLS_LEAF.len()..];
        if leaf == "path" {
            if current.as_ref().is_some_and(|c| c.contains_key("path")) {
                skills.push(current.take().expect("current"));
            }
            current
                .get_or_insert_with(Object::new)
                .insert("path", node.node.value.as_str());
        } else if leaf == "name" {
            current
                .get_or_insert_with(Object::new)
                .insert("name", node.node.value.as_str());
        }
    }
    if let Some(current) = current {
        skills.push(current);
    }
    skills
}

//! Keep one copy of static project rules and skill context across task rows.

use std::collections::HashSet;

use super::collapse_deltas::{CollapsedNode, summarize_collapsed_node};
use crate::domain::merge_bundle::stable_sort_by;
use crate::js::string::locale_compare;
use crate::js::{Object, Value};

const PROJECT_RULES_PREFIX: &str = "context.project_rules.";
const SIGNAL_MESSAGE_KINDS: [&str; 4] = [
    "agent_reasoning",
    "messages_received_from_agents",
    "update_todos",
    "user_query",
];

/// What was already seen in one conversation.
#[derive(Default)]
pub struct EnvelopeDedupeState {
    seen_project_rules: HashSet<String>,
    seen_skill_sets: HashSet<String>,
}

/// Remove the repeated envelope payload of one node; signal fields on a mixed
/// tool event remain untouched. `None` when nothing is left.
pub fn dedupe_envelope(node: &CollapsedNode, state: &mut EnvelopeDedupeState) -> Option<CollapsedNode> {
    if node
        .named
        .message_kind
        .as_deref()
        .is_some_and(|k| SIGNAL_MESSAGE_KINDS.contains(&k))
    {
        return Some(node.clone());
    }

    let mut fields = node.fields.clone().unwrap_or_default();
    let mut rule_entries: Vec<(&String, &Value)> = Vec::new();
    let all_fields = node.fields.clone().unwrap_or_default();
    for (k, v) in all_fields.iter() {
        if k.starts_with(PROJECT_RULES_PREFIX) {
            rule_entries.push((k, v));
        }
    }
    stable_sort_by(&mut rule_entries, |a, b| locale_compare(a.0, b.0));
    let mut project_rules_removed = false;
    if !rule_entries.is_empty() {
        let key = Value::Array(
            rule_entries
                .iter()
                .map(|(k, v)| Value::Array(vec![Value::from(k.as_str()), (*v).clone()]))
                .collect(),
        )
        .to_json();
        if state.seen_project_rules.contains(&key) {
            for (field, _) in &rule_entries {
                fields.remove(field);
            }
            project_rules_removed = true;
        } else {
            state.seen_project_rules.insert(key);
        }
    }

    let mut skills = node.skills.clone();
    let mut skills_removed = false;
    if let Some(list) = &skills
        && !list.is_empty()
    {
        let key = skill_set_key(list);
        if state.seen_skill_sets.contains(&key) {
            skills = None;
            skills_removed = true;
        } else {
            state.seen_skill_sets.insert(key);
        }
    }

    if !project_rules_removed && !skills_removed {
        return Some(node.clone());
    }
    let remaining_skills = skills.unwrap_or_default();
    if fields.is_empty() && remaining_skills.is_empty() {
        return None;
    }
    let mut stripped = node.clone();
    stripped.named.node.value = summarize_collapsed_node(&fields, &remaining_skills);
    stripped.fields = if fields.is_empty() { None } else { Some(fields) };
    stripped.skills = if remaining_skills.is_empty() {
        None
    } else {
        Some(remaining_skills)
    };
    Some(stripped)
}

fn skill_set_key(skills: &[Object]) -> String {
    let identity = |s: &Object| {
        Value::Array(vec![
            s.get("path").cloned().unwrap_or(Value::Null),
            s.get("name").cloned().unwrap_or(Value::Null),
        ])
        .to_json()
    };
    let mut identities: Vec<String> = skills.iter().map(identity).collect();
    stable_sort_by(&mut identities, |a, b| locale_compare(a, b));
    format!("[{}]", identities.join(","))
}

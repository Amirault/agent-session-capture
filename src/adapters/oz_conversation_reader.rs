//! Marker and sessions modes over headless Warp `oz agent run` sessions.
//!
//! A headless run writes only shell rows to Warp's local SQLite, so its
//! conversation is read from Warp's server instead (see
//! [`oz_run_store`](super::oz_run_store)). A conversation binds to a label
//! through an exact marker line run as its own `run_command` action.

use super::compact::truncate_long;
use super::oz_run_store::OzRunStore;
use crate::domain::capture_marker::exact_markers_in_command;
use crate::domain::merge_bundle::stable_sort_by;
use crate::domain::models::{BundleSource, EventDraft, EventKind, Label, Role, SessionInfo, SkippedRow};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{
    CaptureRead, ConversationReader, ReadSessionsOptions, SessionGather, SessionReader, SessionsRead,
};
use crate::domain::skip_unchanged::should_gather;
use crate::domain::source_errors::ReadError;
use crate::domain::stamps::{parse_since_utc, parse_stamp_utc};
use crate::js::string::{js_cmp, locale_compare};
use crate::js::{Object, Value};

const SHELL_ACTION: &str = "run_command";

pub struct OzConversationReader<S: OzRunStore> {
    store: S,
}

impl<S: OzRunStore> OzConversationReader<S> {
    pub fn new(store: S) -> Self {
        Self { store }
    }

    /// Every run with a conversation id, first run of a conversation wins;
    /// a run without one is reported.
    fn runs(&self, skipped: &mut Vec<SkippedRow>) -> Result<Vec<(String, Object)>, ReadError> {
        let mut by_cid: OrderedMap<String, Object> = OrderedMap::new();
        for run in self.store.runs()? {
            match run.get_str("conversation_id").filter(|c| !c.is_empty()) {
                Some(cid) => {
                    if !by_cid.contains_key(&cid.to_string()) {
                        by_cid.insert(cid.to_string(), run.clone());
                    }
                }
                None => skipped.push(SkippedRow::new(
                    "runs",
                    "run has no conversation_id",
                    run.get_str("run_id").unwrap_or("?"),
                )),
            }
        }
        Ok(by_cid.into_entries().collect())
    }
}

impl<S: OzRunStore> ConversationReader for OzConversationReader<S> {
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError> {
        let mut skipped = Vec::new();
        let mut label_by_cid = OrderedMap::new();
        let mut drafts = Vec::new();
        for (cid, run) in self.runs(&mut skipped)? {
            let conversation = self.store.conversation(&run)?;
            let mut labels: Vec<Label> = Vec::new();
            for command in shell_commands(&conversation) {
                for marker in exact_markers_in_command(&command) {
                    if marker.capture_id == capture_id && !labels.contains(&marker.label) {
                        labels.push(marker.label);
                    }
                }
            }
            match labels.len() {
                0 => continue,
                1 => label_by_cid.insert(cid, labels.remove(0)),
                _ => {
                    skipped.push(SkippedRow::new(
                        "conversations",
                        "same conversation has markers for multiple labels",
                        format!("{cid}: {}", labels.join(", ")),
                    ));
                    continue;
                }
            }
            drafts.extend(drafts_from_run(&run, &conversation));
        }
        Ok(CaptureRead {
            source: BundleSource::Oz,
            label_by_cid,
            drafts,
            skipped,
            unbindable: vec![],
            collisions: vec![],
            heuristic_bindings: vec![],
        })
    }
}

impl<S: OzRunStore> SessionReader for OzConversationReader<S> {
    /// A session is one run's conversation, stamped with the run's `updated_at`.
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError> {
        let cutoff = options.since.as_deref().and_then(parse_since_utc);
        let mut skipped = Vec::new();
        let mut runs = self.runs(&mut skipped)?;
        runs.retain(|(cid, run)| {
            let updated = run
                .get_str("updated_at")
                .and_then(|s| parse_stamp_utc(s.trim_end_matches('Z')));
            options.conversation.as_ref().is_none_or(|c| c == cid)
                && cutoff.is_none_or(|cutoff| updated.is_some_and(|u| u >= cutoff))
        });
        stable_sort_by(&mut runs, |a, b| locale_compare(&a.0, &b.0));
        let mut sessions = Vec::new();
        let mut reads = OrderedMap::new();
        for (cid, run) in runs {
            let info = SessionInfo {
                conversation_id: cid.clone(),
                last_modified: run.get_str("updated_at").map(str::to_string),
            };
            if should_gather(&info, options) {
                let conversation = self.store.conversation(&run)?;
                reads.insert(
                    cid,
                    SessionGather {
                        drafts: drafts_from_run(&run, &conversation),
                        skipped: std::mem::take(&mut skipped),
                        last_modified: info.last_modified.clone(),
                    },
                );
            }
            sessions.push(info);
        }
        Ok(SessionsRead {
            source: BundleSource::Oz,
            sessions,
            reads,
        })
    }
}

/// Every message of every step, nested steps included, in time order.
fn messages(conversation: &Object) -> Vec<(String, &Object)> {
    fn walk<'a>(steps: Option<&'a Value>, last_ts: &mut String, out: &mut Vec<(String, &'a Object)>) {
        for step in steps
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_object)
        {
            for message in step
                .get("messages")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_object)
            {
                if let Some(ts) = message.get_str("timestamp") {
                    *last_ts = sortable_ts(ts);
                }
                out.push((last_ts.clone(), message));
            }
            walk(step.get("steps"), last_ts, out);
        }
    }
    let mut out = Vec::new();
    walk(conversation.get("steps"), &mut String::new(), &mut out);
    stable_sort_by(&mut out, |a, b| js_cmp(&a.0, &b.0));
    out
}

/// An RFC 3339 UTC timestamp with its fraction padded to nanoseconds, so
/// timestamps sort as strings (`oz` trims trailing zeros:
/// `…:25.35855734Z`); anything else is kept as is.
fn sortable_ts(ts: &str) -> String {
    let Some(body) = ts.strip_suffix('Z') else {
        return ts.to_string();
    };
    let (seconds, fraction) = body.split_once('.').unwrap_or((body, ""));
    if !seconds.contains('T') || fraction.len() > 9 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return ts.to_string();
    }
    format!("{seconds}.{fraction:0<9}Z")
}

fn content_items(message: &Object) -> impl Iterator<Item = &Object> {
    message
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_object)
}

/// The shell commands the conversation's assistant ran.
fn shell_commands(conversation: &Object) -> Vec<String> {
    messages(conversation)
        .into_iter()
        .filter(|(_, m)| m.get_str("role") == Some("assistant"))
        .flat_map(|(_, m)| content_items(m))
        .filter(|c| c.get_str("type") == Some("action") && c.get_str("name") == Some(SHELL_ACTION))
        .filter_map(|c| input_command(c).map(str::to_string))
        .collect()
}

fn input_command(action: &Object) -> Option<&str> {
    action.get("input")?.as_object()?.get_str("command")
}

/// One run's conversation → events: user text is the `query`, assistant text
/// an `agent_message`, a `run_command` action a `command` and any other action
/// a `tool_call`, an action result a `tool_result`. The run's `request_usage`
/// rides once, on the first event, as `meta.usage`.
pub fn drafts_from_run(run: &Object, conversation: &Object) -> Vec<EventDraft> {
    let cid = conversation
        .get_str("conversation_id")
        .or_else(|| run.get_str("conversation_id"))
        .unwrap_or_default();
    let model = run
        .get("agent_config")
        .and_then(Value::as_object)
        .and_then(|c| c.get("model_id"))
        .filter(|v| !v.is_null());
    let mut drafts = Vec::new();
    for (ts, message) in messages(conversation) {
        let mut base = Object::new();
        if let Some(model) = model.filter(|_| message.get_str("role") == Some("assistant")) {
            base.insert("model", model.clone());
        }
        if let Some(request_id) = message.get("request_id").filter(|v| !v.is_null()) {
            base.insert("exchange_id", request_id.clone());
        }
        if let Some(run_id) = run.get("run_id").filter(|v| !v.is_null()) {
            base.insert("run_id", run_id.clone());
        }
        let role = message.get_str("role");
        for item in content_items(message) {
            let mut meta = base.clone();
            let mapped = match (role, item.get_str("type")) {
                (Some("user"), Some("text")) => text_of(item).map(|t| (Role::User, EventKind::Query, t)),
                (Some("assistant"), Some("text")) => {
                    text_of(item).map(|t| (Role::Assistant, EventKind::AgentMessage, t))
                }
                (Some("assistant"), Some("action")) => {
                    let name = item.get_str("name");
                    let input = item.get("input").cloned().unwrap_or(Value::Null);
                    copy(item, "name", &mut meta, "tool");
                    copy(item, "id", &mut meta, "tool_call_id");
                    meta.insert("fields", truncate_leaves(input.clone()));
                    match (name, input_command(item)) {
                        (Some(SHELL_ACTION), Some(command)) => {
                            Some((Role::Assistant, EventKind::Command, command.to_string()))
                        }
                        _ => Some((Role::Assistant, EventKind::ToolCall, input.to_json())),
                    }
                }
                (Some("tool"), Some("action_result")) => {
                    copy(item, "name", &mut meta, "tool");
                    copy(item, "action_id", &mut meta, "tool_call_id");
                    let output = item.get("output").cloned().unwrap_or(Value::Null);
                    let finished = output
                        .as_object()
                        .and_then(|o| o.get("command_finished"))
                        .and_then(Value::as_object);
                    match finished {
                        Some(finished) => {
                            if let Some(code) = finished.get("exit_code").filter(|v| !v.is_null()) {
                                meta.insert("exit_code", code.clone());
                            }
                            let text = finished.get_str("output").unwrap_or_default().to_string();
                            Some((Role::Tool, EventKind::ToolResult, text))
                        }
                        None => Some((Role::Tool, EventKind::ToolResult, output.to_json())),
                    }
                }
                _ => None,
            };
            let Some((role, kind, content)) = mapped else { continue };
            let content = match truncate_long(&content) {
                Some((short, length)) => {
                    meta.insert("truncated", true);
                    meta.insert("original_len", length);
                    short
                }
                None => content,
            };
            if drafts.is_empty()
                && let Some(usage) = run.get("request_usage").filter(|v| !v.is_null())
            {
                meta.insert("usage", usage.clone());
            }
            drafts.push(EventDraft {
                conversation_id: cid.to_string(),
                ts: ts.clone(),
                role,
                kind,
                content,
                meta,
            });
        }
    }
    drafts
}

fn text_of(item: &Object) -> Option<String> {
    item.get_str("text").filter(|t| !t.is_empty()).map(str::to_string)
}

fn copy(from: &Object, key: &str, meta: &mut Object, as_key: &str) {
    if let Some(value) = from.get(key).filter(|v| !v.is_null()) {
        meta.insert(as_key, value.clone());
    }
}

/// Long strings anywhere in a tool input (file contents, diffs) truncated.
fn truncate_leaves(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(truncate_long(&s).map_or(s, |(short, _)| short)),
        Value::Array(items) => Value::Array(items.into_iter().map(truncate_leaves).collect()),
        Value::Object(object) => Value::Object(
            object
                .iter()
                .map(|(k, v)| (k.clone(), truncate_leaves(v.clone())))
                .collect(),
        ),
        other => other,
    }
}

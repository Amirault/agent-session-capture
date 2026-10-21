//! Claude Code JSONL transcripts (`~/.claude/projects/**/*.jsonl`).
//!
//! Marker mode binds a `Bash` `tool_use` running the marker; conversations are
//! keyed by `sessionId`, so subagent transcripts
//! (`<session>/subagents/agent-<id>.jsonl`) join their parent — unless one
//! session's markers carry several labels, in which case each subagent is keyed
//! `<sessionId>/agent-<agentId>` and bound to its own label. Session mode
//! exports one file per `sessionId`.

use std::collections::{HashMap, HashSet};
use std::fs;

use super::jsonl_bundle_reader::read_text;
use super::readers::marker_reader::parse_marker;
use crate::domain::merge_bundle::stable_sort_by;
use crate::domain::models::{BundleSource, EventDraft, EventKind, Label, Role, SessionInfo, SkippedRow};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{
    CaptureRead, ConversationReader, ReadSessionsOptions, SessionGather, SessionReader, SessionsRead,
};
use crate::domain::skip_unchanged::should_gather;
use crate::domain::source_errors::ReadError;
use crate::domain::stamps::parse_since_utc;
use crate::js::string::{js_cmp, js_trim};
use crate::js::{Object, Value, date, json, path, spread_push};

pub struct ClaudeCodeTranscriptReader {
    root_dir: String,
}

struct JsonlRecord {
    file_path: String,
    line_number: usize,
    value: Object,
}

#[derive(Clone)]
struct MarkerHit {
    conversation_id: String,
    label: Label,
    start_ts: String,
}

/// One Claude Code session: its `<session>.jsonl` plus `<session>/subagents/agent-*.jsonl`.
struct SessionFiles {
    info: SessionInfo,
    stamp: String,
    files: Vec<String>,
}

impl ClaudeCodeTranscriptReader {
    pub fn new(root_dir: Option<String>) -> Self {
        let root_dir = root_dir.unwrap_or_else(|| path::join(&[&path::home_dir(), ".claude", "projects"]));
        Self { root_dir }
    }
}

impl ConversationReader for ClaudeCodeTranscriptReader {
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError> {
        let files = list_jsonl_files(&self.root_dir);
        let mut records = Vec::new();
        let mut skipped = Vec::new();
        if fs::metadata(&self.root_dir).is_err() {
            skipped.push(SkippedRow::new(
                "claude-root",
                "transcript root not found",
                self.root_dir.clone(),
            ));
        }
        for file in &files {
            let (file_records, file_skipped) = read_jsonl(file)?;
            spread_push(&mut records, file_records).map_err(ReadError::Other)?;
            spread_push(&mut skipped, file_skipped).map_err(ReadError::Other)?;
        }

        let split_sessions = multi_label_sessions(&find_marker_hits(&records, capture_id, &HashSet::new()));
        let marker_hits = find_marker_hits(&records, capture_id, &split_sessions);
        let label_by_cid = bind_labels(&marker_hits, &mut skipped);
        let bound: HashSet<String> = label_by_cid.keys().cloned().collect();
        let mut drafts = Vec::new();
        for record in &records {
            drafts.extend(drafts_from_record(
                record,
                &bound,
                &split_sessions,
                &self.root_dir,
                &mut skipped,
            ));
        }
        Ok(CaptureRead {
            source: BundleSource::ClaudeCode,
            label_by_cid,
            drafts,
            skipped,
            unbindable: vec![],
            collisions: vec![],
            heuristic_bindings: vec![],
        })
    }
}

impl SessionReader for ClaudeCodeTranscriptReader {
    /// One session per `sessionId`; `last_modified` is the latest mtime of its
    /// files (ISO-8601 UTC). Gathering reuses the marker-mode record mapping,
    /// with the session as the only bound conversation.
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError> {
        let mut selected = enumerate_sessions(&self.root_dir)?;
        if let Some(conversation) = &options.conversation {
            selected.retain(|s| &s.info.conversation_id == conversation);
        }
        if let Some(since) = &options.since
            && let Some(cutoff) = parse_since_utc(since)
        {
            selected.retain(|s| date::parse(&s.stamp) >= cutoff);
        }
        let mut reads = OrderedMap::new();
        for session in &selected {
            if !should_gather(&session.info, options) {
                continue;
            }
            reads.insert(
                session.info.conversation_id.clone(),
                gather_session(session, &self.root_dir)?,
            );
        }
        Ok(SessionsRead {
            source: BundleSource::ClaudeCode,
            sessions: selected.into_iter().map(|s| s.info).collect(),
            reads,
        })
    }
}

/// `fs.statSync(file).mtimeMs`.
fn mtime_ms(file: &str) -> Result<f64, ReadError> {
    let modified = fs::metadata(file)
        .and_then(|m| m.modified())
        .map_err(|e| ReadError::Other(e.to_string()))?;
    Ok(match modified.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as f64 * 1000.0 + f64::from(d.subsec_nanos()) / 1e6,
        Err(e) => -(e.duration().as_secs() as f64 * 1000.0 + f64::from(e.duration().subsec_nanos()) / 1e6),
    })
}

/// Group every transcript file by its session id, ordered by stamp then id.
fn enumerate_sessions(root_dir: &str) -> Result<Vec<SessionFiles>, ReadError> {
    let mut files_by_session: OrderedMap<String, Vec<String>> = OrderedMap::new();
    for file in list_jsonl_files(root_dir) {
        files_by_session
            .entry_or_insert_with(session_id_of_file(&file), Vec::new)
            .push(file);
    }
    let mut sessions = Vec::new();
    for (conversation_id, files) in files_by_session.into_entries() {
        let mut latest = f64::NEG_INFINITY;
        for file in &files {
            latest = latest.max(mtime_ms(file)?);
        }
        let stamp = date::to_iso_string(latest).ok_or_else(|| ReadError::Other("Invalid time value".to_string()))?;
        sessions.push(SessionFiles {
            info: SessionInfo {
                conversation_id,
                last_modified: Some(stamp.clone()),
            },
            stamp,
            files,
        });
    }
    stable_sort_by(&mut sessions, |a, b| {
        js_cmp(&a.stamp, &b.stamp).then_with(|| js_cmp(&a.info.conversation_id, &b.info.conversation_id))
    });
    Ok(sessions)
}

fn gather_session(session: &SessionFiles, root_dir: &str) -> Result<SessionGather, ReadError> {
    let bound: HashSet<String> = [session.info.conversation_id.clone()].into();
    let mut skipped = Vec::new();
    let mut drafts = Vec::new();
    for file in &session.files {
        let (records, file_skipped) = read_jsonl(file)?;
        skipped.extend(file_skipped);
        for record in &records {
            drafts.extend(drafts_from_record(
                record,
                &bound,
                &HashSet::new(),
                root_dir,
                &mut skipped,
            ));
        }
    }
    Ok(SessionGather {
        drafts,
        skipped,
        last_modified: session.info.last_modified.clone(),
    })
}

fn session_id_of_file(file: &str) -> String {
    let parent = path::dirname(file);
    if path::basename(parent) == "subagents" {
        return path::basename(path::dirname(parent)).to_string();
    }
    path::basename_without(file, ".jsonl").to_string()
}

/// Every `*.jsonl` file under `root_dir` (symlinks not followed), sorted.
fn list_jsonl_files(root_dir: &str) -> Vec<String> {
    let mut files = Vec::new();
    collect_jsonl_files(root_dir, &mut files);
    files.sort_by(|a, b| js_cmp(a, b));
    files
}

fn collect_jsonl_files(dir: &str, files: &mut Vec<String>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let full = path::join(&[dir, &name]);
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_dir() {
            collect_jsonl_files(&full, files);
        } else if file_type.is_file() && name.ends_with(".jsonl") {
            files.push(full);
        }
    }
}

fn skipped_line(file: &str, line_number: usize, reason: &str) -> SkippedRow {
    SkippedRow::new("claude_code_transcripts", reason, format!("{file}:{line_number}"))
}

fn read_jsonl(file: &str) -> Result<(Vec<JsonlRecord>, Vec<SkippedRow>), ReadError> {
    let text = read_text(file).map_err(ReadError::Other)?;
    let mut records = Vec::new();
    let mut skipped = Vec::new();
    for (index, line) in text.split('\n').enumerate() {
        let line_number = index + 1;
        if js_trim(line).is_empty() {
            continue;
        }
        match json::parse(line) {
            Ok(Value::Object(value)) => records.push(JsonlRecord {
                file_path: file.to_string(),
                line_number,
                value,
            }),
            Ok(_) => skipped.push(skipped_line(file, line_number, "json value is not an object")),
            Err(message) => skipped.push(skipped_line(file, line_number, &format!("invalid JSON: {message}"))),
        }
    }
    Ok((records, skipped))
}

fn find_marker_hits(records: &[JsonlRecord], capture_id: &str, split_sessions: &HashSet<String>) -> Vec<MarkerHit> {
    let mut hits = Vec::new();
    for record in records {
        let conversation_id = conversation_id(record, split_sessions);
        let Some(start_ts) = record.value.get_str("timestamp") else {
            continue;
        };
        for candidate in marker_candidate_texts(&record.value) {
            let Some(parsed) = parse_marker(candidate) else {
                continue;
            };
            if parsed.capture_id != capture_id {
                continue;
            }
            hits.push(MarkerHit {
                conversation_id: conversation_id.clone(),
                label: parsed.label,
                start_ts: start_ts.to_string(),
            });
        }
    }
    stable_sort_by(&mut hits, |a, b| js_cmp(&a.start_ts, &b.start_ts));
    hits
}

fn multi_label_sessions(hits: &[MarkerHit]) -> HashSet<String> {
    let mut labels_by_session: HashMap<&str, HashSet<&str>> = HashMap::new();
    for hit in hits {
        labels_by_session
            .entry(&hit.conversation_id)
            .or_default()
            .insert(&hit.label);
    }
    labels_by_session
        .into_iter()
        .filter(|(_, labels)| labels.len() > 1)
        .map(|(s, _)| s.to_string())
        .collect()
}

fn bind_labels(hits: &[MarkerHit], skipped: &mut Vec<SkippedRow>) -> OrderedMap<String, Label> {
    let mut label_by_cid: OrderedMap<String, Label> = OrderedMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    for hit in hits {
        if let Some(previous) = label_by_cid.get(&hit.conversation_id) {
            if previous != &hit.label {
                skipped.push(SkippedRow::new(
                    "claude_code_transcripts",
                    "same session has markers for multiple labels",
                    format!("{}: {} then {}", hit.conversation_id, previous, hit.label),
                ));
                label_by_cid.remove(&hit.conversation_id);
                seen.insert(hit.conversation_id.clone());
            }
            continue;
        }
        if seen.contains(&hit.conversation_id) {
            continue;
        }
        label_by_cid.insert(hit.conversation_id.clone(), hit.label.clone());
    }
    label_by_cid
}

fn drafts_from_record(
    record: &JsonlRecord,
    bound: &HashSet<String>,
    split_sessions: &HashSet<String>,
    root_dir: &str,
    skipped: &mut Vec<SkippedRow>,
) -> Vec<EventDraft> {
    let conversation_id = conversation_id(record, split_sessions);
    if !bound.contains(&conversation_id) {
        return vec![];
    }
    // Session metadata records (custom-title, last-prompt, …) are not events.
    let kind = record.value.get_str("type");
    if kind != Some("user") && kind != Some("assistant") {
        return vec![];
    }
    let Some(ts) = record.value.get_str("timestamp") else {
        skipped.push(skipped_line(&record.file_path, record.line_number, "missing timestamp"));
        return vec![];
    };
    let Some(Value::Object(message)) = record.value.get("message") else {
        return vec![];
    };
    let context = DraftContext {
        record,
        root_dir,
        conversation_id: &conversation_id,
        ts,
    };
    if kind == Some("user") {
        user_drafts(&context, message)
    } else {
        assistant_drafts(&context, message)
    }
}

struct DraftContext<'a> {
    record: &'a JsonlRecord,
    root_dir: &'a str,
    conversation_id: &'a str,
    ts: &'a str,
}

impl DraftContext<'_> {
    fn draft(&self, role: Role, kind: EventKind, content: String, extra: Vec<(&str, Option<Value>)>) -> EventDraft {
        let v = &self.record.value;
        let string_field = |key: &str| v.get_str(key).map(Value::from);
        let mut fields: Vec<(&str, Option<Value>)> = vec![
            ("uuid", string_field("uuid")),
            ("parent_uuid", string_field("parentUuid")),
            ("cwd", string_field("cwd")),
            ("git_branch", string_field("gitBranch")),
            ("claude_version", string_field("version")),
            ("is_sidechain", v.get("isSidechain").cloned()),
            (
                "source_file",
                Some(Value::from(path::relative(self.root_dir, &self.record.file_path))),
            ),
            ("source_line", Some(Value::from(self.record.line_number))),
        ];
        fields.extend(extra);
        let mut meta = Object::new();
        for (key, value) in fields {
            match value {
                None | Some(Value::Null) => {}
                Some(value) => meta.insert(key, value),
            }
        }
        EventDraft {
            conversation_id: self.conversation_id.to_string(),
            ts: self.ts.to_string(),
            role,
            kind,
            content,
            meta,
        }
    }
}

fn user_drafts(c: &DraftContext<'_>, message: &Object) -> Vec<EventDraft> {
    match message.get("content") {
        Some(Value::String(content)) => vec![c.draft(Role::User, EventKind::Query, content.clone(), vec![])],
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| {
                let Value::Object(block) = block else { return None };
                match block.get_str("type") {
                    Some("tool_result") => Some(c.draft(
                        Role::Tool,
                        EventKind::ToolResult,
                        content_text(block.get("content")),
                        vec![("tool_use_id", block.get_str("tool_use_id").map(Value::from))],
                    )),
                    Some("text") => {
                        Some(c.draft(Role::User, EventKind::Query, content_text(block.get("text")), vec![]))
                    }
                    _ => None,
                }
            })
            .collect(),
        _ => vec![],
    }
}

fn assistant_drafts(c: &DraftContext<'_>, message: &Object) -> Vec<EventDraft> {
    let Some(Value::Array(blocks)) = message.get("content") else {
        return vec![];
    };
    let model = || message.get_str("model").map(Value::from);
    let message_id = || message.get_str("id").map(Value::from);
    blocks
        .iter()
        .filter_map(|block| {
            let Value::Object(block) = block else { return None };
            match block.get_str("type") {
                Some(kind @ ("text" | "thinking")) => {
                    let text = match block.get("text") {
                        None | Some(Value::Null) => block.get("thinking"),
                        some => some,
                    };
                    Some(c.draft(
                        Role::Assistant,
                        EventKind::AgentMessage,
                        content_text(text),
                        vec![
                            ("message_kind", Some(Value::from(kind))),
                            ("model", model()),
                            ("message_id", message_id()),
                        ],
                    ))
                }
                Some("tool_use") => {
                    let input = match block.get("input") {
                        Some(Value::Object(o)) => o.clone(),
                        _ => Object::new(),
                    };
                    let tool = block.get_str("name");
                    Some(c.draft(
                        Role::Assistant,
                        EventKind::ToolCall,
                        tool_call_content(tool, &input),
                        vec![
                            ("message_kind", Some(Value::from("tool_use"))),
                            ("tool", tool.map(Value::from)),
                            ("tool_use_id", block.get_str("id").map(Value::from)),
                            ("input", Some(Value::Object(input))),
                            ("model", model()),
                            ("message_id", message_id()),
                        ],
                    ))
                }
                _ => None,
            }
        })
        .collect()
}

fn marker_candidate_texts(value: &Object) -> Vec<&str> {
    if value.get_str("type") != Some("assistant") {
        return vec![];
    }
    let Some(Value::Object(message)) = value.get("message") else {
        return vec![];
    };
    let Some(Value::Array(blocks)) = message.get("content") else {
        return vec![];
    };
    blocks
        .iter()
        .filter_map(|block| {
            let block = block.as_object()?;
            if block.get_str("type") != Some("tool_use") || block.get_str("name") != Some("Bash") {
                return None;
            }
            block.get("input")?.as_object()?.get_str("command")
        })
        .collect()
}

fn tool_call_content(tool: Option<&str>, input: &Object) -> String {
    match input.get_str("command") {
        Some(command) if tool == Some("Bash") => command.to_string(),
        _ => input.to_json(),
    }
}

fn content_text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| content_text(Some(item)))
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(Value::Object(o)) => match o.get_str("text").or_else(|| o.get_str("content")) {
            Some(text) => text.to_string(),
            None => o.to_json(),
        },
        None | Some(Value::Null) => String::new(),
        Some(other) => other.to_json(),
    }
}

fn conversation_id(record: &JsonlRecord, split_sessions: &HashSet<String>) -> String {
    let session_id = record
        .value
        .get_str("sessionId")
        .map(str::to_string)
        .unwrap_or_else(|| path::basename_without(&record.file_path, ".jsonl").to_string());
    match record.value.get_str("agentId") {
        Some(agent_id) if split_sessions.contains(&session_id) => format!("{session_id}/agent-{agent_id}"),
        _ => session_id,
    }
}

//! Hermes' canonical `<HERMES_HOME>/state.db` SQLite store.
//!
//! Marker mode binds an exact marker line run by an assistant `terminal` /
//! `run_shell_command` tool call; compression continuations and delegate
//! subagents inherit their parent's binding through `parent_session_id`.
//! Session mode exports one file per lineage root. Every read runs inside one
//! deferred read transaction on a read-only connection.

use std::collections::{HashMap, HashSet};

use super::sqlite_readable_db::{Cell, ReadableDb, Row};
use crate::domain::capture_marker::{ExactMarker, exact_markers_in_command};
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
use crate::js::{Object, Value, date, json, path};

pub struct HermesConversationReader {
    explicit_db_path: Option<String>,
}

#[derive(Clone, Debug)]
struct SessionRow {
    id: String,
    source: Option<String>,
    model: Cell,
    parent_session_id: Option<String>,
    end_reason: Option<String>,
    model_config: Option<String>,
}

struct MessageRow {
    id: Cell,
    session_id: String,
    role: Option<String>,
    content: Option<String>,
    tool_call_id: Cell,
    tool_calls: Option<String>,
    tool_name: Cell,
    timestamp: Cell,
    active: Cell,
    compacted: Cell,
}

struct ToolCall {
    id: String,
    name: String,
    arguments: Object,
}

struct ParsedMessage {
    row: MessageRow,
    tool_calls: Vec<ToolCall>,
}

struct Columns {
    sessions: HashSet<String>,
    messages: HashSet<String>,
}

/// One Hermes session for `--sessions`: a lineage root and the rows that join it.
struct SessionGroup {
    info: SessionInfo,
    members: Vec<SessionRow>,
}

const REQUIRED_SESSIONS: [&str; 3] = ["id", "source", "parent_session_id"];
const REQUIRED_MESSAGES: [&str; 8] = [
    "id",
    "session_id",
    "role",
    "content",
    "tool_call_id",
    "tool_calls",
    "tool_name",
    "timestamp",
];
const MARKER_TOOLS: [&str; 2] = ["terminal", "run_shell_command"];
const MARKER_ANCHOR: &str = "CAPTURE_MARKER";

fn cell_value(cell: &Cell) -> Value {
    super::readers::cell_to_value(cell)
}

impl HermesConversationReader {
    pub fn new(db_path: Option<String>) -> Self {
        Self {
            explicit_db_path: db_path,
        }
    }

    pub fn resolve_db_path(&self) -> String {
        if let Some(explicit) = &self.explicit_db_path {
            return explicit.clone();
        }
        if let Ok(home) = std::env::var("HERMES_HOME") {
            let home = js_trim(&home);
            if !home.is_empty() {
                return path::join(&[home, "state.db"]);
            }
        }
        path::join(&[&path::home_dir(), ".hermes", "state.db"])
    }

    /// Open the DB read-only and run `f` inside one deferred read transaction.
    fn with_snapshot<T>(&self, f: impl FnOnce(&ReadableDb, &Columns) -> Result<T, ReadError>) -> Result<T, ReadError> {
        let db_path = self.resolve_db_path();
        let db = ReadableDb::open_read_only(&db_path)
            .map_err(|detail| ReadError::Other(format!("Hermes state database error at \"{db_path}\": {detail}")))?;
        db.connection().execute_batch("BEGIN DEFERRED")?;
        let result = validate_schema(&db, &db_path).and_then(|columns| f(&db, &columns));
        let _ = db
            .connection()
            .execute_batch(if result.is_ok() { "COMMIT" } else { "ROLLBACK" });
        result
    }
}

fn table_columns(db: &ReadableDb, table: &str) -> Result<HashSet<String>, ReadError> {
    let rows = db.all("SELECT name FROM pragma_table_info(?)", &[&table])?;
    Ok(rows.iter().filter_map(|r| r.get("name").text()).collect())
}

fn validate_schema(db: &ReadableDb, db_path: &str) -> Result<Columns, ReadError> {
    let columns = Columns {
        sessions: table_columns(db, "sessions")?,
        messages: table_columns(db, "messages")?,
    };
    let mut missing = Vec::new();
    for (table, have, required) in [
        ("sessions", &columns.sessions, &REQUIRED_SESSIONS[..]),
        ("messages", &columns.messages, &REQUIRED_MESSAGES[..]),
    ] {
        if have.is_empty() {
            missing.push(format!("table {table}"));
            continue;
        }
        for column in required {
            if !have.contains(*column) {
                missing.push(format!("{table}.{column}"));
            }
        }
    }
    if !missing.is_empty() {
        return Err(ReadError::Other(format!(
            "Hermes schema compatibility error at \"{db_path}\": missing {}",
            missing.join(", ")
        )));
    }
    Ok(columns)
}

fn optional_column(columns: &HashSet<String>, name: &str, fallback: &str) -> String {
    if columns.contains(name) {
        name.to_string()
    } else {
        format!("{fallback} AS {name}")
    }
}

fn read_session_rows(db: &ReadableDb, columns: &HashSet<String>) -> Result<Vec<SessionRow>, ReadError> {
    let sql = format!(
        "SELECT id,
              source,
              {},
              parent_session_id,
              {},
              {}
         FROM sessions
        ORDER BY {} id",
        optional_column(columns, "model", "NULL"),
        optional_column(columns, "end_reason", "NULL"),
        optional_column(columns, "model_config", "NULL"),
        if columns.contains("started_at") {
            "started_at,"
        } else {
            ""
        }
    );
    Ok(db
        .all(&sql, &[])?
        .iter()
        .map(|r| SessionRow {
            id: r.get("id").text().unwrap_or_else(|| "null".to_string()),
            source: r.get("source").text(),
            model: r.get("model").clone(),
            parent_session_id: r.get("parent_session_id").text(),
            end_reason: r.get("end_reason").text(),
            model_config: r.get("model_config").text(),
        })
        .collect())
}

fn message_select(columns: &HashSet<String>) -> String {
    format!(
        "SELECT id,
                 session_id,
                 role,
                 content,
                 tool_call_id,
                 tool_calls,
                 tool_name,
                 timestamp,
                 {},
                 {}
            FROM messages",
        optional_column(columns, "active", "1"),
        optional_column(columns, "compacted", "0")
    )
}

fn message_row(r: &Row) -> MessageRow {
    MessageRow {
        id: r.get("id").clone(),
        session_id: r.get("session_id").text().unwrap_or_else(|| "null".to_string()),
        role: r.get("role").text(),
        content: r.get("content").text(),
        tool_call_id: r.get("tool_call_id").clone(),
        tool_calls: r.get("tool_calls").text(),
        tool_name: r.get("tool_name").clone(),
        timestamp: r.get("timestamp").clone(),
        active: r.get("active").clone(),
        compacted: r.get("compacted").clone(),
    }
}

fn read_marker_messages(db: &ReadableDb, columns: &HashSet<String>) -> Result<Vec<MessageRow>, ReadError> {
    let sql = format!(
        "{}
        WHERE role = 'assistant'
          AND tool_calls IS NOT NULL
          AND instr(tool_calls, ?) > 0
        ORDER BY id",
        message_select(columns)
    );
    Ok(db.all(&sql, &[&MARKER_ANCHOR])?.iter().map(message_row).collect())
}

fn read_bound_messages<'a>(
    db: &ReadableDb,
    columns: &HashSet<String>,
    session_ids: impl Iterator<Item = &'a String>,
    skipped: &mut Vec<SkippedRow>,
) -> Result<Vec<ParsedMessage>, ReadError> {
    let sql = format!(
        "{}
      WHERE session_id = ?
      ORDER BY id",
        message_select(columns)
    );
    let mut messages = Vec::new();
    for session_id in session_ids {
        for r in db.all(&sql, &[session_id])? {
            let row = message_row(&r);
            let tool_calls = parse_tool_calls(&row, Some(skipped));
            messages.push(ParsedMessage { row, tool_calls });
        }
    }
    Ok(messages)
}

/// Latest message timestamp (epoch seconds) per session id.
fn read_latest_message_stamps(db: &ReadableDb) -> Result<HashMap<String, f64>, ReadError> {
    let rows = db.all(
        "SELECT session_id, MAX(timestamp) AS latest FROM messages GROUP BY session_id",
        &[],
    )?;
    Ok(rows
        .iter()
        .filter_map(|r| Some((r.get("session_id").text()?, js_number(r.get("latest")))))
        .collect())
}

/// `Number(value)` of a driver value.
fn js_number(cell: &Cell) -> f64 {
    match cell {
        Cell::Null => 0.0,
        Cell::Integer(i) => *i as f64,
        Cell::Real(f) => *f,
        Cell::Text(s) => {
            let t = js_trim(s);
            if t.is_empty() {
                0.0
            } else {
                t.parse().unwrap_or(f64::NAN)
            }
        }
        Cell::Blob(_) => f64::NAN,
    }
}

fn iso_from_seconds(seconds: f64) -> Result<String, ReadError> {
    date::to_iso_string(seconds * 1000.0).ok_or_else(|| ReadError::Other("Invalid time value".to_string()))
}

fn group_sessions(sessions: &[SessionRow], latest: &HashMap<String, f64>) -> Result<Vec<SessionGroup>, ReadError> {
    let by_id: HashMap<&str, &SessionRow> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut members_by_root: OrderedMap<String, Vec<SessionRow>> = OrderedMap::new();
    for session in sessions {
        members_by_root
            .entry_or_insert_with(lineage_root_id(session, &by_id), Vec::new)
            .push(session.clone());
    }
    let mut groups = Vec::new();
    for (conversation_id, members) in members_by_root.into_entries() {
        let stamps: Vec<f64> = members.iter().filter_map(|m| latest.get(&m.id).copied()).collect();
        let last_modified = if stamps.is_empty() {
            None
        } else {
            Some(iso_from_seconds(
                stamps.iter().copied().fold(f64::NEG_INFINITY, js_max),
            )?)
        };
        groups.push(SessionGroup {
            info: SessionInfo {
                conversation_id,
                last_modified,
            },
            members,
        });
    }
    stable_sort_by(&mut groups, |a, b| {
        let (left, right) = (&a.info.last_modified, &b.info.last_modified);
        if left != right {
            return match (left, right) {
                (None, _) => std::cmp::Ordering::Greater,
                (_, None) => std::cmp::Ordering::Less,
                (Some(l), Some(r)) => js_cmp(l, r),
            };
        }
        js_cmp(&a.info.conversation_id, &b.info.conversation_id)
    });
    Ok(groups)
}

/// `Math.max` (NaN wins).
fn js_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() { f64::NAN } else { a.max(b) }
}

/// Walk up the workflow parents (the marker-mode lineage rule) to the topmost known row.
fn lineage_root_id(session: &SessionRow, by_id: &HashMap<&str, &SessionRow>) -> String {
    let mut visited: HashSet<&str> = [session.id.as_str()].into();
    let mut current = session;
    loop {
        let parent = workflow_parent_id(current, by_id).and_then(|id| by_id.get(id.as_str()).copied());
        match parent {
            Some(parent) if !visited.contains(parent.id.as_str()) => {
                visited.insert(parent.id.as_str());
                current = parent;
            }
            _ => return current.id.clone(),
        }
    }
}

fn gather_group(
    db: &ReadableDb,
    columns: &HashSet<String>,
    group: &SessionGroup,
    by_id: &HashMap<&str, &SessionRow>,
) -> Result<SessionGather, ReadError> {
    let mut member_ids: Vec<String> = Vec::new();
    for member in &group.members {
        if !member_ids.contains(&member.id) {
            member_ids.push(member.id.clone());
        }
    }
    let bound: HashSet<String> = member_ids.iter().cloned().collect();
    let mut skipped = Vec::new();
    let messages = read_bound_messages(db, columns, member_ids.iter(), &mut skipped)?;
    let mut drafts = Vec::new();
    for message in &messages {
        for mut draft in drafts_from_message(message, &bound, by_id)? {
            draft.conversation_id = group.info.conversation_id.clone();
            drafts.push(draft);
        }
    }
    Ok(SessionGather {
        drafts,
        skipped,
        last_modified: group.info.last_modified.clone(),
    })
}

fn parse_tool_calls(row: &MessageRow, skipped: Option<&mut Vec<SkippedRow>>) -> Vec<ToolCall> {
    let Some(raw) = &row.tool_calls else { return vec![] };
    let mut skipped = skipped;
    let mut record = |error: String, index: Option<usize>| {
        if let Some(skipped) = skipped.as_deref_mut() {
            skipped.push(SkippedRow::new(
                "messages",
                format!("invalid tool_calls JSON: {error}"),
                format!(
                    "message id={} session={}{}",
                    super::readers::cell_display(&row.id),
                    row.session_id,
                    index.map(|i| format!(" call={i}")).unwrap_or_default()
                ),
            ));
        }
    };
    let values = match json::parse(raw) {
        Ok(Value::Array(values)) => values,
        Ok(_) => {
            record("expected an array".to_string(), None);
            return vec![];
        }
        Err(error) => {
            record(error, None);
            return vec![];
        }
    };
    let mut calls = Vec::new();
    for (index, value) in values.iter().enumerate() {
        match parse_tool_call(value) {
            Ok(call) => calls.push(call),
            Err(error) => record(error, Some(index)),
        }
    }
    calls
}

fn parse_tool_call(value: &Value) -> Result<ToolCall, String> {
    let Some(call) = value.as_object() else {
        return Err("tool call is missing function data".to_string());
    };
    let Some(function) = call.get("function").and_then(Value::as_object) else {
        return Err("tool call is missing function data".to_string());
    };
    let id = call.get_str("id").or_else(|| call.get_str("call_id"));
    let name = function.get_str("name");
    let (Some(id), Some(name)) = (id, name) else {
        return Err("tool call is missing an id or function name".to_string());
    };
    let arguments = match function.get("arguments") {
        Some(Value::String(raw)) => json::parse(raw)?,
        Some(other) => other.clone(),
        None => Value::Null,
    };
    let Value::Object(arguments) = arguments else {
        return Err("tool call function arguments are not an object".to_string());
    };
    Ok(ToolCall {
        id: id.to_string(),
        name: name.to_string(),
        arguments,
    })
}

fn bind_sessions(
    sessions: &[SessionRow],
    marker_messages: &[MessageRow],
    capture_id: &str,
    skipped: &mut Vec<SkippedRow>,
) -> OrderedMap<String, Label> {
    let mut markers_by_session: OrderedMap<String, Vec<ExactMarker>> = OrderedMap::new();
    for row in marker_messages {
        for call in parse_tool_calls(row, None) {
            let markers = markers_from_tool_call(&call);
            if markers.is_empty() {
                continue;
            }
            markers_by_session
                .entry_or_insert_with(row.session_id.clone(), Vec::new)
                .extend(markers);
        }
    }

    let mut explicit_labels: HashMap<String, Label> = HashMap::new();
    let mut barriers: HashSet<String> = HashSet::new();
    for (session_id, markers) in markers_by_session.iter() {
        let mut target_labels: Vec<&str> = Vec::new();
        for marker in markers.iter().filter(|m| m.capture_id == capture_id) {
            if !target_labels.contains(&marker.label.as_str()) {
                target_labels.push(&marker.label);
            }
        }
        match target_labels.len() {
            1 => {
                explicit_labels.insert(session_id.clone(), target_labels[0].to_string());
            }
            0 => {
                barriers.insert(session_id.clone());
            }
            _ => {
                barriers.insert(session_id.clone());
                skipped.push(SkippedRow::new(
                    "messages",
                    "same session has markers for multiple labels",
                    format!("{session_id}: {}", target_labels.join(", ")),
                ));
            }
        }
    }

    let by_id: HashMap<&str, &SessionRow> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
    let mut label_by_cid = OrderedMap::new();
    for session in sessions {
        if let Some(label) = nearest_marked_label(session, &by_id, &explicit_labels, &barriers) {
            label_by_cid.insert(session.id.clone(), label);
        }
    }
    label_by_cid
}

fn nearest_marked_label(
    session: &SessionRow,
    by_id: &HashMap<&str, &SessionRow>,
    explicit_labels: &HashMap<String, Label>,
    barriers: &HashSet<String>,
) -> Option<Label> {
    let mut visited: HashSet<&str> = HashSet::new();
    let mut current = Some(session);
    while let Some(row) = current {
        if !visited.insert(row.id.as_str()) {
            break;
        }
        if let Some(label) = explicit_labels.get(&row.id) {
            return Some(label.clone());
        }
        if barriers.contains(&row.id) {
            return None;
        }
        current = workflow_parent_id(row, by_id).and_then(|id| by_id.get(id.as_str()).copied());
    }
    None
}

fn workflow_parent_id(session: &SessionRow, by_id: &HashMap<&str, &SessionRow>) -> Option<String> {
    let parent_id = session.parent_session_id.as_ref()?;
    if session.source.as_deref() == Some("tool") {
        return None;
    }
    let config = parse_model_config(session.model_config.as_deref());
    let has = |key: &str| config.get(key).is_some_and(|v| !v.is_null());
    if has("_branched_from") {
        return None;
    }
    if has("_delegate_from") || session.source.as_deref() == Some("subagent") {
        return Some(parent_id.clone());
    }
    let parent = by_id.get(parent_id.as_str())?;
    (parent.end_reason.as_deref() == Some("compression")).then(|| parent_id.clone())
}

fn parse_model_config(raw: Option<&str>) -> Object {
    match raw.map(json::parse) {
        Some(Ok(Value::Object(o))) => o,
        _ => Object::new(),
    }
}

fn markers_from_tool_call(call: &ToolCall) -> Vec<ExactMarker> {
    if !MARKER_TOOLS.contains(&call.name.as_str()) {
        return vec![];
    }
    call.arguments
        .get_str("command")
        .map(exact_markers_in_command)
        .unwrap_or_default()
}

fn drafts_from_message(
    message: &ParsedMessage,
    bound: &HashSet<String>,
    by_id: &HashMap<&str, &SessionRow>,
) -> Result<Vec<EventDraft>, ReadError> {
    let row = &message.row;
    if !bound.contains(&row.session_id) {
        return Ok(vec![]);
    }
    let Some(session) = by_id.get(row.session_id.as_str()) else {
        return Ok(vec![]);
    };

    let ts = iso_from_seconds(js_number(&row.timestamp))?;
    let mut base_meta = Object::new();
    for (key, value) in [
        ("message_id", cell_value(&row.id)),
        ("active", cell_value(&row.active)),
        ("compacted", cell_value(&row.compacted)),
        (
            "session_source",
            session.source.clone().map_or(Value::Null, Value::from),
        ),
        ("model", cell_value(&session.model)),
    ] {
        if !value.is_null() {
            base_meta.insert(key, value);
        }
    }
    let draft = |role, kind, content: String, meta: &Object, index: usize| {
        let mut meta = meta.clone();
        meta.insert("message_event_index", index);
        EventDraft {
            conversation_id: row.session_id.clone(),
            ts: ts.clone(),
            role,
            kind,
            content,
            meta,
        }
    };

    match row.role.as_deref() {
        Some("user") => Ok(row
            .content
            .iter()
            .map(|c| draft(Role::User, EventKind::Query, c.clone(), &base_meta, 0))
            .collect()),
        Some("tool") => {
            let mut meta = base_meta.clone();
            for (key, value) in [
                ("tool", cell_value(&row.tool_name)),
                ("tool_call_id", cell_value(&row.tool_call_id)),
            ] {
                if !value.is_null() {
                    meta.insert(key, value);
                }
            }
            Ok(row
                .content
                .iter()
                .map(|c| draft(Role::Tool, EventKind::ToolResult, c.clone(), &meta, 0))
                .collect())
        }
        Some("assistant") => {
            let mut drafts = Vec::new();
            if let Some(content) = row.content.as_ref().filter(|c| !c.is_empty()) {
                drafts.push(draft(
                    Role::Assistant,
                    EventKind::AgentMessage,
                    content.clone(),
                    &base_meta,
                    drafts.len(),
                ));
            }
            for call in &message.tool_calls {
                let mut meta = base_meta.clone();
                meta.insert("tool", call.name.as_str());
                meta.insert("tool_call_id", call.id.as_str());
                meta.insert("input", call.arguments.clone());
                drafts.push(draft(
                    Role::Assistant,
                    EventKind::ToolCall,
                    tool_call_content(call),
                    &meta,
                    drafts.len(),
                ));
            }
            Ok(drafts)
        }
        _ => Ok(vec![]),
    }
}

fn tool_call_content(call: &ToolCall) -> String {
    if MARKER_TOOLS.contains(&call.name.as_str())
        && let Some(command) = call.arguments.get_str("command")
    {
        return command.to_string();
    }
    call.arguments.to_json()
}

impl ConversationReader for HermesConversationReader {
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError> {
        self.with_snapshot(|db, columns| {
            let sessions = read_session_rows(db, &columns.sessions)?;
            let marker_messages = read_marker_messages(db, &columns.messages)?;
            let mut skipped = Vec::new();
            let label_by_cid = bind_sessions(&sessions, &marker_messages, capture_id, &mut skipped);
            let messages = read_bound_messages(db, &columns.messages, label_by_cid.keys(), &mut skipped)?;
            let by_id: HashMap<&str, &SessionRow> = sessions.iter().map(|s| (s.id.as_str(), s)).collect();
            let bound: HashSet<String> = label_by_cid.keys().cloned().collect();
            let mut drafts = Vec::new();
            for message in &messages {
                drafts.extend(drafts_from_message(message, &bound, &by_id)?);
            }
            Ok(CaptureRead {
                source: BundleSource::Hermes,
                label_by_cid,
                drafts,
                skipped,
                unbindable: vec![],
                collisions: vec![],
                heuristic_bindings: vec![],
            })
        })
    }
}

impl SessionReader for HermesConversationReader {
    /// One session per lineage root; `last_modified` is the latest message
    /// timestamp of the group (ISO-8601 UTC); every event carries the root's id.
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError> {
        self.with_snapshot(|db, columns| {
            let rows = read_session_rows(db, &columns.sessions)?;
            let mut selected = group_sessions(&rows, &read_latest_message_stamps(db)?)?;
            if let Some(conversation) = &options.conversation {
                selected.retain(|g| &g.info.conversation_id == conversation);
            }
            if let Some(since) = &options.since
                && let Some(cutoff) = parse_since_utc(since)
            {
                // A group without messages has no stamp: included, never hidden.
                selected.retain(|g| g.info.last_modified.as_ref().is_none_or(|s| date::parse(s) >= cutoff));
            }
            let by_id: HashMap<&str, &SessionRow> = rows.iter().map(|s| (s.id.as_str(), s)).collect();
            let mut reads = OrderedMap::new();
            for group in &selected {
                if !should_gather(&group.info, options) {
                    continue;
                }
                reads.insert(
                    group.info.conversation_id.clone(),
                    gather_group(db, &columns.messages, group, &by_id)?,
                );
            }
            Ok(SessionsRead {
                source: BundleSource::Hermes,
                sessions: selected.into_iter().map(|g| g.info).collect(),
                reads,
            })
        })
    }
}

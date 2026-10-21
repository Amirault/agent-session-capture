//! Hermes `state.db` fixtures (the subset of Hermes' schema the reader requires).
//! Also included by the Hermes CLI integration tests via `#[path]`.

#![allow(dead_code)]

use rusqlite::{Connection, params};

/// The capture id every Hermes marker test uses.
pub const SPEC: &str = "2026-07-15-hermes-session-capture-adapter";

const SESSIONS_TABLE: &str = "
CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  source TEXT NOT NULL,
  model TEXT,
  parent_session_id TEXT,
  started_at REAL NOT NULL,
  ended_at REAL,
  end_reason TEXT,
  model_config TEXT NOT NULL DEFAULT '{}'
);
";

const MESSAGES_TABLE_WITH_REASONING: &str = "
CREATE TABLE messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT,
  reasoning_content TEXT,
  tool_calls TEXT,
  tool_call_id TEXT,
  tool_name TEXT,
  timestamp REAL NOT NULL,
  active INTEGER NOT NULL DEFAULT 1,
  compacted INTEGER NOT NULL DEFAULT 0
);
";

const MESSAGES_TABLE: &str = "
CREATE TABLE messages (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  session_id TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT,
  tool_calls TEXT,
  tool_call_id TEXT,
  tool_name TEXT,
  timestamp REAL NOT NULL,
  active INTEGER NOT NULL DEFAULT 1,
  compacted INTEGER NOT NULL DEFAULT 0
);
";

/// The marker-mode fixture DB (with `messages.reasoning_content`), in `journal_mode`.
pub fn create_hermes_db(db_path: &str, journal_mode: &str) -> Connection {
    let db = Connection::open(db_path).expect("open Hermes fixture");
    let _: String = db
        .query_row(&format!("PRAGMA journal_mode = {journal_mode}"), [], |row| row.get(0))
        .expect("journal mode");
    db.execute_batch(&format!("{SESSIONS_TABLE}{MESSAGES_TABLE_WITH_REASONING}"))
        .expect("Hermes schema");
    db
}

/// The session-mode fixture DB (no `messages.reasoning_content`).
pub fn create_session_hermes_db(db_path: &str) -> Connection {
    let db = Connection::open(db_path).expect("open Hermes fixture");
    db.execute_batch(&format!("{SESSIONS_TABLE}{MESSAGES_TABLE}"))
        .expect("Hermes schema");
    db
}

/// Options of [`seed_session`]; `None` fields take the original defaults.
#[derive(Default)]
pub struct SessionOptions<'a> {
    pub parent: Option<&'a str>,
    pub source: Option<&'a str>,
    pub end_reason: Option<&'a str>,
    /// `model_config` JSON text (default `{}`).
    pub model_config: Option<&'a str>,
    pub started_at: Option<f64>,
}

pub fn seed_session(db: &Connection, id: &str, options: SessionOptions<'_>) {
    db.execute(
        "INSERT INTO sessions (
           id, source, model, parent_session_id, started_at, ended_at, end_reason, model_config
         ) VALUES (?, ?, 'gpt-5.6-sol', ?, ?, NULL, ?, ?)",
        params![
            id,
            options.source.unwrap_or("cli"),
            options.parent,
            options.started_at.unwrap_or(1_784_000_000.0),
            options.end_reason,
            options.model_config.unwrap_or("{}")
        ],
    )
    .expect("insert session");
}

/// Options of [`insert_message`]; `tool_calls` is the stored JSON text.
pub struct MessageOptions<'a> {
    pub session_id: &'a str,
    pub role: &'a str,
    pub timestamp: f64,
    pub content: Option<&'a str>,
    pub tool_calls: Option<&'a str>,
    pub tool_call_id: Option<&'a str>,
    pub tool_name: Option<&'a str>,
    pub active: i64,
    pub compacted: i64,
}

impl<'a> MessageOptions<'a> {
    pub fn new(session_id: &'a str, role: &'a str, timestamp: f64) -> Self {
        Self {
            session_id,
            role,
            timestamp,
            content: None,
            tool_calls: None,
            tool_call_id: None,
            tool_name: None,
            active: 1,
            compacted: 0,
        }
    }
}

pub fn insert_message(db: &Connection, options: MessageOptions<'_>) {
    db.execute(
        "INSERT INTO messages (
           session_id, role, content, reasoning_content, tool_calls,
           tool_call_id, tool_name, timestamp, active, compacted
         ) VALUES (?, ?, ?, NULL, ?, ?, ?, ?, ?, ?)",
        params![
            options.session_id,
            options.role,
            options.content,
            options.tool_calls,
            options.tool_call_id,
            options.tool_name,
            options.timestamp,
            options.active,
            options.compacted
        ],
    )
    .expect("insert message");
}

/// JSON string literal (as `JSON.stringify` writes it).
pub fn quote(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `{"command": command}` as JSON text.
pub fn command_args(command: &str) -> String {
    format!("{{\"command\":{}}}", quote(command))
}

/// One OpenAI-style tool call as JSON text; `args_json` is the stringified arguments.
pub fn tool_call(id: &str, name: &str, args_json: &str) -> String {
    format!(
        "{{\"id\":{},\"type\":\"function\",\"function\":{{\"name\":{},\"arguments\":{}}}}}",
        quote(id),
        quote(name),
        quote(args_json)
    )
}

/// A JSON array of already-encoded calls.
pub fn calls(items: &[String]) -> String {
    format!("[{}]", items.join(","))
}

pub fn marker(label: &str) -> String {
    format!(": CAPTURE_MARKER v=1 id={SPEC} label={label}")
}

pub fn seed_tool_calls(db: &Connection, session_id: &str, timestamp: f64, calls_json: &str, content: Option<&str>) {
    insert_message(
        db,
        MessageOptions {
            content,
            tool_calls: Some(calls_json),
            ..MessageOptions::new(session_id, "assistant", timestamp)
        },
    );
}

pub fn seed_marker_named(db: &Connection, session_id: &str, label: &str, timestamp: f64, name: &str) {
    let call = tool_call(&format!("{session_id}-{label}"), name, &command_args(&marker(label)));
    seed_tool_calls(db, session_id, timestamp, &calls(&[call]), None);
}

pub fn seed_marker(db: &Connection, session_id: &str, label: &str, timestamp: f64) {
    seed_marker_named(db, session_id, label, timestamp, "terminal");
}

pub fn seed_text(db: &Connection, session_id: &str, content: &str, timestamp: f64) {
    insert_message(
        db,
        MessageOptions {
            content: Some(content),
            ..MessageOptions::new(session_id, "user", timestamp)
        },
    );
}

// --- Session mode (`--sessions`) ---------------------------------------------

/// Epoch seconds (Hermes' `timestamp`/`started_at` scale) of an ISO-8601 UTC instant.
pub fn at(iso: &str) -> f64 {
    crate::js::date::parse(iso) / 1000.0
}

/// One session-mode session row.
#[derive(Default)]
pub struct SessionRow<'a> {
    pub id: &'a str,
    pub parent: Option<&'a str>,
    pub source: Option<&'a str>,
    pub end_reason: Option<&'a str>,
    /// `model_config` JSON text (default `{}`).
    pub model_config: Option<&'a str>,
}

pub fn seed_session_row(db: &Connection, row: SessionRow<'_>, started_at: &str) {
    db.execute(
        "INSERT INTO sessions (id, source, model, parent_session_id, started_at, end_reason, model_config)
         VALUES (?, ?, 'gpt-5.6-sol', ?, ?, ?, ?)",
        params![
            row.id,
            row.source.unwrap_or("cli"),
            row.parent,
            at(started_at),
            row.end_reason,
            row.model_config.unwrap_or("{}")
        ],
    )
    .expect("insert session");
}

/// One user message: maps to exactly one `query` event whose content is `content`.
pub fn seed_user_message(db: &Connection, session_id: &str, content: &str, timestamp: &str) {
    db.execute(
        "INSERT INTO messages (session_id, role, content, timestamp) VALUES (?, 'user', ?, ?)",
        params![session_id, content, at(timestamp)],
    )
    .expect("insert message");
}

/// Example 3: root `20261015_101500_ab12` (ended by compression), its
/// compression continuation `20261015_113000_cd34`, and a `/branch` child
/// `20261015_120000_ef56`.
pub const ROOT_ID: &str = "20261015_101500_ab12";
pub const CONTINUATION_ID: &str = "20261015_113000_cd34";
pub const BRANCH_ID: &str = "20261015_120000_ef56";

pub fn seed_example3(db_path: &str) {
    let db = create_session_hermes_db(db_path);
    seed_session_row(
        &db,
        SessionRow {
            id: ROOT_ID,
            end_reason: Some("compression"),
            ..Default::default()
        },
        "2026-10-15T10:15:00Z",
    );
    seed_user_message(&db, ROOT_ID, "root question", "2026-10-15T10:15:01Z");
    seed_user_message(&db, ROOT_ID, "root follow-up", "2026-10-15T10:20:00Z");
    seed_session_row(
        &db,
        SessionRow {
            id: CONTINUATION_ID,
            parent: Some(ROOT_ID),
            ..Default::default()
        },
        "2026-10-15T11:30:00Z",
    );
    seed_user_message(
        &db,
        CONTINUATION_ID,
        "continued after compression",
        "2026-10-15T11:30:01Z",
    );
    seed_user_message(&db, CONTINUATION_ID, "continuation follow-up", "2026-10-15T11:45:00Z");
    let branch_config = format!("{{\"_branched_from\":{}}}", quote(ROOT_ID));
    seed_session_row(
        &db,
        SessionRow {
            id: BRANCH_ID,
            parent: Some(ROOT_ID),
            model_config: Some(&branch_config),
            ..Default::default()
        },
        "2026-10-15T12:00:00Z",
    );
    seed_user_message(&db, BRANCH_ID, "explore an alternative", "2026-10-15T12:00:01Z");
}

/// Parse every line of a JSONL file.
pub fn read_jsonl(file_path: &str) -> Vec<crate::js::Object> {
    let text = std::fs::read_to_string(file_path).expect("read JSONL");
    text.trim_end()
        .split('\n')
        .map(|line| match crate::js::json::parse(line).expect("JSON line") {
            crate::js::Value::Object(object) => object,
            other => panic!("not an object: {other:?}"),
        })
        .collect()
}

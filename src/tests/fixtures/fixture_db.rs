//! Test fixture mirroring the verified Warp schema (captured from the live DB).
//! Lets every test seed controlled rows without touching the real Warp DB.
//! Also included by the CLI integration tests (`tests/common`).

#![allow(dead_code)]

use rusqlite::{Connection, params};

const SCHEMA: &str = "
CREATE TABLE ai_queries (
  id INTEGER PRIMARY KEY NOT NULL,
  exchange_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  start_ts DATETIME NOT NULL,
  input TEXT NOT NULL,
  working_directory TEXT,
  output_status TEXT NOT NULL,
  model_id TEXT NOT NULL DEFAULT '',
  planning_model_id TEXT NOT NULL DEFAULT '',
  coding_model_id TEXT NOT NULL DEFAULT ''
);
CREATE TABLE agent_tasks (
  id INTEGER PRIMARY KEY NOT NULL,
  conversation_id TEXT NOT NULL,
  task_id TEXT NOT NULL,
  task BLOB NOT NULL,
  last_modified_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE blocks (
  id INTEGER PRIMARY KEY,
  pane_leaf_uuid BLOB,
  stylized_command BLOB,
  stylized_output BLOB,
  pwd TEXT,
  git_branch TEXT,
  virtual_env TEXT,
  conda_env TEXT,
  exit_code INTEGER NOT NULL,
  did_execute BOOLEAN NOT NULL,
  completed_ts DATETIME,
  start_ts DATETIME,
  ps1 TEXT,
  honor_ps1 BOOLEAN NOT NULL DEFAULT FALSE,
  shell TEXT,
  user TEXT,
  host TEXT,
  is_background BOOLEAN NOT NULL DEFAULT false,
  rprompt TEXT,
  prompt_snapshot TEXT,
  block_id TEXT NOT NULL DEFAULT \"\",
  ai_metadata TEXT,
  is_local BOOLEAN,
  agent_view_visibility TEXT,
  git_branch_name TEXT
);
CREATE TABLE commands (
  id INTEGER PRIMARY KEY NOT NULL,
  command TEXT NOT NULL,
  exit_code INTEGER,
  start_ts DATETIME,
  completed_ts DATETIME,
  pwd TEXT,
  shell TEXT,
  username TEXT,
  hostname TEXT,
  session_id BIGINTEGER,
  git_branch TEXT,
  cloud_workflow_id TEXT,
  workflow_command TEXT,
  is_agent_executed BOOLEAN
);
CREATE TABLE agent_conversations (
  id INTEGER PRIMARY KEY NOT NULL,
  conversation_id TEXT NOT NULL,
  conversation_data TEXT NOT NULL,
  last_modified_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE TABLE terminal_panes (
  id INTEGER PRIMARY KEY NOT NULL,
  kind TEXT NOT NULL DEFAULT 'terminal',
  uuid BLOB NOT NULL UNIQUE,
  cwd TEXT,
  is_active BOOLEAN NOT NULL DEFAULT FALSE,
  shell_launch_data TEXT,
  input_config TEXT,
  llm_model_override TEXT,
  active_profile_id TEXT,
  conversation_ids TEXT,
  active_conversation_id TEXT
);
";

/// Create a fixture Warp DB at `path` (`wal` sets WAL journaling).
pub fn create_fixture(path: &str, wal: bool) -> Connection {
    let db = Connection::open(path).expect("open fixture");
    if wal {
        db.pragma_update(None, "journal_mode", "WAL").expect("wal");
    }
    db.execute_batch(SCHEMA).expect("schema");
    db
}

/// Mimic Warp's per-character bold styling so ANSI stripping is exercised.
pub fn ansi_wrap(text: &str) -> Vec<u8> {
    text.chars().map(|c| format!("\x1b[1m{c}\x1b[0m")).collect::<String>().into_bytes()
}

/// Options of [`seed_marker`].
pub struct MarkerSeed<'a> {
    pub capture_id: &'a str,
    pub label: &'a str,
    pub conversation_id: &'a str,
    pub start_ts: &'a str,
}

/// Seed a marker: a clean no-op in commands + a binding block at the same start_ts.
pub fn seed_marker(db: &Connection, seed: MarkerSeed<'_>) {
    let command = format!(": CAPTURE_MARKER v=1 id={} label={}", seed.capture_id, seed.label);
    db.execute(
        "INSERT INTO commands (command, start_ts, is_agent_executed, session_id, pwd, git_branch)
         VALUES (?, ?, 1, 1, '/repo', 'main')",
        params![command, seed.start_ts],
    )
    .expect("insert command");
    let metadata = format!("{{\"conversation_id\":\"{}\",\"subagent_task_id\":null}}", seed.conversation_id);
    db.execute(
        "INSERT INTO blocks (pane_leaf_uuid, stylized_command, stylized_output, pwd, git_branch, exit_code, did_execute, start_ts, completed_ts, ai_metadata, git_branch_name)
         VALUES (?, ?, ?, '/repo', 'main', 0, 1, ?, ?, ?, 'main')",
        params![vec![0u8, 0, 0, 0], ansi_wrap(&command), Vec::<u8>::new(), seed.start_ts, seed.start_ts, metadata],
    )
    .expect("insert block");
}

/// Seed a marker command with NO binding block — the "unbindable" case.
pub fn seed_unbindable_marker(db: &Connection, capture_id: &str, label: &str, start_ts: &str) {
    let command = format!(": CAPTURE_MARKER v=1 id={capture_id} label={label}");
    db.execute(
        "INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)",
        params![command, start_ts],
    )
    .expect("insert command");
}

/// Options of [`seed_query`]; `None` fields take the original defaults.
#[derive(Default)]
pub struct QuerySeed<'a> {
    pub conversation_id: &'a str,
    pub start_ts: &'a str,
    pub text: &'a str,
    pub working_directory: Option<&'a str>,
    pub model_id: Option<&'a str>,
    pub exchange_id: Option<&'a str>,
}

pub fn seed_query(db: &Connection, seed: QuerySeed<'_>) {
    let input = format!("[{{\"Query\":{{\"text\":{}}}}}]", crate_quote(seed.text));
    let exchange = seed.exchange_id.map(str::to_string).unwrap_or_else(|| format!("{}-ex", seed.conversation_id));
    db.execute(
        "INSERT INTO ai_queries (exchange_id, conversation_id, start_ts, input, working_directory, output_status, model_id)
         VALUES (?, ?, ?, ?, ?, '', ?)",
        params![
            exchange,
            seed.conversation_id,
            seed.start_ts,
            input,
            seed.working_directory.unwrap_or("/repo"),
            seed.model_id.unwrap_or("claude-test")
        ],
    )
    .expect("insert query");
}

/// JSON string literal (as `JSON.stringify` writes it).
fn crate_quote(s: &str) -> String {
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

/// Options of [`seed_block`]; `None` fields take the original defaults.
#[derive(Default)]
pub struct BlockSeed<'a> {
    pub conversation_id: &'a str,
    pub start_ts: &'a str,
    pub completed_ts: Option<&'a str>,
    pub command: Option<&'a str>,
    pub output: Option<&'a str>,
    /// `Some(false)` stores plain text instead of ANSI-exploded text.
    pub ansi: Option<bool>,
    pub pwd: Option<&'a str>,
    pub git_branch: Option<&'a str>,
    pub exit_code: Option<i64>,
    pub did_execute: Option<i64>,
    pub subagent_task_id: Option<&'a str>,
}

pub fn seed_block(db: &Connection, seed: BlockSeed<'_>) {
    let command = seed.command.unwrap_or("echo hi");
    let output = seed.output.unwrap_or("");
    let plain = seed.ansi == Some(false);
    let styled = |t: &str| if plain { t.as_bytes().to_vec() } else { ansi_wrap(t) };
    let subagent = seed.subagent_task_id.map_or("null".to_string(), crate_quote);
    let metadata = format!("{{\"conversation_id\":{},\"subagent_task_id\":{subagent}}}", crate_quote(seed.conversation_id));
    let branch = seed.git_branch.unwrap_or("main");
    db.execute(
        "INSERT INTO blocks (pane_leaf_uuid, stylized_command, stylized_output, pwd, git_branch, exit_code, did_execute, start_ts, completed_ts, ai_metadata, git_branch_name)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            vec![0u8, 0, 0, 0],
            styled(command),
            styled(output),
            seed.pwd.unwrap_or("/repo"),
            branch,
            seed.exit_code.unwrap_or(0),
            seed.did_execute.unwrap_or(1),
            seed.start_ts,
            seed.completed_ts.unwrap_or(seed.start_ts),
            metadata,
            branch
        ],
    )
    .expect("insert block");
}

/// Seed an agent_conversations row — the durable per-conversation stamp.
pub fn seed_conversation(db: &Connection, conversation_id: &str, last_modified_at: Option<&str>) {
    db.execute(
        "INSERT INTO agent_conversations (conversation_id, conversation_data, last_modified_at) VALUES (?, '{}', ?)",
        params![conversation_id, last_modified_at.unwrap_or("2026-06-30 12:00:00.000000")],
    )
    .expect("insert conversation");
}

/// Seed an agent_tasks row; `None` fields take the original defaults.
pub fn seed_task(db: &Connection, conversation_id: &str, task: &[u8], task_id: Option<&str>, last_modified_at: Option<&str>) {
    let task_id = task_id.map(str::to_string).unwrap_or_else(|| format!("{conversation_id}-task"));
    db.execute(
        "INSERT INTO agent_tasks (conversation_id, task_id, task, last_modified_at) VALUES (?, ?, ?, ?)",
        params![conversation_id, task_id, task, last_modified_at.unwrap_or("2026-06-30 12:00:00.000000")],
    )
    .expect("insert task");
}

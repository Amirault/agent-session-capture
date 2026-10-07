import Database from "better-sqlite3";
import type { Label } from "../../domain/models.js";

/**
 * Test fixture mirroring the verified Warp schema (captured from the live DB).
 * Lets every test seed controlled rows without touching the real Warp DB.
 */

const SCHEMA = `
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
  block_id TEXT NOT NULL DEFAULT "",
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
`;

export type FixtureDb = InstanceType<typeof Database>;

export function createFixture(filePath: string, mode: "wal" | "delete" = "delete"): FixtureDb {
  const db = new Database(filePath);
  if (mode === "wal") db.pragma("journal_mode=WAL");
  db.exec(SCHEMA);
  return db;
}

/** Mimic Warp's per-character bold styling so stripAnsi is exercised. */
export function ansiWrap(text: string): Buffer {
  let out = "";
  for (const ch of text) out += `\x1b[1m${ch}\x1b[0m`;
  return Buffer.from(out, "utf8");
}

/** Seed a marker: a clean no-op in commands + a binding block at the same start_ts. */
export function seedMarker(
  db: FixtureDb,
  opts: {
    capture_id: string;
    label: Label;
    conversation_id: string;
    start_ts: string;
    session_id?: number;
    pwd?: string;
    git_branch?: string;
  }
): void {
  const command = `: CAPTURE_MARKER v=1 id=${opts.capture_id} label=${opts.label}`;
  db.prepare(
    `INSERT INTO commands (command, start_ts, is_agent_executed, session_id, pwd, git_branch)
     VALUES (?, ?, 1, ?, ?, ?)`
  ).run(command, opts.start_ts, opts.session_id ?? 1, opts.pwd ?? "/repo", opts.git_branch ?? "main");
  db.prepare(
    `INSERT INTO blocks (pane_leaf_uuid, stylized_command, stylized_output, pwd, git_branch, exit_code, did_execute, start_ts, completed_ts, ai_metadata, git_branch_name)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(
    Buffer.from([0, 0, 0, 0]),
    ansiWrap(command),
    Buffer.from(""),
    opts.pwd ?? "/repo",
    opts.git_branch ?? "main",
    0,
    1,
    opts.start_ts,
    opts.start_ts,
    JSON.stringify({ conversation_id: opts.conversation_id, subagent_task_id: null }),
    opts.git_branch ?? "main"
  );
}

/** Seed a marker command with NO binding block — the "unbindable" case. */
export function seedUnbindableMarker(
  db: FixtureDb,
  opts: { capture_id: string; label: Label; start_ts: string }
): void {
  const command = `: CAPTURE_MARKER v=1 id=${opts.capture_id} label=${opts.label}`;
  db.prepare(`INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)`).run(
    command,
    opts.start_ts
  );
}

export function seedQuery(
  db: FixtureDb,
  opts: {
    conversation_id: string;
    start_ts: string;
    text: string;
    working_directory?: string;
    model_id?: string;
    exchange_id?: string;
  }
): void {
  const input = JSON.stringify([{ Query: { text: opts.text } }]);
  db.prepare(
    `INSERT INTO ai_queries (exchange_id, conversation_id, start_ts, input, working_directory, output_status, model_id)
     VALUES (?, ?, ?, ?, ?, '', ?)`
  ).run(
    opts.exchange_id ?? `${opts.conversation_id}-ex`,
    opts.conversation_id,
    opts.start_ts,
    input,
    opts.working_directory ?? "/repo",
    opts.model_id ?? "claude-test"
  );
}

export function seedBlock(
  db: FixtureDb,
  opts: {
    conversation_id: string;
    start_ts: string;
    completed_ts?: string;
    command?: string;
    output?: string;
    ansi?: boolean;
    pwd?: string;
    git_branch?: string;
    exit_code?: number;
    did_execute?: number;
    subagent_task_id?: string | null;
  }
): void {
  const cmd = opts.command ?? "echo hi";
  const out = opts.output ?? "";
  const stylizedCmd = opts.ansi === false ? Buffer.from(cmd, "utf8") : ansiWrap(cmd);
  const stylizedOut = opts.ansi === false ? Buffer.from(out, "utf8") : ansiWrap(out);
  db.prepare(
    `INSERT INTO blocks (pane_leaf_uuid, stylized_command, stylized_output, pwd, git_branch, exit_code, did_execute, start_ts, completed_ts, ai_metadata, git_branch_name)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`
  ).run(
    Buffer.from([0, 0, 0, 0]),
    stylizedCmd,
    stylizedOut,
    opts.pwd ?? "/repo",
    opts.git_branch ?? "main",
    opts.exit_code ?? 0,
    opts.did_execute ?? 1,
    opts.start_ts,
    opts.completed_ts ?? opts.start_ts,
    JSON.stringify({ conversation_id: opts.conversation_id, subagent_task_id: opts.subagent_task_id ?? null }),
    opts.git_branch ?? "main"
  );
}

export function seedTask(
  db: FixtureDb,
  opts: {
    conversation_id: string;
    task: Buffer;
    task_id?: string;
    last_modified_at?: string;
  }
): void {
  db.prepare(
    `INSERT INTO agent_tasks (conversation_id, task_id, task, last_modified_at)
     VALUES (?, ?, ?, ?)`
  ).run(
    opts.conversation_id,
    opts.task_id ?? `${opts.conversation_id}-task`,
    opts.task,
    opts.last_modified_at ?? "2026-06-30 12:00:00.000000"
  );
}

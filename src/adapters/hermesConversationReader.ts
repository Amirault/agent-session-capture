import os from "node:os";
import path from "node:path";
import Database from "better-sqlite3";
import { DEFAULT_LABEL } from "../domain/models.js";
import type { EventDraft, Label, SkippedRow } from "../domain/models.js";
import type { ConversationReader, CaptureRead } from "../domain/ports.js";

export interface HermesConversationReaderOptions {
  dbPath?: string;
}

interface HermesSessionRow {
  id: string;
  source: string;
  model: string | null;
  parent_session_id: string | null;
  end_reason: string | null;
  model_config: string | null;
}

interface HermesMessageRow {
  id: number;
  session_id: string;
  role: string;
  content: string | null;
  tool_call_id: string | null;
  tool_calls: string | null;
  tool_name: string | null;
  timestamp: number;
  active: number;
  compacted: number;
}

interface ParsedToolCall {
  id: string;
  name: string;
  arguments: Record<string, unknown>;
}

interface ParsedMessage {
  row: HermesMessageRow;
  toolCalls: ParsedToolCall[];
}

interface ParsedMarker {
  captureId: string;
  label: Label;
}

const REQUIRED_COLUMNS = {
  sessions: ["id", "source", "parent_session_id"],
  messages: [
    "id",
    "session_id",
    "role",
    "content",
    "tool_call_id",
    "tool_calls",
    "tool_name",
    "timestamp",
  ],
} as const;

const MARKER_TOOLS = new Set(["terminal", "run_shell_command"]);
const MARKER_ANCHOR = "CAPTURE_MARKER";

export class HermesConversationReader implements ConversationReader {
  private readonly explicitDbPath: string | undefined;

  constructor(options: HermesConversationReaderOptions = {}) {
    this.explicitDbPath = options.dbPath;
  }

  readCapture(captureId: string): CaptureRead {
    const dbPath = this.resolveDbPath();
    const db = openReadOnly(dbPath);

    try {
      const readSnapshot = db.transaction(() => {
        const columns = validateSchema(db, dbPath);
        const sessions = readSessions(db, columns.sessions);
        const markerMessages = readMarkerMessages(db, columns.messages);
        const skipped: SkippedRow[] = [];
        const labelByCid = bindSessions(sessions, markerMessages, captureId, skipped);
        const messages = readBoundMessages(db, columns.messages, labelByCid, skipped);
        const sessionById = new Map(sessions.map((session) => [session.id, session]));
        const drafts = messages.flatMap((message) =>
          draftsFromMessage(message, labelByCid, sessionById)
        );

        return {
          source: "hermes" as const,
          labelByCid,
          drafts,
          skipped,
          unbindable: [],
          collisions: [],
          heuristic_bindings: [],
        };
      });
      return readSnapshot.deferred();
    } finally {
      db.close();
    }
  }

  resolveDbPath(): string {
    if (this.explicitDbPath !== undefined) return this.explicitDbPath;

    const configuredHome = process.env.HERMES_HOME?.trim();
    if (configuredHome) return path.join(configuredHome, "state.db");

    if (process.platform === "win32") {
      const localAppData = process.env.LOCALAPPDATA?.trim();
      const hermesHome = localAppData
        ? path.join(localAppData, "hermes")
        : path.join(os.homedir(), "AppData", "Local", "hermes");
      return path.join(hermesHome, "state.db");
    }

    return path.join(os.homedir(), ".hermes", "state.db");
  }
}

function openReadOnly(dbPath: string): Database.Database {
  try {
    return new Database(dbPath, { readonly: true, fileMustExist: true });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`Hermes state database error at "${dbPath}": ${detail}`);
  }
}

function validateSchema(
  db: Database.Database,
  dbPath: string
): Record<keyof typeof REQUIRED_COLUMNS, Set<string>> {
  const columns = {
    sessions: tableColumns(db, "sessions"),
    messages: tableColumns(db, "messages"),
  };
  const missing: string[] = [];

  for (const table of Object.keys(REQUIRED_COLUMNS) as Array<keyof typeof REQUIRED_COLUMNS>) {
    if (columns[table].size === 0) {
      missing.push(`table ${table}`);
      continue;
    }
    for (const column of REQUIRED_COLUMNS[table]) {
      if (!columns[table].has(column)) missing.push(`${table}.${column}`);
    }
  }

  if (missing.length > 0) {
    throw new Error(
      `Hermes schema compatibility error at "${dbPath}": missing ${missing.join(", ")}`
    );
  }

  return columns;
}

function tableColumns(db: Database.Database, table: string): Set<string> {
  const rows = db
    .prepare("SELECT name FROM pragma_table_info(?)")
    .all(table) as Array<{ name: string }>;
  return new Set(rows.map((row) => row.name));
}

function optionalColumn(columns: Set<string>, name: string, fallback: string): string {
  return columns.has(name) ? name : `${fallback} AS ${name}`;
}

function readSessions(
  db: Database.Database,
  columns: Set<string>
): HermesSessionRow[] {
  return db
    .prepare(
      `SELECT id,
              source,
              ${optionalColumn(columns, "model", "NULL")},
              parent_session_id,
              ${optionalColumn(columns, "end_reason", "NULL")},
              ${optionalColumn(columns, "model_config", "NULL")}
         FROM sessions
        ORDER BY ${columns.has("started_at") ? "started_at," : ""} id`
    )
    .all() as HermesSessionRow[];
}

function messageSelect(columns: Set<string>): string {
  return `SELECT id,
                 session_id,
                 role,
                 content,
                 tool_call_id,
                 tool_calls,
                 tool_name,
                 timestamp,
                 ${optionalColumn(columns, "active", "1")},
                 ${optionalColumn(columns, "compacted", "0")}
            FROM messages`;
}

function readMarkerMessages(
  db: Database.Database,
  columns: Set<string>
): HermesMessageRow[] {
  return db
    .prepare(
      `${messageSelect(columns)}
        WHERE role = 'assistant'
          AND tool_calls IS NOT NULL
          AND instr(tool_calls, ?) > 0
        ORDER BY id`
    )
    .all(MARKER_ANCHOR) as HermesMessageRow[];
}

function readBoundMessages(
  db: Database.Database,
  columns: Set<string>,
  labelByCid: Map<string, Label>,
  skipped: SkippedRow[]
): ParsedMessage[] {
  const statement = db.prepare(
    `${messageSelect(columns)}
      WHERE session_id = ?
      ORDER BY id`
  );
  const messages: ParsedMessage[] = [];

  for (const sessionId of labelByCid.keys()) {
    const rows = statement.all(sessionId) as HermesMessageRow[];
    for (const row of rows) messages.push(parseMessage(row, skipped));
  }

  return messages;
}

function parseMessage(row: HermesMessageRow, skipped: SkippedRow[]): ParsedMessage {
  return { row, toolCalls: parseToolCalls(row, skipped) };
}

function parseToolCalls(
  row: HermesMessageRow,
  skipped?: SkippedRow[]
): ParsedToolCall[] {
  if (row.tool_calls === null) return [];

  let values: unknown;
  try {
    values = JSON.parse(row.tool_calls);
    if (!Array.isArray(values)) throw new Error("expected an array");
  } catch (error) {
    recordToolCallSkip(row, skipped, error);
    return [];
  }

  const calls: ParsedToolCall[] = [];
  for (const [index, value] of values.entries()) {
    try {
      calls.push(parseToolCall(value));
    } catch (error) {
      recordToolCallSkip(row, skipped, error, index);
    }
  }
  return calls;
}

function recordToolCallSkip(
  row: HermesMessageRow,
  skipped: SkippedRow[] | undefined,
  error: unknown,
  index?: number
): void {
  if (skipped === undefined) return;
  const detail = error instanceof Error ? error.message : String(error);
  skipped.push({
    table: "messages",
    reason: `invalid tool_calls JSON: ${detail}`,
    detail: `message id=${row.id} session=${row.session_id}${index === undefined ? "" : ` call=${index}`}`,
  });
}

function parseToolCall(value: unknown): ParsedToolCall {
  if (!isRecord(value) || !isRecord(value.function)) {
    throw new Error("tool call is missing function data");
  }
  const id = stringField(value, "id") ?? stringField(value, "call_id");
  const name = stringField(value.function, "name");
  if (id === undefined || name === undefined) {
    throw new Error("tool call is missing an id or function name");
  }

  const rawArguments = value.function.arguments;
  const parsedArguments =
    typeof rawArguments === "string" ? JSON.parse(rawArguments) : rawArguments;
  if (!isRecord(parsedArguments)) {
    throw new Error("tool call function arguments are not an object");
  }

  return { id, name, arguments: parsedArguments };
}

function bindSessions(
  sessions: HermesSessionRow[],
  markerMessages: HermesMessageRow[],
  captureId: string,
  skipped: SkippedRow[]
): Map<string, Label> {
  const markersBySession = new Map<string, ParsedMarker[]>();
  for (const row of markerMessages) {
    for (const call of parseToolCalls(row)) {
      const markers = markersFromToolCall(call);
      if (markers.length === 0) continue;
      const sessionMarkers = markersBySession.get(row.session_id) ?? [];
      sessionMarkers.push(...markers);
      markersBySession.set(row.session_id, sessionMarkers);
    }
  }

  const explicitLabels = new Map<string, Label>();
  const barriers = new Set<string>();
  for (const [sessionId, markers] of markersBySession) {
    const targetLabels = new Set(
      markers.filter((marker) => marker.captureId === captureId).map((marker) => marker.label)
    );
    if (targetLabels.size === 1) {
      explicitLabels.set(sessionId, [...targetLabels][0]!);
    } else if (targetLabels.size > 1) {
      barriers.add(sessionId);
      skipped.push({
        table: "messages",
        reason: "same session has markers for multiple labels",
        detail: `${sessionId}: ${[...targetLabels].join(", ")}`,
      });
    } else {
      barriers.add(sessionId);
    }
  }

  const sessionById = new Map(sessions.map((session) => [session.id, session]));
  const labelByCid = new Map<string, Label>();
  for (const session of sessions) {
    const label = nearestMarkedLabel(session, sessionById, explicitLabels, barriers);
    if (label !== undefined) labelByCid.set(session.id, label);
  }
  return labelByCid;
}

function nearestMarkedLabel(
  session: HermesSessionRow,
  sessionById: Map<string, HermesSessionRow>,
  explicitLabels: Map<string, Label>,
  barriers: Set<string>
): Label | undefined {
  const visited = new Set<string>();
  let current: HermesSessionRow | undefined = session;

  while (current !== undefined && !visited.has(current.id)) {
    visited.add(current.id);
    const explicit = explicitLabels.get(current.id);
    if (explicit !== undefined) return explicit;
    if (barriers.has(current.id)) return undefined;

    const parentId = workflowParentId(current, sessionById);
    current = parentId === undefined ? undefined : sessionById.get(parentId);
  }

  return undefined;
}

function workflowParentId(
  session: HermesSessionRow,
  sessionById: Map<string, HermesSessionRow>
): string | undefined {
  const parentId = session.parent_session_id;
  if (parentId === null || session.source === "tool") return undefined;

  const config = parseModelConfig(session.model_config);
  if (hasConfigMarker(config._branched_from)) return undefined;
  if (hasConfigMarker(config._delegate_from) || session.source === "subagent") {
    return parentId;
  }

  return sessionById.get(parentId)?.end_reason === "compression" ? parentId : undefined;
}

function hasConfigMarker(value: unknown): boolean {
  return value !== undefined && value !== null;
}

function parseModelConfig(raw: string | null): Record<string, unknown> {
  if (raw === null) return {};
  try {
    const value: unknown = JSON.parse(raw);
    return isRecord(value) ? value : {};
  } catch {
    return {};
  }
}

function markersFromToolCall(call: ParsedToolCall): ParsedMarker[] {
  if (!MARKER_TOOLS.has(call.name)) return [];
  const command = stringField(call.arguments, "command");
  if (command === undefined) return [];

  const markers: ParsedMarker[] = [];
  for (const line of command.split("\n")) {
    const marker = parseExactMarkerLine(line.trim());
    if (marker !== null) markers.push(marker);
  }
  return markers;
}

function parseExactMarkerLine(line: string): ParsedMarker | null {
  const tokens = line.split(" ");
  if (
    (tokens.length !== 4 && tokens.length !== 5) ||
    tokens[0] !== ":" ||
    tokens[1] !== "CAPTURE_MARKER" ||
    tokens[2] !== "v=1" ||
    !tokens[3]!.startsWith("id=") ||
    (tokens.length === 5 && !tokens[4]!.startsWith("label="))
  ) {
    return null;
  }

  const captureId = tokens[3]!.slice("id=".length);
  const label = tokens.length === 5 ? tokens[4]!.slice("label=".length) : DEFAULT_LABEL;
  if (captureId.length === 0 || label.length === 0) return null;
  return { captureId, label };
}

function draftsFromMessage(
  message: ParsedMessage,
  labelByCid: Map<string, Label>,
  sessionById: Map<string, HermesSessionRow>
): EventDraft[] {
  const { row, toolCalls } = message;
  if (!labelByCid.has(row.session_id)) return [];
  const session = sessionById.get(row.session_id);
  if (session === undefined) return [];

  const ts = new Date(row.timestamp * 1000).toISOString();
  const baseMeta = cleanMeta({
    message_id: row.id,
    active: row.active,
    compacted: row.compacted,
    session_source: session.source,
    model: session.model,
  });

  if (row.role === "user") {
    return row.content === null
      ? []
      : [draft(row.session_id, ts, "user", "query", row.content, baseMeta, 0)];
  }
  if (row.role === "tool") {
    return row.content === null
      ? []
      : [
          draft(
            row.session_id,
            ts,
            "tool",
            "tool_result",
            row.content,
            {
              ...baseMeta,
              ...cleanMeta({ tool: row.tool_name, tool_call_id: row.tool_call_id }),
            },
            0
          ),
        ];
  }
  if (row.role !== "assistant") return [];

  const drafts: EventDraft[] = [];
  let eventIndex = 0;
  if (row.content !== null && row.content.length > 0) {
    drafts.push(
      draft(
        row.session_id,
        ts,
        "assistant",
        "agent_message",
        row.content,
        baseMeta,
        eventIndex
      )
    );
    eventIndex += 1;
  }
  for (const call of toolCalls) {
    drafts.push(
      draft(
        row.session_id,
        ts,
        "assistant",
        "tool_call",
        toolCallContent(call),
        {
          ...baseMeta,
          tool: call.name,
          tool_call_id: call.id,
          input: call.arguments,
        },
        eventIndex
      )
    );
    eventIndex += 1;
  }
  return drafts;
}

function draft(
  conversation_id: string,
  ts: string,
  role: EventDraft["role"],
  kind: EventDraft["kind"],
  content: string,
  meta: Record<string, unknown>,
  messageEventIndex: number
): EventDraft {
  return {
    conversation_id,
    ts,
    role,
    kind,
    content,
    meta: { ...meta, message_event_index: messageEventIndex },
  };
}

function toolCallContent(call: ParsedToolCall): string {
  if (MARKER_TOOLS.has(call.name)) {
    const command = stringField(call.arguments, "command");
    if (command !== undefined) return command;
  }
  return JSON.stringify(call.arguments);
}

function cleanMeta(meta: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(meta).filter(([, value]) => value !== null && value !== undefined)
  );
}

function stringField(
  record: Record<string, unknown>,
  key: string
): string | undefined {
  const value = record[key];
  return typeof value === "string" ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

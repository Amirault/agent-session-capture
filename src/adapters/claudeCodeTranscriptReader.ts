import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import type { EventDraft, Label, SkippedRow } from "../domain/models.js";
import type { ConversationReader, CaptureRead } from "../domain/ports.js";
import { parseMarker } from "./readers/markerReader.js";

export interface ClaudeCodeTranscriptReaderOptions {
  rootDir?: string;
}

interface JsonlRecord {
  filePath: string;
  lineNumber: number;
  value: Record<string, unknown>;
}

interface MarkerHit {
  conversation_id: string;
  label: Label;
  marker_command: string;
  start_ts: string;
}

export class ClaudeCodeTranscriptReader implements ConversationReader {
  private readonly rootDir: string;

  constructor(options: ClaudeCodeTranscriptReaderOptions = {}) {
    this.rootDir = options.rootDir ?? path.join(os.homedir(), ".claude", "projects");
  }

  readCapture(captureId: string): CaptureRead {
    const files = listJsonlFiles(this.rootDir);
    const records: JsonlRecord[] = [];
    const skipped: SkippedRow[] = [];
    if (!fs.existsSync(this.rootDir)) {
      skipped.push({
        table: "claude-root",
        reason: "transcript root not found",
        detail: this.rootDir,
      });
    }

    for (const filePath of files) {
      const read = readJsonl(filePath);
      records.push(...read.records);
      skipped.push(...read.skipped);
    }

    const splitSessions = multiLabelSessions(findMarkerHits(records, captureId, new Set()));
    const markerHits = findMarkerHits(records, captureId, splitSessions);
    const labelByCid = bindLabels(markerHits, skipped);
    const boundCids = new Set(labelByCid.keys());
    const drafts = records.flatMap((record) =>
      draftsFromRecord(record, boundCids, splitSessions, this.rootDir, skipped)
    );

    return {
      source: "claude-code",
      labelByCid,
      drafts,
      skipped,
      unbindable: [],
      collisions: [],
      heuristic_bindings: [],
    };
  }
}

function listJsonlFiles(rootDir: string): string[] {
  if (!fs.existsSync(rootDir)) return [];
  const entries = fs.readdirSync(rootDir, { withFileTypes: true });
  const files: string[] = [];

  for (const entry of entries) {
    const fullPath = path.join(rootDir, entry.name);
    if (entry.isDirectory()) {
      files.push(...listJsonlFiles(fullPath));
    } else if (entry.isFile() && entry.name.endsWith(".jsonl")) {
      files.push(fullPath);
    }
  }

  return files.sort();
}

function readJsonl(filePath: string): { records: JsonlRecord[]; skipped: SkippedRow[] } {
  const records: JsonlRecord[] = [];
  const skipped: SkippedRow[] = [];
  const lines = fs.readFileSync(filePath, "utf8").split("\n");

  lines.forEach((line, index) => {
    const lineNumber = index + 1;
    if (line.trim().length === 0) return;

    try {
      const value: unknown = JSON.parse(line);
      if (isRecord(value)) {
        records.push({ filePath, lineNumber, value });
      } else {
        skipped.push(skippedLine(filePath, lineNumber, "json value is not an object"));
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : "unknown parse error";
      skipped.push(skippedLine(filePath, lineNumber, `invalid JSON: ${message}`));
    }
  });

  return { records, skipped };
}

function skippedLine(filePath: string, lineNumber: number, reason: string): SkippedRow {
  return {
    table: "claude_code_transcripts",
    reason,
    detail: `${filePath}:${lineNumber}`,
  };
}

function findMarkerHits(records: JsonlRecord[], captureId: string, splitSessions: Set<string>): MarkerHit[] {
  const hits: MarkerHit[] = [];

  for (const record of records) {
    const conversation_id = conversationId(record, splitSessions);
    const start_ts = timestamp(record);
    if (conversation_id === null || start_ts === null) continue;

    for (const candidate of markerCandidateTexts(record.value)) {
      const parsed = parseMarker(candidate);
      if (parsed === null || parsed.capture_id !== captureId) continue;
      hits.push({
        conversation_id,
        label: parsed.label,
        marker_command: candidate,
        start_ts,
      });
    }
  }

  return hits.sort((a, b) => (a.start_ts < b.start_ts ? -1 : a.start_ts > b.start_ts ? 1 : 0));
}

function multiLabelSessions(markerHits: MarkerHit[]): Set<string> {
  const labelsBySession = new Map<string, Set<Label>>();
  for (const hit of markerHits) {
    const labels = labelsBySession.get(hit.conversation_id) ?? new Set<Label>();
    labels.add(hit.label);
    labelsBySession.set(hit.conversation_id, labels);
  }
  return new Set([...labelsBySession].filter(([, labels]) => labels.size > 1).map(([sessionId]) => sessionId));
}

function bindLabels(markerHits: MarkerHit[], skipped: SkippedRow[]): Map<string, Label> {
  const labelByCid = new Map<string, Label>();
  const seen = new Set<string>();

  for (const hit of markerHits) {
    const previous = labelByCid.get(hit.conversation_id);
    if (previous !== undefined) {
      if (previous !== hit.label) {
        skipped.push({
          table: "claude_code_transcripts",
          reason: "same session has markers for multiple labels",
          detail: `${hit.conversation_id}: ${previous} then ${hit.label}`,
        });
        labelByCid.delete(hit.conversation_id);
        seen.add(hit.conversation_id);
      }
      continue;
    }
    if (seen.has(hit.conversation_id)) continue;
    labelByCid.set(hit.conversation_id, hit.label);
  }

  return labelByCid;
}

function draftsFromRecord(
  record: JsonlRecord,
  boundCids: Set<string>,
  splitSessions: Set<string>,
  rootDir: string,
  skipped: SkippedRow[]
): EventDraft[] {
  const conversation_id = conversationId(record, splitSessions);
  const ts = timestamp(record);
  if (conversation_id === null || !boundCids.has(conversation_id)) return [];
  // Session metadata records (custom-title, last-prompt, file-history-snapshot, …)
  // carry no timestamp and are not events — only message records are.
  const type = stringField(record.value, "type");
  if (type !== "user" && type !== "assistant") return [];
  if (ts === null) {
    skipped.push(skippedLine(record.filePath, record.lineNumber, "missing timestamp"));
    return [];
  }

  const message = record.value.message;
  if (!isRecord(message)) return [];

  if (type === "user") return userDrafts(record, conversation_id, ts, message, rootDir);
  if (type === "assistant") return assistantDrafts(record, conversation_id, ts, message, rootDir);
  return [];
}

function userDrafts(
  record: JsonlRecord,
  conversation_id: string,
  ts: string,
  message: Record<string, unknown>,
  rootDir: string
): EventDraft[] {
  const content = message.content;
  if (typeof content === "string") {
    return [draft(record, rootDir, conversation_id, ts, "user", "query", content, {})];
  }
  if (!Array.isArray(content)) return [];

  return content.flatMap((block) => {
    if (!isRecord(block)) return [];
    const blockType = stringField(block, "type");
    if (blockType === "tool_result") {
      return [
        draft(record, rootDir, conversation_id, ts, "tool", "tool_result", contentText(block.content), {
          tool_use_id: stringField(block, "tool_use_id"),
        }),
      ];
    }
    if (blockType === "text") {
      return [draft(record, rootDir, conversation_id, ts, "user", "query", contentText(block.text), {})];
    }
    return [];
  });
}

function assistantDrafts(
  record: JsonlRecord,
  conversation_id: string,
  ts: string,
  message: Record<string, unknown>,
  rootDir: string
): EventDraft[] {
  const content = message.content;
  if (!Array.isArray(content)) return [];

  return content.flatMap((block) => {
    if (!isRecord(block)) return [];
    const blockType = stringField(block, "type");
    if (blockType === "text" || blockType === "thinking") {
      return [
        draft(record, rootDir, conversation_id, ts, "assistant", "agent_message", contentText(block.text ?? block.thinking), {
          message_kind: blockType,
          model: stringField(message, "model"),
          message_id: stringField(message, "id"),
        }),
      ];
    }
    if (blockType === "tool_use") {
      const input = isRecord(block.input) ? block.input : {};
      const tool = stringField(block, "name");
      return [
        draft(record, rootDir, conversation_id, ts, "assistant", "tool_call", toolCallContent(tool, input), {
          message_kind: "tool_use",
          tool,
          tool_use_id: stringField(block, "id"),
          input,
          model: stringField(message, "model"),
          message_id: stringField(message, "id"),
        }),
      ];
    }
    return [];
  });
}

function draft(
  record: JsonlRecord,
  rootDir: string,
  conversation_id: string,
  ts: string,
  role: EventDraft["role"],
  kind: EventDraft["kind"],
  content: string,
  extraMeta: Record<string, unknown>
): EventDraft {
  return {
    conversation_id,
    ts,
    role,
    kind,
    content,
    meta: cleanMeta({
      ...baseMeta(record, rootDir),
      ...extraMeta,
    }),
  };
}

function baseMeta(record: JsonlRecord, rootDir: string): Record<string, unknown> {
  return {
    uuid: stringField(record.value, "uuid"),
    parent_uuid: stringField(record.value, "parentUuid"),
    cwd: stringField(record.value, "cwd"),
    git_branch: stringField(record.value, "gitBranch"),
    claude_version: stringField(record.value, "version"),
    is_sidechain: record.value.isSidechain,
    source_file: path.relative(rootDir, record.filePath),
    source_line: record.lineNumber,
  };
}

function cleanMeta(meta: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(Object.entries(meta).filter(([, value]) => value !== undefined && value !== null));
}

function markerCandidateTexts(value: Record<string, unknown>): string[] {
  if (stringField(value, "type") !== "assistant") return [];
  const message = value.message;
  if (!isRecord(message)) return [];
  const content = message.content;
  const candidates: string[] = [];

  if (Array.isArray(content)) {
    for (const block of content) {
      if (!isRecord(block)) continue;
      const blockType = stringField(block, "type");
      const tool = stringField(block, "name");
      if (blockType === "tool_use" && tool === "Bash" && isRecord(block.input)) {
        const command = stringField(block.input, "command");
        if (command !== undefined) candidates.push(command);
      }
    }
  }

  return candidates;
}

function toolCallContent(tool: string | undefined, input: Record<string, unknown>): string {
  const command = stringField(input, "command");
  if (tool === "Bash" && command !== undefined) return command;
  return stableJson(input);
}

function contentText(value: unknown): string {
  if (typeof value === "string") return value;
  if (Array.isArray(value)) return value.map(contentText).filter((text) => text.length > 0).join("\n");
  if (isRecord(value)) {
    const text = stringField(value, "text") ?? stringField(value, "content");
    if (text !== undefined) return text;
  }
  if (value === undefined || value === null) return "";
  return stableJson(value);
}

function stableJson(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return String(value);
  }
}

function conversationId(record: JsonlRecord, splitSessions: Set<string>): string | null {
  const sessionId = stringField(record.value, "sessionId") ?? path.basename(record.filePath, ".jsonl");
  const agentId = stringField(record.value, "agentId");
  if (agentId === undefined || !splitSessions.has(sessionId)) return sessionId;
  return `${sessionId}/agent-${agentId}`;
}

function timestamp(record: JsonlRecord): string | null {
  return stringField(record.value, "timestamp") ?? null;
}

function stringField(record: Record<string, unknown>, key: string): string | undefined {
  const value = record[key];
  return typeof value === "string" ? value : undefined;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

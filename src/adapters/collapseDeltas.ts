/**
 * Collapses streaming field-deltas emitted for `tool_call` / `tool_call_result`
 * messages, and generically merges any other consecutive same-`message_kind`
 * deltas of one logical entity.
 *
 * Warp streams a call/result incrementally: instead of one `Message` carrying
 * every field, the wire task blob holds many separate `Message` occurrences,
 * each setting a single leaf field (e.g. one occurrence just for
 * `tool_call_result.run_shell_command.output`, another just for
 * `...context.updated_skills_context.available_skills[i].path`). The walker +
 * schema overlay surface these as a run of `agent_message` nodes sharing the
 * same `message_kind` (and, for calls/results, the same `tool_call_id`).
 *
 * This pass:
 *  - groups a contiguous run of `tool_call`/`tool_call_result` nodes that
 *    share one `tool_call_id` into a single event carrying a `fields` map
 *    (relative field path -> value, or -> value[] when the same relative
 *    path recurs with a different value — e.g. `diffs.file_path` for each
 *    file in a multi-file `apply_file_diffs` call — so sibling repeated-field
 *    items are never silently overwritten) plus the `tool_call_id`;
 *  - within such a group, reconstructs `updated_skills_context` fan-out
 *    (one leaf per skill field) into a `skills` summary list instead of
 *    leaking every `path`/`name` leaf into `fields`;
 *  - for any other `message_kind`, merges a run of consecutive nodes sharing
 *    the same `message_kind` AND `field_path` onto the last (most complete)
 *    value, keeping the first occurrence's position and recording a
 *    `merged_count`. Pure exact-repeat dedup is already handled upstream by
 *    `compact.ts`; this only fires when consecutive deltas of the same field
 *    hold *different* values (progressive/streamed content).
 *
 * Nodes without a `message_kind` (Task-level scalars, outside any `Message`
 * oneof) always pass through untouched.
 */
import type { NamedNode } from "./schemaOverlay.js";

export interface SkillSummary {
  path?: string;
  name?: string;
}

export interface CollapsedNode extends NamedNode {
  tool_call_id?: string;
  fields?: Record<string, string | string[]>;
  skills?: SkillSummary[];
  merged_count?: number;
}

/** Message oneof variants whose per-field streaming deltas get grouped into one event. */
const GROUPED_KINDS: ReadonlySet<string> = new Set(["tool_call", "tool_call_result"]);

/** Relative-path marker for skills fan-out, matched regardless of nesting depth. */
const SKILLS_LEAF = "updated_skills_context.available_skills.";

export function collapseDeltas(nodes: readonly NamedNode[]): CollapsedNode[] {
  const out: CollapsedNode[] = [];
  let i = 0;
  while (i < nodes.length) {
    const node = nodes[i]!;
    if (node.message_kind !== undefined && GROUPED_KINDS.has(node.message_kind)) {
      const end = groupEnd(nodes, i);
      out.push(collapseGroup(nodes.slice(i, end)));
      i = end;
      continue;
    }
    const end = node.message_kind !== undefined ? sameFieldRunEnd(nodes, i) : i + 1;
    if (end > i + 1) {
      out.push({ ...nodes[end - 1]!, merged_count: end - i });
    } else {
      out.push({ ...node });
    }
    i = end;
  }
  return out;
}

/** End (exclusive) of a contiguous run sharing `message_kind` and (if present) `tool_call_id`. */
function groupEnd(nodes: readonly NamedNode[], start: number): number {
  const kind = nodes[start]!.message_kind;
  const idPath = `messages.${kind}.tool_call_id`;
  let toolCallId: string | undefined;
  let i = start;
  for (; i < nodes.length && nodes[i]!.message_kind === kind; i++) {
    if (nodes[i]!.field_path === idPath) {
      const id = nodes[i]!.value;
      if (toolCallId !== undefined && id !== toolCallId) break;
      toolCallId = id;
    }
  }
  return i;
}

/** End (exclusive) of a contiguous run sharing both `message_kind` and `field_path`. */
function sameFieldRunEnd(nodes: readonly NamedNode[], start: number): number {
  const { message_kind, field_path } = nodes[start]!;
  let i = start + 1;
  while (
    i < nodes.length &&
    nodes[i]!.message_kind === message_kind &&
    nodes[i]!.field_path === field_path
  ) {
    i++;
  }
  return i;
}

function collapseGroup(group: readonly NamedNode[]): CollapsedNode {
  const kind = group[0]!.message_kind!;
  const prefix = `messages.${kind}.`;
  const fields: Record<string, string | string[]> = {};
  const skillNodes: NamedNode[] = [];
  let toolCallId: string | undefined;

  for (const node of group) {
    const rel = node.field_path.startsWith(prefix) ? node.field_path.slice(prefix.length) : node.field_path;
    if (rel === "tool_call_id") {
      toolCallId = node.value;
      continue;
    }
    if (rel.includes(SKILLS_LEAF)) {
      skillNodes.push(node);
      continue;
    }
    addField(fields, rel, node.value);
  }

  const skills = collapseSkills(skillNodes);
  const toolNode = group.find((n) => n.tool !== undefined);

  const collapsed: CollapsedNode = {
    ...group[0]!,
    field_path: `messages.${kind}`,
    value: summarizeCollapsedNode(fields, skills),
  };
  if (toolNode?.tool !== undefined) collapsed.tool = toolNode.tool;
  if (toolCallId !== undefined) collapsed.tool_call_id = toolCallId;
  if (Object.keys(fields).length > 0) collapsed.fields = fields;
  if (skills.length > 0) collapsed.skills = skills;
  if (group.length > 1) collapsed.merged_count = group.length;
  return collapsed;
}

/**
 * Records a relative-field value without overwriting an earlier, distinct
 * value for the same path. A repeated field number (e.g. `diffs.file_path`
 * for each file in a multi-file `apply_file_diffs` call, or any other
 * repeated sub-message) legitimately carries a *different* value per
 * occurrence — the second one is not a delta on the first, it is a sibling.
 * Overwriting would silently drop every occurrence but the last.
 */
function addField(fields: Record<string, string | string[]>, rel: string, value: string): void {
  const existing = fields[rel];
  if (existing === undefined) {
    fields[rel] = value;
  } else if (Array.isArray(existing)) {
    if (existing[existing.length - 1] !== value) existing.push(value);
  } else if (existing !== value) {
    fields[rel] = [existing, value];
  }
}

export function summarizeCollapsedNode(
  fields: Record<string, string | string[]>,
  skills: SkillSummary[]
): string {
  const parts = Object.keys(fields).length > 0 ? [JSON.stringify(fields)] : [];
  if (skills.length > 0) {
    const names = skills.map((s) => s.path ?? s.name ?? "?").join(", ");
    parts.push(`skills_context(${skills.length}): ${names}`);
  }
  return parts.join(" ");
}

/**
 * Reconstructs skill records from a flat run of `path`/`name` leaves (one leaf
 * per streamed delta). A new record starts whenever a `path` leaf arrives and
 * the current record already has one — this mirrors the observed one-skill-
 * at-a-time streaming order (path, then name, then the next skill's path...).
 */
function collapseSkills(nodes: readonly NamedNode[]): SkillSummary[] {
  const skills: SkillSummary[] = [];
  let current: SkillSummary | null = null;
  for (const node of nodes) {
    const idx = node.field_path.lastIndexOf(SKILLS_LEAF);
    const leaf = node.field_path.slice(idx + SKILLS_LEAF.length);
    if (leaf === "path") {
      if (current?.path !== undefined) {
        skills.push(current);
        current = null;
      }
      current = current ?? {};
      current.path = node.value;
    } else if (leaf === "name") {
      current = current ?? {};
      current.name = node.value;
    }
    // description / bundled_skill_id / provider / scope: dropped from the
    // summary (they add bytes without adding decision-tracing signal).
  }
  if (current) skills.push(current);
  return skills;
}

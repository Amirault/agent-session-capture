import type { EventDraft, Label, SessionInfo, SkippedRow } from "../domain/models.js";
import type {
  ConversationReader,
  ReadSessionsOptions,
  SessionGather,
  SessionReader,
  SessionsRead,
  CaptureRead,
} from "../domain/ports.js";
import { shouldGather } from "../domain/skipUnchanged.js";
import { exactMarkersInCommand } from "../domain/captureMarker.js";
import { parseSinceUtc } from "../domain/stamps.js";
import type { LangfuseObservation, LangfuseObservationLister } from "./langfuseObservationLister.js";

export interface LangfuseConversationReaderOptions {
  now?: () => Date;
}

const LOOKBACK_MS = 30 * 24 * 60 * 60 * 1000;
const MARKER_TYPES = new Set(["TOOL", "SPAN"]);
const MARKER_TOOLS = new Set(["terminal", "run_shell_command", "Bash"]);

/**
 * Marker and sessions modes over a Langfuse project, read-only. A conversation
 * is the `sessionId` of a trace, else its `traceId`. In marker mode it binds to
 * a label through an exact marker line run as the `command` of a `TOOL`/`SPAN`
 * observation named `terminal`, `run_shell_command` or `Bash`. The client is
 * injected, so the reader never touches the network itself.
 */
export class LangfuseConversationReader implements ConversationReader, SessionReader {
  private readonly now: () => Date;

  constructor(
    private readonly client: LangfuseObservationLister,
    options: LangfuseConversationReaderOptions = {}
  ) {
    this.now = options.now ?? (() => new Date());
  }

  async readCapture(captureId: string): Promise<CaptureRead> {
    const fromStartTime = new Date(this.now().getTime() - LOOKBACK_MS).toISOString();
    const observations = await this.client.listObservations({ fromStartTime });
    const skipped: SkippedRow[] = [];
    const conversationOf = conversationsOf(observations);
    const labelByCid = bindConversations(observations, conversationOf, captureId, skipped);
    const bound = observations.filter((o) => labelByCid.has(conversationOf(o)));

    return {
      source: "langfuse",
      labelByCid,
      drafts: draftsFromObservations(bound, conversationOf),
      skipped,
      unbindable: [],
      collisions: [],
      heuristic_bindings: [],
    };
  }

  /**
   * Sessions mode: one content-less pass enumerates the conversations active
   * at/after `--since` (every trace of a session is one group, stamped with
   * the latest `updatedAt`); only the changed groups are then fetched, trace
   * by trace with the `traceId` filter (`sessionId` matches the root alone).
   * With `--since`, a trace whose root started before the cutoff looks up its
   * root to find its session, and a changed session first lists its roots by
   * `sessionId`, so its file also holds the traces started before the cutoff.
   */
  async readSessions(options: ReadSessionsOptions): Promise<SessionsRead> {
    const cutoff = options.since === undefined ? null : parseSinceUtc(options.since);
    const activity = await this.client.listActivity(
      cutoff === null ? {} : { fromStartTime: new Date(cutoff).toISOString() }
    );
    if (cutoff !== null) activity.push(...(await this.sessionRootsOfCutTraces(activity)));
    let groups = groupByConversation(activity);
    if (options.conversation !== undefined) {
      groups = groups.filter((g) => g.info.conversation_id === options.conversation);
    }

    const reads = new Map<string, SessionGather>();
    for (const group of groups) {
      if (!shouldGather(group.info, options)) continue;
      reads.set(group.info.conversation_id, await this.gatherGroup(group, cutoff !== null));
    }
    return { source: "langfuse", sessions: groups.map((g) => g.info), reads };
  }

  /**
   * A windowed pass misses the root of a trace that started before the cutoff,
   * and only the root carries `sessionId`: such a trace looks up its root, so
   * it joins its session instead of becoming a session-less trace of its own.
   */
  private async sessionRootsOfCutTraces(activity: LangfuseObservation[]): Promise<LangfuseObservation[]> {
    const resolved = new Set(traceIdsIn(activity.filter((o) => isRoot(o) || Boolean(o.sessionId))));
    const cut = new Set(traceIdsIn(activity).filter((traceId) => !resolved.has(traceId)));
    const roots: LangfuseObservation[] = [];
    for (const traceId of cut) {
      const trace = await this.client.listActivity({ traceId });
      roots.push(...trace.filter((o) => Boolean(o.sessionId)));
    }
    return roots;
  }

  private async gatherGroup(group: ConversationGroup, windowed: boolean): Promise<SessionGather> {
    const observations: LangfuseObservation[] = [];
    for (const traceId of await this.traceIdsOf(group, windowed)) {
      observations.push(...(await this.client.listObservations({ traceId })));
    }
    const cid = group.info.conversation_id;
    return {
      drafts: draftsFromObservations(observations, () => cid),
      skipped: [],
      last_modified: group.info.last_modified,
    };
  }

  /** A windowed activity pass only sees a session's recent traces: its roots name them all. */
  private async traceIdsOf(group: ConversationGroup, windowed: boolean): Promise<Set<string>> {
    if (!windowed || !group.isSession) return group.traceIds;
    const roots = await this.client.listActivity({ sessionId: group.info.conversation_id });
    return new Set([...traceIdsIn(roots), ...group.traceIds]);
  }
}

interface ConversationGroup {
  info: SessionInfo;
  traceIds: Set<string>;
  isSession: boolean;
}

function traceIdsIn(observations: LangfuseObservation[]): string[] {
  return observations.flatMap((o) => (o.traceId === null ? [] : [o.traceId]));
}

/** Activity → one group per conversation, its traces and its latest `updatedAt` (ISO-8601 UTC). */
function groupByConversation(activity: LangfuseObservation[]): ConversationGroup[] {
  const conversationOf = conversationsOf(activity);
  const groups = new Map<string, { traceIds: Set<string>; latest: number | null; isSession: boolean }>();
  for (const observation of activity) {
    const cid = conversationOf(observation);
    const group = groups.get(cid) ?? { traceIds: new Set<string>(), latest: null, isSession: false };
    if (observation.traceId !== null) group.traceIds.add(observation.traceId);
    if (observation.sessionId === cid) group.isSession = true;
    const updatedAt = observation.updatedAt ? Date.parse(observation.updatedAt) : NaN;
    if (!Number.isNaN(updatedAt)) group.latest = Math.max(group.latest ?? updatedAt, updatedAt);
    groups.set(cid, group);
  }
  return [...groups]
    .map(([cid, { traceIds, latest, isSession }]) => ({
      info: {
        conversation_id: cid,
        last_modified: latest === null ? null : new Date(latest).toISOString(),
      },
      traceIds,
      isSession,
    }))
    .sort((a, b) => a.info.conversation_id.localeCompare(b.info.conversation_id));
}

type ConversationOf = (observation: LangfuseObservation) => string;

/**
 * Langfuse v2 returns `sessionId` only on the observation that carries the
 * session (usually the trace root) and `""` on the others, so the session is
 * resolved per trace: any non-empty `sessionId` of the trace, else its `traceId`.
 */
function conversationsOf(observations: LangfuseObservation[]): ConversationOf {
  const sessionByTrace = new Map<string, string>();
  for (const observation of observations) {
    if (observation.traceId !== null && observation.sessionId) {
      sessionByTrace.set(observation.traceId, observation.sessionId);
    }
  }
  return (observation) =>
    (observation.traceId === null ? undefined : sessionByTrace.get(observation.traceId)) ||
    observation.sessionId ||
    observation.traceId ||
    observation.id;
}

function bindConversations(
  observations: LangfuseObservation[],
  conversationOf: ConversationOf,
  captureId: string,
  skipped: SkippedRow[]
): Map<string, Label> {
  const labelsByCid = new Map<string, Set<Label>>();
  for (const observation of [...observations].sort(byStartTime)) {
    for (const marker of markersOf(observation)) {
      if (marker.captureId !== captureId) continue;
      const cid = conversationOf(observation);
      labelsByCid.set(cid, (labelsByCid.get(cid) ?? new Set()).add(marker.label));
    }
  }

  const labelByCid = new Map<string, Label>();
  for (const [cid, labels] of labelsByCid) {
    if (labels.size === 1) {
      labelByCid.set(cid, [...labels][0]!);
    } else {
      skipped.push({
        table: "observations",
        reason: "same session has markers for multiple labels",
        detail: `${cid}: ${[...labels].join(", ")}`,
      });
    }
  }
  return labelByCid;
}

function markersOf(observation: LangfuseObservation) {
  if (!MARKER_TYPES.has(observation.type)) return [];
  const command = commandOf(observation);
  return command === undefined ? [] : exactMarkersInCommand(command);
}

/**
 * Observations → events (the use cases order them by `ts`): a root
 * observation's input is the `query`, a `GENERATION` output an
 * `agent_message`, a `TOOL`/`SPAN` input a `tool_call` and its output a
 * `tool_result` (both carry the observation id as `tool_call_id`). Inputs are
 * stamped at the observation's `startTime`, outputs at its `endTime` (else
 * `startTime`), so tool and answer durations survive. Anything else carries no
 * conversation content.
 */
function draftsFromObservations(
  observations: LangfuseObservation[],
  conversationOf: ConversationOf
): EventDraft[] {
  const agentByTrace = agentsByTrace(observations);
  return observations.flatMap((observation) => {
    const cid = conversationOf(observation);
    const started = new Date(observation.startTime).toISOString();
    const ended = new Date(observation.endTime ?? observation.startTime).toISOString();
    const meta = metaOf(observation, agentByTrace);
    const event = (
      ts: string,
      role: EventDraft["role"],
      kind: EventDraft["kind"],
      content: string | null,
      extra = {}
    ) =>
      content === null || content.length === 0
        ? []
        : [{ conversation_id: cid, ts, role, kind, content, meta: { ...meta, ...extra } }];

    if (isRoot(observation)) return event(started, "user", "query", textOf(observation.input));
    if (observation.type === "GENERATION") {
      return event(ended, "assistant", "agent_message", textOf(observation.output));
    }
    if (!MARKER_TYPES.has(observation.type)) return [];
    const tool = cleanMeta({ tool: observation.name, tool_call_id: observation.id });
    return [
      ...event(started, "assistant", "tool_call", commandOf(observation) ?? textOf(observation.input), tool),
      ...event(ended, "tool", "tool_result", textOf(observation.output), tool),
    ];
  });
}

function metaOf(
  observation: LangfuseObservation,
  agentByTrace: Map<string, string>
): Record<string, unknown> {
  return cleanMeta({
    observation_id: observation.id,
    observation_type: observation.type,
    trace_id: observation.traceId,
    session_source: observation.traceId === null ? undefined : agentByTrace.get(observation.traceId),
  });
}

/** The agent each trace names in its metadata (`agent` on the trace's root observation). */
function agentsByTrace(observations: LangfuseObservation[]): Map<string, string> {
  const agents = new Map<string, string>();
  for (const observation of observations) {
    const agent = observation.metadata?.agent;
    if (isRoot(observation) && observation.traceId !== null && typeof agent === "string") {
      agents.set(observation.traceId, agent);
    }
  }
  return agents;
}

function isRoot(observation: LangfuseObservation): boolean {
  return observation.isRootObservation ?? observation.parentObservationId === null;
}

function byStartTime(a: LangfuseObservation, b: LangfuseObservation): number {
  return Date.parse(a.startTime) - Date.parse(b.startTime);
}

/** The shell command of a tool observation whose input is `{"command": "…"}`. */
function commandOf(observation: LangfuseObservation): string | undefined {
  if (!MARKER_TOOLS.has(observation.name ?? "")) return undefined;
  const input = parseJson(observation.input);
  const command = isRecord(input) ? input.command : undefined;
  return typeof command === "string" ? command : undefined;
}

/** Event text of a raw v2 payload: a JSON string is unwrapped, anything else kept raw. */
function textOf(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value !== "string") return JSON.stringify(value);
  const parsed = parseJson(value);
  return typeof parsed === "string" ? parsed : value;
}

function parseJson(value: unknown): unknown {
  if (typeof value !== "string") return value;
  try {
    return JSON.parse(value);
  } catch {
    return value;
  }
}

function cleanMeta(meta: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(meta).filter(([, value]) => value !== null && value !== undefined)
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * The fields of a Langfuse v2 observation (`GET /api/public/v2/observations`,
 * field groups core,basic,io,metadata,trace_context, or core,basic,time for
 * the activity pass) the reader uses. The v2 endpoint returns `input`/`output`
 * as raw strings.
 */
export interface LangfuseObservation {
  id: string;
  traceId: string | null;
  sessionId?: string | null;
  type: string;
  name?: string | null;
  startTime: string;
  /** ISO-8601, field group `core`; null while the observation is still running. */
  endTime?: string | null;
  parentObservationId: string | null;
  isRootObservation?: boolean;
  input?: unknown;
  output?: unknown;
  metadata?: Record<string, unknown>;
  /** ISO-8601: the observation's last write (field group `time`). */
  updatedAt?: string | null;
}

export interface ActivityQuery {
  /** ISO-8601: only observations started at/after it (none: the whole project). */
  fromStartTime?: string;
  /** Only the observations carrying this session: the root of each of its traces. */
  sessionId?: string;
  /** Only the observations of this trace. */
  traceId?: string;
}

export type ObservationQuery = ActivityQuery;

/** What the reader needs from Langfuse: every observation matching the query, all pages. */
export interface LangfuseObservationLister {
  /** Observations with their content (input, output, metadata). */
  listObservations(query: ObservationQuery): Promise<LangfuseObservation[]>;
  /** Observations without content: ids, `sessionId` and `updatedAt` only. */
  listActivity(query: ActivityQuery): Promise<LangfuseObservation[]>;
}

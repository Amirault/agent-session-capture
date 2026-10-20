import type { LangfuseObservation } from "../../adapters/langfuseObservationLister.js";

/**
 * Synthetic Langfuse v2 observations (`GET /api/public/v2/observations`,
 * field groups core,basic,io,metadata,trace_context). Input and output are
 * raw strings, as the v2 endpoint returns them. A real OTLP export (Langfuse
 * Cloud, 2026-10-20) confirmed the string payloads and showed `sessionId` set
 * only on the observation carrying the session, `""` on the others; a real
 * Hermes trace is still to be checked against this shape.
 */
export const anyObservation: LangfuseObservation = {
  id: "obs-any",
  traceId: "tr-any",
  sessionId: "hermes-0419",
  type: "SPAN",
  name: "step",
  startTime: "2026-10-19T09:00:00.000Z",
  parentObservationId: "obs-root",
  isRootObservation: false,
  input: null,
  output: null,
  metadata: {},
};

export function markerCommand(captureId: string, label: string): string {
  return `: CAPTURE_MARKER v=1 id=${captureId} label=${label}`;
}

/** A `TOOL` observation named `terminal` whose input runs `command`. */
export function terminalCall(
  overrides: Partial<LangfuseObservation> & { command: string }
): LangfuseObservation {
  const { command, ...rest } = overrides;
  return {
    ...anyObservation,
    type: "TOOL",
    name: "terminal",
    input: JSON.stringify({ command }),
    ...rest,
  };
}

/** A `GENERATION` observation whose output is the assistant text. */
export function generation(
  overrides: Partial<LangfuseObservation> & { text: string }
): LangfuseObservation {
  const { text, ...rest } = overrides;
  return { ...anyObservation, type: "GENERATION", name: "llm", output: JSON.stringify(text), ...rest };
}

/** The root observation of a trace, carrying the user's prompt as input. */
export function traceRoot(
  overrides: Partial<LangfuseObservation> & { prompt: string }
): LangfuseObservation {
  const { prompt, ...rest } = overrides;
  return {
    ...anyObservation,
    type: "AGENT",
    name: "hermes-run",
    parentObservationId: null,
    isRootObservation: true,
    input: JSON.stringify(prompt),
    ...rest,
  };
}

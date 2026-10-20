import { DEFAULT_LABEL } from "./models.js";
import type { Label } from "./models.js";

/** One canonical `: CAPTURE_MARKER v=1 id=<id> [label=<label>]` emission. */
export interface ExactMarker {
  captureId: string;
  label: Label;
}

/**
 * The exact markers a shell command ran: every line of `command` that is, once
 * trimmed, the canonical marker line and nothing else. A marker quoted inside
 * another command (`echo ': CAPTURE_MARKER …'`) is a mention, not an emission.
 * Shared by the readers whose sources record the tool call's raw command
 * (Hermes, Langfuse).
 */
export function exactMarkersInCommand(command: string): ExactMarker[] {
  const markers: ExactMarker[] = [];
  for (const line of command.split("\n")) {
    const marker = parseExactMarkerLine(line.trim());
    if (marker !== null) markers.push(marker);
  }
  return markers;
}

function parseExactMarkerLine(line: string): ExactMarker | null {
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

/**
 * Strip ANSI escape sequences from a styled Warp block buffer.
 *
 * Warp stores block command/output text "ANSI-exploded" (e.g. per-character bold:
 * `\x1b[1mH\x1b[0m\x1b[1mi\x1b[0m`). We recover the clean text by walking bytes
 * and dropping escape sequences — a state machine, not a regex.
 */
export function stripAnsi(input: Buffer | string | null | undefined): string {
  if (input === null || input === undefined) return "";
  const buf = typeof input === "string" ? Buffer.from(input, "utf8") : input;
  const out: number[] = [];
  let i = 0;
  const CSI = 0x5b; // '['
  const OSC = 0x5d; // ']'
  const BEL = 0x07; // BEL terminates some OSC sequences
  const ESC = 0x1b;

  while (i < buf.length) {
    const b = buf[i]!;
    if (b === ESC) {
      i += 1;
      const next = buf[i];
      if (next === CSI) {
        // CSI: ESC [ params... final byte in 0x40..0x7e
        i += 1;
        while (i < buf.length) {
          const c = buf[i]!;
          i += 1;
          if (c >= 0x40 && c <= 0x7e) break;
        }
      } else if (next === OSC) {
        // OSC: ESC ] ... terminated by BEL or ST (ESC \)
        i += 1;
        while (i < buf.length) {
          const c = buf[i]!;
          if (c === BEL) {
            i += 1;
            break;
          }
          if (c === ESC && buf[i + 1] === 0x5c) {
            i += 2;
            break;
          }
          i += 1;
        }
      } else {
        // Two-character escape (ESC x) — skip the one following byte if present.
        if (i < buf.length) i += 1;
      }
    } else if (b === BEL) {
      // Stray BEL outside an OSC — drop.
      i += 1;
    } else {
      out.push(b);
      i += 1;
    }
  }
  return Buffer.from(out).toString("utf8");
}

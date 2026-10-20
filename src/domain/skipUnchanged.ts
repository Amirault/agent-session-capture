import type { SessionInfo } from "./models.js";
import type { ReadSessionsOptions } from "./ports.js";

/**
 * Skip-unchanged, shared by every SessionReader: gather unless the stored
 * header's stamp equals the fresh stamp (raw string equality — the stamps
 * never change representation). New conversations, `null` stamps (no durable
 * stamp — always re-extract) and the `--conversation` debug filter are always
 * gathered.
 */
export function shouldGather(info: SessionInfo, options: ReadSessionsOptions): boolean {
  if (options.conversation === info.conversation_id) return true;
  if (info.last_modified === null) return true;
  const stored = options.storedByCid.get(info.conversation_id);
  return stored === undefined || stored.last_modified !== info.last_modified;
}

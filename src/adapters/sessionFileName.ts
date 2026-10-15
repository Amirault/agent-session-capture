import { createHash } from "node:crypto";

/** Characters that never need escaping in a filename (Warp UUIDs qualify). */
const SAFE_SESSION_FILE_RE = /^[A-Za-z0-9._-]+$/;

/**
 * Map a conversation id to its `sessions/<file>.jsonl` basename.
 *
 * Pass-through when the id is already a safe single path segment
 * (`[A-Za-z0-9._-]+`, not `.` / `..`). Otherwise sanitize disallowed
 * characters to `-` and append `-<first 8 hex of sha1(cid)>`: deterministic, so
 * the same id always maps to the same file, and collision-proof, so two
 * different unsafe ids can never sanitize onto the same name. The result can
 * never be `.` or `..` and never contains a path separator, so it cannot
 * escape the sessions/ directory.
 */
export function sessionFileName(conversationId: string): string {
  if (
    SAFE_SESSION_FILE_RE.test(conversationId) &&
    conversationId !== "." &&
    conversationId !== ".."
  ) {
    return conversationId;
  }
  const sanitized = conversationId.replace(/[^A-Za-z0-9._-]/g, "-");
  const digest = createHash("sha1").update(conversationId, "utf8").digest("hex").slice(0, 8);
  return `${sanitized}-${digest}`;
}

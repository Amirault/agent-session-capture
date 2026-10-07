import { z } from "zod";

/**
 * zod schemas for the JSON columns Warp stores. Parsing is lenient-on-shape:
 * unknown extra keys are passed through (we export raw), but the fields we bind
 * on must be present and correctly typed. Rows that fail parsing are logged and
 * skipped — never silently dropped, never crash the run.
 */

/**
 * ai_queries.input — a JSON array whose first element carries the user's prompt
 * text under Query.text (verified shape from the live DB):
 *   [{"Query":{"text":"...","context":...}}]
 */
export const QueryInputSchema = z
  .array(
    z.looseObject({
      Query: z.looseObject({ text: z.string() }),
    })
  )
  .min(1);

/** blocks.ai_metadata — binds a block to a conversation (and tags subagent work). */
export const BlockAiMetadataSchema = z.looseObject({
  conversation_id: z.string().nullable().optional(),
  subagent_task_id: z.string().nullable().optional(),
});

/** Parsed, typed views for downstream code. */
export type QueryInput = z.infer<typeof QueryInputSchema>;
export type BlockAiMetadata = z.infer<typeof BlockAiMetadataSchema>;

/** Uniform result of a safe parse: the data, or an error message (never throws). */
export type ParseResult<T> = { ok: true; data: T } | { ok: false; error: string };

/** JSON.parse then schema-validate, collapsing both failure modes into ParseResult. */
function parseJson<T>(raw: string, schema: z.ZodType<T>): ParseResult<T> {
  let json: unknown;
  try {
    json = JSON.parse(raw);
  } catch (e) {
    return { ok: false, error: `invalid JSON: ${(e as Error).message}` };
  }
  const parsed = schema.safeParse(json);
  return parsed.success
    ? { ok: true, data: parsed.data }
    : { ok: false, error: parsed.error.message };
}

export function parseQueryInput(raw: string): ParseResult<QueryInput> {
  return parseJson(raw, QueryInputSchema);
}

export function parseBlockAiMetadata(raw: string | null): ParseResult<BlockAiMetadata> | null {
  if (raw === null) return null;
  return parseJson(raw, BlockAiMetadataSchema);
}

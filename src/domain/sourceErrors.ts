/**
 * The source could not be read at all (access refused, server error,
 * unreachable). A stored bundle must not mask it: the use cases rethrow it
 * instead of degrading to stored-only, so nothing is rewritten and the CLI
 * exits 1. The message names what failed (e.g. HTTP status and host) and never
 * carries a secret.
 */
export class SourceAccessError extends Error {
  override name = "SourceAccessError";
}

/** The source is not configured (e.g. credentials missing): a usage error, the CLI exits 2. */
export class SourceConfigurationError extends SourceAccessError {
  override name = "SourceConfigurationError";
}

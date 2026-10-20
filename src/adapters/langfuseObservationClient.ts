import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { LangfuseClient } from "@langfuse/client";
import { SourceAccessError, SourceConfigurationError } from "../domain/sourceErrors.js";
import type {
  ActivityQuery,
  LangfuseObservation,
  LangfuseObservationLister,
  ObservationQuery,
} from "./langfuseObservationLister.js";

export interface LangfuseCredentials {
  host: string;
  publicKey: string;
  secretKey: string;
}

export interface LangfuseCredentialOptions {
  /** Environment to read first (default `process.env`). */
  env?: Record<string, string | undefined>;
  /** `--langfuse-env <file>`: read after the environment. */
  envFile?: string;
  /** Home holding `.config/langfuse/capture.env`, read last (default `os.homedir()`). */
  homeDir?: string;
}

const DEFAULT_HOST = "https://cloud.langfuse.com";
const CONTENT_FIELDS = "core,basic,io,metadata,trace_context";
const ACTIVITY_FIELDS = "core,basic,time";
const PAGE_LIMIT = 1000;
const MISSING_CREDENTIALS =
  "Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)";

/**
 * Resolve each `LANGFUSE_*` variable from the environment, else the
 * `--langfuse-env` file, else `~/.config/langfuse/capture.env`. The host
 * defaults to Langfuse Cloud; a missing key is a configuration error whose
 * message names the variables, never a value.
 */
export function resolveLangfuseCredentials(options: LangfuseCredentialOptions = {}): LangfuseCredentials {
  const layers = [
    options.env ?? process.env,
    options.envFile === undefined ? {} : readEnvFile(options.envFile),
    readEnvFile(path.join(options.homeDir ?? os.homedir(), ".config", "langfuse", "capture.env")),
  ];
  const value = (name: string) =>
    layers.map((layer) => layer[name]?.trim()).find((v) => v !== undefined && v.length > 0);

  const publicKey = value("LANGFUSE_PUBLIC_KEY");
  const secretKey = value("LANGFUSE_SECRET_KEY");
  if (publicKey === undefined || secretKey === undefined) {
    throw new SourceConfigurationError(MISSING_CREDENTIALS);
  }
  return { host: value("LANGFUSE_HOST") ?? DEFAULT_HOST, publicKey, secretKey };
}

/** Dotenv-style `KEY=value` lines (optional `export `, quotes, `#` comments); a missing file is empty. */
function readEnvFile(filePath: string): Record<string, string> {
  if (!fs.existsSync(filePath)) return {};
  const entries: Record<string, string> = {};
  for (const rawLine of fs.readFileSync(filePath, "utf8").split("\n")) {
    const line = rawLine.trim().replace(/^export\s+/, "");
    const separator = line.indexOf("=");
    if (line.startsWith("#") || separator <= 0) continue;
    entries[line.slice(0, separator).trim()] = line.slice(separator + 1).trim().replace(/^(['"])(.*)\1$/, "$2");
  }
  return entries;
}

/**
 * The only importer of `@langfuse/client`: reads observations through the v2
 * endpoint (`api.observations.getMany`), read-only. Credentials resolve on the
 * first call, so building the client needs none. Every API or network failure
 * becomes a `SourceAccessError` naming the HTTP status (or "unreachable") and
 * the host.
 */
export class LangfuseObservationClient implements LangfuseObservationLister {
  constructor(private readonly credentialOptions: LangfuseCredentialOptions = {}) {}

  listObservations(query: ObservationQuery): Promise<LangfuseObservation[]> {
    return this.allPages({ fields: CONTENT_FIELDS, ...query });
  }

  listActivity(query: ActivityQuery): Promise<LangfuseObservation[]> {
    return this.allPages({ fields: ACTIVITY_FIELDS, ...query });
  }

  /**
   * Follows the cursor until a page carries none. The SDK passes the raw body
   * through unvalidated, so a `null` or `""` cursor also ends the listing
   * (sending it back would restart at the first page).
   */
  private async allPages(filters: ObservationQuery & { fields: string }): Promise<LangfuseObservation[]> {
    const { host, publicKey, secretKey } = resolveLangfuseCredentials(this.credentialOptions);
    const langfuse = new LangfuseClient({ publicKey, secretKey, baseUrl: host });
    const observations: LangfuseObservation[] = [];
    let cursor: string | null | undefined;
    do {
      const page = await langfuse.api.observations
        .getMany({ ...filters, limit: PAGE_LIMIT, cursor: cursor ?? undefined })
        .catch((error: unknown) => {
          throw accessError(error, host);
        });
      observations.push(...page.data);
      cursor = page.meta.cursor;
    } while (cursor !== undefined && cursor !== null && cursor !== "");
    return observations;
  }
}

function accessError(error: unknown, host: string): SourceAccessError {
  const status = (error as { statusCode?: unknown }).statusCode;
  if (typeof status === "number") return new SourceAccessError(`Langfuse API ${status} at ${host}`);
  const detail = error instanceof Error ? error.message : String(error);
  return new SourceAccessError(`Langfuse API unreachable at ${host} (${detail})`);
}

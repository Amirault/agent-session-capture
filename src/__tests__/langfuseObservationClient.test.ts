import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import {
  LangfuseObservationClient,
  resolveLangfuseCredentials,
} from "../adapters/langfuseObservationClient.js";
import { SourceAccessError, SourceConfigurationError } from "../domain/sourceErrors.js";
import { anyObservation } from "./fixtures/langfuseObservations.js";
import {
  startFakeLangfuse,
  TEST_PUBLIC_KEY,
  TEST_SECRET_KEY,
  type FakeLangfuseServer,
} from "./fixtures/fakeLangfuseServer.js";

const MISSING_CREDENTIALS =
  "Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)";

let home: string;
let server: FakeLangfuseServer | undefined;

beforeEach(() => {
  home = fs.mkdtempSync(path.join(os.tmpdir(), "lf-client-"));
});
afterEach(async () => {
  await server?.close();
  server = undefined;
  fs.rmSync(home, { recursive: true, force: true });
});

function writeEnvFile(filePath: string, lines: string[]): string {
  fs.mkdirSync(path.dirname(filePath), { recursive: true });
  fs.writeFileSync(filePath, `${lines.join("\n")}\n`);
  return filePath;
}

function defaultEnvFile(): string {
  return path.join(home, ".config", "langfuse", "capture.env");
}

function clientFor(url: string): LangfuseObservationClient {
  return new LangfuseObservationClient({
    env: { LANGFUSE_HOST: url, LANGFUSE_PUBLIC_KEY: TEST_PUBLIC_KEY, LANGFUSE_SECRET_KEY: TEST_SECRET_KEY },
    homeDir: home,
  });
}

describe("resolveLangfuseCredentials", () => {
  it("Given the three variables in the environment, When resolving, Then they are used as given", () => {
    // Given
    const env = { LANGFUSE_HOST: "https://eu.example", LANGFUSE_PUBLIC_KEY: "pk", LANGFUSE_SECRET_KEY: "sk" };

    // When
    const credentials = resolveLangfuseCredentials({ env, homeDir: home });

    // Then
    expect(credentials).toEqual({ host: "https://eu.example", publicKey: "pk", secretKey: "sk" });
  });

  it("Given keys without LANGFUSE_HOST, When resolving, Then the host defaults to Langfuse Cloud", () => {
    // Given
    const env = { LANGFUSE_PUBLIC_KEY: "pk", LANGFUSE_SECRET_KEY: "sk" };

    // When
    const credentials = resolveLangfuseCredentials({ env, homeDir: home });

    // Then
    expect(credentials.host).toBe("https://cloud.langfuse.com");
  });

  it("Given keys only in ~/.config/langfuse/capture.env, When resolving, Then the file's dotenv lines are used", () => {
    // Given
    writeEnvFile(defaultEnvFile(), [
      "# capture keys",
      "export LANGFUSE_PUBLIC_KEY=pk-file",
      'LANGFUSE_SECRET_KEY="sk-file"',
      "",
      "LANGFUSE_HOST='https://file.example'",
    ]);

    // When
    const credentials = resolveLangfuseCredentials({ env: {}, homeDir: home });

    // Then
    expect(credentials).toEqual({ host: "https://file.example", publicKey: "pk-file", secretKey: "sk-file" });
  });

  it("Given a variable in the environment, in --langfuse-env and in the default file, When resolving, Then the environment wins, then --langfuse-env, then the default file", () => {
    // Given
    writeEnvFile(defaultEnvFile(), [
      "LANGFUSE_HOST=https://default.example",
      "LANGFUSE_PUBLIC_KEY=pk-default",
      "LANGFUSE_SECRET_KEY=sk-default",
    ]);
    const envFile = writeEnvFile(path.join(home, "other.env"), [
      "LANGFUSE_PUBLIC_KEY=pk-explicit",
      "LANGFUSE_SECRET_KEY=sk-explicit",
    ]);

    // When
    const credentials = resolveLangfuseCredentials({
      env: { LANGFUSE_SECRET_KEY: "sk-env" },
      envFile,
      homeDir: home,
    });

    // Then
    expect(credentials).toEqual({
      host: "https://default.example",
      publicKey: "pk-explicit",
      secretKey: "sk-env",
    });
  });

  it.each([
    ["no key anywhere", {}],
    ["only the public key", { LANGFUSE_PUBLIC_KEY: "pk-only" }],
    ["only the secret key", { LANGFUSE_SECRET_KEY: "sk-only" }],
    ["empty keys", { LANGFUSE_PUBLIC_KEY: "", LANGFUSE_SECRET_KEY: " " }],
  ])("Given %s (Example 10), When resolving, Then it fails as a configuration error naming the three variables", (_, env) => {
    // When
    const resolve = () => resolveLangfuseCredentials({ env, homeDir: home });

    // Then
    expect(resolve).toThrow(new SourceConfigurationError(MISSING_CREDENTIALS));
  });
});

describe("LangfuseObservationClient (against a loopback fake of the v2 endpoint)", () => {
  it("Given 2,350 observations served 1,000 per page (Example 17), When listing, Then it follows the cursor over 3 pages and returns every observation once, in order", async () => {
    // Given
    const observations = Array.from({ length: 2350 }, (_, i) => ({ ...anyObservation, id: `obs-${i}` }));
    server = await startFakeLangfuse({ observations });

    // When
    const listed = await clientFor(server.url).listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    expect(listed.map((o) => o.id)).toEqual(observations.map((o) => o.id));
    expect(server.requests.map((r) => r.query.cursor)).toEqual([undefined, "page-1000", "page-2000"]);
  });

  it("Given credentials, When listing, Then it queries the v2 observations endpoint authenticated, with the marker-mode field groups, page size and start cutoff", async () => {
    // Given
    server = await startFakeLangfuse({});

    // When
    await clientFor(server.url).listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    expect(server.requests).toEqual([
      {
        path: "/api/public/v2/observations",
        query: {
          fields: "core,basic,io,metadata,trace_context",
          limit: "1000",
          fromStartTime: "2026-09-19T12:00:00.000Z",
        },
        authorized: true,
      },
    ]);
  });

  it.each([
    ["null", { cursor: null }],
    ["an empty string", { cursor: "" }],
  ])("Given a last page answering cursor %s, When listing, Then it stops after that page", async (_, lastPageMeta) => {
    // Given
    const observations = Array.from({ length: 3 }, (_, i) => ({ ...anyObservation, id: `obs-${i}` }));
    server = await startFakeLangfuse({ observations, pageSize: 2, lastPageMeta, maxRequests: 5 });

    // When
    const listed = await clientFor(server.url).listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    expect(listed.map((o) => o.id)).toEqual(["obs-0", "obs-1", "obs-2"]);
    expect(server.requests.map((r) => r.query.cursor)).toEqual([undefined, "page-2"]);
  });

  it("Given a trace id, When listing its observations, Then it queries the v2 endpoint with the traceId filter and the content field groups, without start cutoff", async () => {
    // Given
    server = await startFakeLangfuse({
      observations: [
        { ...anyObservation, id: "obs-a", traceId: "tr-a" },
        { ...anyObservation, id: "obs-b", traceId: "tr-b" },
      ],
    });

    // When
    const listed = await clientFor(server.url).listObservations({ traceId: "tr-a" });

    // Then
    expect(listed.map((o) => o.id)).toEqual(["obs-a"]);
    expect(server.requests.map((r) => r.query)).toEqual([
      { fields: "core,basic,io,metadata,trace_context", limit: "1000", traceId: "tr-a" },
    ]);
  });

  it.each([
    [{ fromStartTime: "2026-10-01T00:00:00.000Z" }, { fromStartTime: "2026-10-01T00:00:00.000Z" }],
    [{ sessionId: "hermes-0419" }, { sessionId: "hermes-0419" }],
    [{}, {}],
  ])("Given the activity query %j, When listing activity, Then it queries the v2 endpoint with the core,basic,time field groups only", async (query, cutoff) => {
    // Given
    server = await startFakeLangfuse({});

    // When
    await clientFor(server.url).listActivity(query);

    // Then
    expect(server.requests).toEqual([
      {
        path: "/api/public/v2/observations",
        query: { fields: "core,basic,time", limit: "1000", ...cutoff },
        authorized: true,
      },
    ]);
  });

  it("Given an API answering 401 (Example 11), When listing, Then it fails as a source access error naming the status and the host, without any key", async () => {
    // Given
    server = await startFakeLangfuse({ failWith: 401 });

    // When
    const listing = clientFor(server.url).listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    await expect(listing).rejects.toThrow(new SourceAccessError(`Langfuse API 401 at ${server.url}`));
  });

  it("Given an API answering 503 (Example 11), When listing, Then it fails as a source access error naming the status and the host", async () => {
    // Given
    server = await startFakeLangfuse({ failWith: 503 });

    // When
    const listing = clientFor(server.url).listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    await expect(listing).rejects.toThrow(new SourceAccessError(`Langfuse API 503 at ${server.url}`));
  }, 10_000);

  it("Given a host nothing listens on, When listing, Then it fails as a source access error naming the host", async () => {
    // Given
    const closed = await startFakeLangfuse({});
    await closed.close();

    // When
    const listing = clientFor(closed.url).listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    await expect(listing).rejects.toThrow(
      expect.objectContaining({
        name: "SourceAccessError",
        message: expect.stringMatching(new RegExp(`^Langfuse API unreachable at ${closed.url}`)),
      })
    );
  });

  it("Given no credentials, When listing, Then it fails as a configuration error before any request", async () => {
    // Given
    server = await startFakeLangfuse({});
    const client = new LangfuseObservationClient({ env: { LANGFUSE_HOST: server.url }, homeDir: home });

    // When
    const listing = client.listObservations({ fromStartTime: "2026-09-19T12:00:00.000Z" });

    // Then
    await expect(listing).rejects.toThrow(new SourceConfigurationError(MISSING_CREDENTIALS));
    expect(server.requests).toEqual([]);
  });
});

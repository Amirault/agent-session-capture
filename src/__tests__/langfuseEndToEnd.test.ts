import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import type { LangfuseObservation } from "../adapters/langfuseObservationLister.js";
import { computeBundleHeader } from "../domain/bundleHeader.js";
import type { BundleSource } from "../domain/models.js";
import { generation, markerCommand, terminalCall, traceRoot } from "./fixtures/langfuseObservations.js";
import {
  startFakeLangfuse,
  TEST_PUBLIC_KEY,
  TEST_SECRET_KEY,
  type FakeLangfuseServer,
} from "./fixtures/fakeLangfuseServer.js";

const TEST_DIR = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(TEST_DIR, "..", "..");
const TSX = path.join(ROOT, "node_modules", ".bin", "tsx");
const CAPTURE_ID = "checkout-bug";

interface CliResult {
  stdout: string;
  stderr: string;
  exitCode: number;
}

let tmp: string;
let store: string;
let server: FakeLangfuseServer | undefined;

beforeEach(() => {
  tmp = fs.mkdtempSync(path.join(os.tmpdir(), "lf-e2e-"));
  store = path.join(tmp, ".agent-captures");
});
afterEach(async () => {
  await server?.close();
  server = undefined;
  fs.rmSync(tmp, { recursive: true, force: true });
});

/** Environment without any real LANGFUSE_* value nor the user's ~/.config/langfuse/capture.env. */
function isolatedEnv(langfuse: Record<string, string> = {}): NodeJS.ProcessEnv {
  const env = Object.fromEntries(
    Object.entries(process.env).filter(([name]) => !name.startsWith("LANGFUSE_"))
  );
  return { ...env, HOME: tmp, ...langfuse };
}

function testKeys(host: string): Record<string, string> {
  return { LANGFUSE_HOST: host, LANGFUSE_PUBLIC_KEY: TEST_PUBLIC_KEY, LANGFUSE_SECRET_KEY: TEST_SECRET_KEY };
}

/** Async spawn: the loopback fake answers from this process while the CLI runs. */
function runCli(args: string[], env: NodeJS.ProcessEnv): Promise<CliResult> {
  return new Promise((resolve) => {
    const child = spawn(TSX, ["src/cli.ts", ...args], { cwd: ROOT, env });
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => (stdout += chunk));
    child.stderr.on("data", (chunk) => (stderr += chunk));
    child.on("close", (code) => resolve({ stdout, stderr, exitCode: code ?? 1 }));
  });
}

function implementSession(): LangfuseObservation[] {
  return [
    traceRoot({ id: "obs-1", prompt: "implement increment 4", startTime: "2026-10-19T09:00:00.000Z" }),
    terminalCall({ id: "obs-91", command: markerCommand(CAPTURE_ID, "implement"), startTime: "2026-10-19T09:00:01.000Z" }),
    generation({ id: "obs-2", text: "I will read the spec first", startTime: "2026-10-19T09:00:02.000Z" }),
  ].map((o) => ({ ...o, startTime: new Date().toISOString() }));
}

/** A stored specify-label bundle of `source` with one event, at `<dir>/<id>.jsonl`. */
function seedStoredBundle(dir: string, source: BundleSource): { bundlePath: string; content: string } {
  const bundlePath = path.join(dir, `${CAPTURE_ID}.jsonl`);
  const header = {
    type: "bundle_header",
    ...computeBundleHeader(CAPTURE_ID, source, new Map([["c-specify", "specify"]])),
    extracted_at: "2026-10-19T08:00:00.000Z",
  };
  const event = {
    capture_id: CAPTURE_ID, label: "specify", conversation_id: "c-specify", seq: 1,
    ts: "2026-10-19T08:00:00.000Z", role: "user", kind: "query", content: "specify it", meta: {},
  };
  const content = `${JSON.stringify(header)}\n${JSON.stringify(event)}\n`;
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(bundlePath, content);
  return { bundlePath, content };
}

describe("end-to-end --id --source langfuse (CLI -> client -> loopback v2 endpoint -> sink)", () => {
  it("Given a local claude-code bundle and a Langfuse session bound to the capture id (Examples 5 and 12), When the CLI captures into the langfuse sub-store, Then it writes the Langfuse bundle and leaves the local bundle byte-identical", async () => {
    // Given
    const local = seedStoredBundle(store, "claude-code");
    server = await startFakeLangfuse({ observations: implementSession() });
    const langfuseStore = path.join(store, "langfuse");

    // When
    const result = await runCli(
      ["--id", CAPTURE_ID, "--source", "langfuse", "--out", langfuseStore],
      isolatedEnv(testKeys(server.url))
    );

    // Then
    const outPath = path.join(langfuseStore, `${CAPTURE_ID}.jsonl`);
    expect(result).toMatchObject({
      exitCode: 0,
      stdout: `wrote ${outPath} (3 events, 1 conversations)\n`,
    });
    expect(JSON.parse(fs.readFileSync(outPath, "utf8").split("\n")[0]!)).toMatchObject({
      source: "langfuse",
      labels: ["implement"],
    });
    expect(fs.readFileSync(local.bundlePath, "utf8")).toBe(local.content);
  });

  it("Given no LANGFUSE_* variable, no ~/.config/langfuse/capture.env and no --langfuse-env (Example 10), When the CLI runs, Then it exits 2 naming the three variables", async () => {
    // When
    const result = await runCli(["--id", CAPTURE_ID, "--source", "langfuse", "--out", store], isolatedEnv());

    // Then
    expect(result).toEqual({
      exitCode: 2,
      stdout: "",
      stderr:
        "error: Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)\n",
    });
  });

  it("Given only the public key, When the CLI runs, Then it exits 2 and never prints the key", async () => {
    // When
    const result = await runCli(
      ["--id", CAPTURE_ID, "--source", "langfuse", "--out", store],
      isolatedEnv({ LANGFUSE_PUBLIC_KEY: TEST_PUBLIC_KEY })
    );

    // Then
    expect(result.exitCode).toBe(2);
    expect(result.stdout + result.stderr).not.toContain(TEST_PUBLIC_KEY);
  });

  it("Given keys only in the --langfuse-env file, When the CLI runs, Then it reads Langfuse with them", async () => {
    // Given
    server = await startFakeLangfuse({ observations: implementSession() });
    const envFile = path.join(tmp, "capture.env");
    fs.writeFileSync(
      envFile,
      Object.entries(testKeys(server.url)).map(([name, value]) => `${name}=${value}`).join("\n")
    );

    // When
    const result = await runCli(
      ["--id", CAPTURE_ID, "--source", "langfuse", "--out", store, "--langfuse-env", envFile],
      isolatedEnv()
    );

    // Then
    expect(result.exitCode).toBe(0);
    expect(server.requests.map((r) => r.authorized)).toEqual([true]);
  });

  it("Given a stored Langfuse bundle and an API answering 401 (Example 11), When the CLI runs, Then it exits 1 with the status and host, leaves the bundle untouched and prints no key", async () => {
    // Given
    server = await startFakeLangfuse({ failWith: 401 });
    const stored = seedStoredBundle(store, "langfuse");

    // When
    const result = await runCli(
      ["--id", CAPTURE_ID, "--source", "langfuse", "--out", store],
      isolatedEnv(testKeys(server.url))
    );

    // Then
    expect(result).toEqual({ exitCode: 1, stdout: "", stderr: `error: Langfuse API 401 at ${server.url}\n` });
    expect(fs.readFileSync(stored.bundlePath, "utf8")).toBe(stored.content);
  });
});

/** Example 17: session `hermes-0421`, one trace of 2,350 observations (root + 2,349 answers). */
function largeSession(): LangfuseObservation[] {
  const start = Date.parse("2026-10-18T09:00:00.000Z");
  const at = (i: number) => new Date(start + i * 1000).toISOString();
  return [
    traceRoot({ id: "obs-0", traceId: "tr-0421", sessionId: "hermes-0421", prompt: "answer 0", startTime: at(0), updatedAt: at(0) }),
    ...Array.from({ length: 2349 }, (_, k) =>
      generation({
        id: `obs-${k + 1}`,
        traceId: "tr-0421",
        sessionId: "",
        text: `answer ${k + 1}`,
        startTime: at(k + 1),
        updatedAt: at(k + 1),
      })
    ),
  ];
}

/** A stored Langfuse session file of `cid` with one event, at `<dir>/sessions/<cid>.jsonl`. */
function seedStoredSession(dir: string, cid: string): { sessionPath: string; content: string } {
  const sessionPath = path.join(dir, "sessions", `${cid}.jsonl`);
  const header = {
    type: "session_header", conversation_id: cid, source: "langfuse",
    last_modified: "2026-10-17T14:02:11.000Z", extracted_at: "2026-10-18T00:00:00.000Z",
  };
  const event = {
    conversation_id: cid, seq: 1, ts: "2026-10-17T14:00:00.000Z", role: "user", kind: "query", content: "run", meta: {},
  };
  const content = `${JSON.stringify(header)}\n${JSON.stringify(event)}\n`;
  fs.mkdirSync(path.dirname(sessionPath), { recursive: true });
  fs.writeFileSync(sessionPath, content);
  return { sessionPath, content };
}

describe("end-to-end --sessions --source langfuse (CLI -> client -> loopback v2 endpoint -> sink)", () => {
  it("Given a session of 2,350 observations served 1,000 per page (Example 17), When the CLI exports it with --conversation, Then it follows the cursor and writes every mapped event once, ordered by ts", async () => {
    // Given
    server = await startFakeLangfuse({ observations: largeSession() });
    const langfuseStore = path.join(store, "langfuse");

    // When
    const result = await runCli(
      ["--sessions", "--source", "langfuse", "--conversation", "hermes-0421", "--out", langfuseStore],
      isolatedEnv(testKeys(server.url))
    );

    // Then
    const outPath = path.join(langfuseStore, "sessions", "hermes-0421.jsonl");
    expect(result).toMatchObject({
      exitCode: 0,
      stdout: `wrote ${outPath} (2350 events)\n1 sessions: 1 written, 0 unchanged, 0 untouched\n`,
    });
    const [header, ...events] = fs.readFileSync(outPath, "utf8").trimEnd().split("\n").map((line) => JSON.parse(line));
    expect(header).toMatchObject({ conversation_id: "hermes-0421", source: "langfuse", last_modified: "2026-10-18T09:39:09.000Z" });
    expect(events.map((e) => [e.seq, e.content])).toEqual(
      Array.from({ length: 2350 }, (_, i) => [i + 1, `answer ${i}`])
    );
    expect(server.requests.map((r) => [r.query.fields, r.query.traceId, r.query.cursor])).toEqual([
      ["core,basic,time", undefined, undefined],
      ["core,basic,time", undefined, "page-1000"],
      ["core,basic,time", undefined, "page-2000"],
      ["core,basic,io,metadata,trace_context", "tr-0421", undefined],
      ["core,basic,io,metadata,trace_context", "tr-0421", "page-1000"],
      ["core,basic,io,metadata,trace_context", "tr-0421", "page-2000"],
    ]);
  });

  it("Given no LANGFUSE_* variable, no ~/.config/langfuse/capture.env and no --langfuse-env (Example 10), When the CLI runs --sessions, Then it exits 2 naming the three variables and writes nothing", async () => {
    // When
    const result = await runCli(["--sessions", "--source", "langfuse", "--out", store], isolatedEnv());

    // Then
    expect(result).toEqual({
      exitCode: 2,
      stdout: "",
      stderr:
        "error: Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)\n",
    });
    expect(fs.existsSync(store)).toBe(false);
  });

  it("Given a stored Langfuse session file and an API answering 401 (Example 11), When the CLI runs --sessions, Then it exits 1 with the status and host and leaves the file untouched", async () => {
    // Given
    server = await startFakeLangfuse({ failWith: 401 });
    const stored = seedStoredSession(store, "hermes-0419");

    // When
    const result = await runCli(["--sessions", "--source", "langfuse", "--out", store], isolatedEnv(testKeys(server.url)));

    // Then
    expect(result).toEqual({ exitCode: 1, stdout: "", stderr: `error: Langfuse API 401 at ${server.url}\n` });
    expect(fs.readFileSync(stored.sessionPath, "utf8")).toBe(stored.content);
  });
});

describe("capture.sh store for --source langfuse", () => {
  /** A copy of capture.sh in a non-git tree whose tsx prints the arguments it receives. */
  function captureCopy(): string {
    const toolDir = path.join(tmp, "tool");
    fs.mkdirSync(path.join(toolDir, "node_modules", ".bin"), { recursive: true });
    fs.copyFileSync(path.join(ROOT, "capture.sh"), path.join(toolDir, "capture.sh"));
    fs.writeFileSync(path.join(toolDir, "node_modules", ".bin", "tsx"), '#!/bin/sh\nprintf "%s\\n" "$@"\n', {
      mode: 0o755,
    });
    return toolDir;
  }

  it.each([
    [["--id", CAPTURE_ID, "--source", "langfuse"], ".agent-captures/langfuse"],
    [["--id", CAPTURE_ID, "--source=langfuse"], ".agent-captures/langfuse"],
    [["--id", CAPTURE_ID, "--source", "claude-code"], ".agent-captures"],
    [["--sessions", "--source", "langfuse"], ".agent-captures/langfuse"],
  ])("Given capture.sh %j without --out (Example 12), When it runs, Then the default store is %s", (captureArgs, expectedStore) => {
    // Given — run from a directory outside any git checkout, so the store is ./.agent-captures
    const toolDir = captureCopy();
    const project = fs.realpathSync(tmp);

    // When
    const result = spawnSync("/bin/bash", [path.join(toolDir, "capture.sh"), ...captureArgs], {
      cwd: project,
      encoding: "utf8",
      env: { PATH: "/usr/bin:/bin", HOME: tmp },
    });

    // Then
    const args = result.stdout.trim().split("\n");
    expect(args.slice(-2)).toEqual(["--out", path.join(project, expectedStore)]);
  });
});

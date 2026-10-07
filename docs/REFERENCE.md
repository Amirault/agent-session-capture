# Reference

Reads local agent session stores and exports **every conversation tagged
with one capture id** — across sessions and subagents — as raw JSONL, ordered
chronologically while preserving each conversation's original message order, ready for
whatever analysis comes next
(prompt quality, agent decisions, execution time, …).

Supported sources:

- `warp`: Warp's local SQLite database.
- `claude-code`: Claude Code JSONL transcripts under `~/.claude/projects`.
- `hermes`: Hermes' canonical `<HERMES_HOME>/state.db` SQLite store.

It is schema-less where a source schema is opaque (notably Warp's protobuf
`agent_tasks.task` blob) and read-only against the live stores.

---

## What it does

Given a capture id (any token, e.g. `JIRA-123` or `fix-login-bug`), the adapter:

1. **Finds correlation markers** — `: CAPTURE_MARKER v=1 id=<id> [label=<label>]`
   shell no-ops emitted during the sessions (see [MARKER.md](MARKER.md)) — and binds each
   to a conversation. `label` is an optional free-form tag (default `default`):
   - Warp: marker command in `commands.command`, conversation id via the matching
     `blocks.start_ts` row.
   - Claude Code: marker command in a `Bash` `tool_use` block inside the session
     JSONL transcript.
   - Hermes: an exact marker line in an assistant `terminal` or
     `run_shell_command` tool call stored in `messages.tool_calls`.
2. **Reads every event** for those conversations from the selected source:
   - Claude Code `type: "user"` entries → prompts and tool results
     (`kind: "query"` / `kind: "tool_result"`)
   - Claude Code `type: "assistant"` content blocks → assistant text and tool
     calls (`kind: "agent_message"` / `kind: "tool_call"`)
   - Hermes `user`, `assistant`, and `tool` messages → prompts, assistant text,
     tool calls, and tool results. Inactive compacted rows are retained with
     `active` / `compacted` metadata so the original learning history is not lost.
   - Warp reads three tables:
     - `ai_queries` → user prompts (`kind: "query"`)
     - `blocks` → shell command executions (`kind: "command"`), incl. subagent blocks
     - `agent_tasks` → assistant text recovered by walking the protobuf `task` blob
       (`kind: "agent_message"`)
3. **Classifies** each event by its conversation's label, orders the whole bundle
   chronologically while preserving each conversation's original message order,
   and assigns a monotonic `seq`.
4. **Compacts** the walked protobuf nodes (drop base64 residue + UUIDs, dedupe
   repeats, truncate long values) to keep behavior signal and cut line count.
5. **Writes** `out/<id>.jsonl` (header line + one event per line).

Warp subagent tasks/blocks share the parent `conversation_id`, so they are pulled
in automatically. Hermes compression continuations and delegate subagents are
expanded through `parent_session_id`; explicit `/branch`, generic, and tool child
sessions are excluded unless they contain their own marker. Claude Code
conversations are keyed by `sessionId`, so subagent transcripts
(`<session>/subagents/agent-<id>.jsonl`) join their parent; only when one
session's markers carry several labels (e.g. an orchestrator whose subagents
each emit their own label) is each subagent keyed `<sessionId>/agent-<agentId>` and bound
to its own label. Re-runs under one label are distinct conversations and are all kept.

### Safety

The live Warp DB is opened **read-only** and never written. All work runs against a
`VACUUM INTO` snapshot (committed WAL pages included) that is itself opened
read-only and deleted when the run finishes.

The Hermes DB is opened directly with SQLite `readonly` + `fileMustExist` inside
one deferred read transaction. This gives marker discovery, lineage resolution,
and message reads one consistent snapshot including committed WAL frames. Do not
make a plain filesystem copy of a live WAL database for `--hermes-db-path`; use a
SQLite backup or `VACUUM INTO` when a copied fixture is required.

---

## Prerequisites

- macOS for Warp's default DB path (Claude Code and Hermes extraction work on any
  platform with a readable source store)
- Node.js >= 22
- Warp terminal installed and used (for `--source warp`)
- Claude Code installed and used (for `--source claude-code`)
- Hermes installed and used (for `--source hermes`)

For `--source warp`, the default Warp DB path is:

```text
~/Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.Warp-Stable/warp.sqlite
```

## Install

```bash
git clone https://github.com/Amirault/agent-session-capture.git
cd agent-session-capture
npm install
```

## Usage

Go through the `capture.sh` wrapper from **your project's** directory:

```bash
/path/to/agent-session-capture/capture.sh --id <id> --source claude-code
```

It installs the npm dependencies on first use, runs the CLI from your current directory
and defaults `--out` to the **main git checkout's** `.agent-captures/` store, so a capture made
in a disposable git worktree survives the worktree's removal. Set
`AGENT_CAPTURE_NODE_RUNNER="mise exec --"` (or similar) to run node through a version manager.
Every other flag is passed through; an explicit `--out` wins.

The raw CLI (`npx tsx src/cli.ts …`) resolves the default `--out out` relative to the cwd it
is run from.

### Extract one capture from Warp

```bash
npx tsx src/cli.ts --id <id> --source warp
# custom output directory
npx tsx src/cli.ts --id <id> --source warp --out /tmp/bundles
# point at a specific DB (debugging)
npx tsx src/cli.ts --id <id> --source warp --db-path /path/to/warp.sqlite
```

### Extract one capture from Claude Code

Claude Code stores transcripts as JSONL files under
`~/.claude/projects/<encoded-project-path>/<session-id>.jsonl`. The same
`CAPTURE_MARKER` no-op command must have been emitted in each session.

```bash
npx tsx src/cli.ts \
  --source claude-code \
  --id <id>

# custom transcript root (debugging / fixture / copied Claude home)
npx tsx src/cli.ts \
  --source claude-code \
  --claude-root /path/to/.claude/projects \
  --id <id>
```

### Extract one capture from Hermes

Hermes stores every profile's sessions in `<HERMES_HOME>/state.db`. A shell tool
inherits the current profile's `HERMES_HOME`, so this works directly from Hermes:

```bash
npx tsx src/cli.ts \
  --source hermes \
  --id <id>

# explicit SQLite-consistent backup / fixture
npx tsx src/cli.ts \
  --source hermes \
  --hermes-db-path /path/to/state.db \
  --id <id>
```

Without `HERMES_HOME`, the fallback matches Hermes itself: `~/.hermes/state.db`
on POSIX and `%LOCALAPPDATA%\hermes\state.db` on Windows. Other profiles are not
scanned automatically.

Output is written to `out/<id>.jsonl` (relative to cwd), with a one-line summary:

```text
wrote out/fix-login-bug.jsonl (199 events, 3 conversations)
```

If a marker can't bind to a conversation (see **Marker binding decays** below) or
binds to more than one, the run still succeeds but each anomaly is reported on
its own line (never as a bare count):

```text
warnings:
unbindable marker: label=default start_ts=2026-06-30 11:00:00.000000
```

If **zero** conversations bind for the capture id, nothing is written (no
header-only file) and the run exits non-zero:

```text
no conversations bound for capture id "<id>" — nothing written.
unbindable marker: label=default start_ts=2026-06-30 11:00:00.000000
a marker was found but its binding block is gone — Warp can evict blocks rows over time; extract soon after the session, before the binding decays.
```

### Exit codes

| Code | Meaning                                                                                         |
| ---- | ----------------------------------------------------------------------------------------------- |
| `0`  | Bundle written.                                                                                 |
| `1`  | Nothing written — zero conversations bound.                                                      |
| `2`  | Usage error (bad/missing flag).                                                                 |

### CLI flags

| Flag                      | Description                                                                                                                |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `--id <id>`               | Capture id to extract (required).                                                                                          |
| `--no-merge`              | Replace the existing bundle instead of merging (default: merge decay-safe); required when intentionally switching sources. |
| `--out <dir>`             | Output directory (default `out`; `capture.sh` defaults to the main checkout's `.agent-captures/`).                         |
| `--source <source>`       | Conversation source: `warp`, `claude-code`, or `hermes`; required.                                           |
| `--db-path <path>`        | Override the live Warp DB path (`--source warp`).                                                                          |
| `--claude-root <dir>`     | Override Claude Code transcript root (`--source claude-code`, default `~/.claude/projects`).                               |
| `--hermes-db-path <path>` | Override Hermes `state.db` (`--source hermes`, default `$HERMES_HOME/state.db`).                                           |
| `-h, --help`              | Print usage.                                                                                                               |

---

## Output format

Strict NDJSON: one JSON object per physical line (newlines inside values are
escaped by `JSON.stringify`, so no value spans multiple lines). The file ends with
a trailing newline.

**Line 1 — bundle header:**

```json
{
  "type": "bundle_header",
  "capture_id": "...",
  "labels": ["plan", "build"],
  "conversations_per_label": { "plan": 1, "build": 2 },
  "conversation_ids": ["..."],
  "extracted_at": "...",
  "source": "warp"
}
```

**Every subsequent line — one event:**

```json
{
  "capture_id": "...",
  "label": "build",
  "conversation_id": "...",
  "seq": 1,
  "ts": "2026-06-30 10:10:00.000000",
  "role": "user",
  "kind": "query",
  "content": "fix the login redirect",
  "meta": {
    "cwd": "/...",
    "model": "...",
    "git_branch": "main",
    "exchange_id": "..."
  }
}
```

Fields:

- `seq` — monotonic across the bundle; events are chronological while each
  conversation keeps its original message order.
- `role` — `user` | `assistant` | `tool`.
- `kind` — `query` | `agent_message` | `command` | `tool_call` | `tool_result`.
- `meta` — kind-specific: `field_path`, `exit_code`, `git_branch`, `cwd`, `model`,
  `block_id`, `exchange_id`, `task_id`, `task_event_index`, `subagent_task_id`,
  `repeat`, `truncated`, `original_len`, `confidence`, `message_kind`, `tool`,
  `tool_call_id`, `fields`, `skills`, `merged_count`, `message_id`,
  `message_event_index`, `active`, `compacted`, `session_source` (see Schema-aware
  field paths below).

### Compaction (`agent_message` events)

Walked protobuf nodes are compacted to reduce noise without losing signal:

- **Drop** base64-looking residue (>= 16 chars the walker could not unfold) and
  pure UUIDs.
- **Dedupe** identical `(field_path, value)` pairs onto one survivor with a
  `repeat` count (only present when > 1).
- **Truncate** values > 2000 chars to `head(1000) + …[truncated len=N]… + tail(500)`
  with `truncated: true` and `original_len`.
- **Collapse streaming deltas** (`collapseDeltas.ts`, after the schema overlay):
  Warp streams a `tool_call`/`tool_call_result` incrementally as many separate
  `Message` occurrences that each set one leaf field. These are grouped by
  shared `tool_call_id` into one event carrying a `fields` map (relative
  field path -> value) plus `meta.tool_call_id`. When the same relative path
  recurs with a _different_ value — e.g. `diffs.file_path` for each file in a
  multi-file `apply_file_diffs` call — the value becomes an array instead of
  being overwritten, so sibling repeated-field items are never silently
  dropped. `updated_skills_context` fan-out (one leaf per skill field) is
  reconstructed into a `meta.skills` summary list (`{path, name}` per skill)
  instead of one event per field. Any other consecutive same-`message_kind`
  deltas of one entity (e.g. a streamed `agent_output.text` growing chunk by
  chunk) are merged onto their final value with `meta.merged_count`.
  `agent_reasoning`, `user_query`, `update_todos`, and
  `messages_received_from_agents` are never grouped or merged away.
- **Dedupe static context across task rows** (`envelopeDedupe.ts`): per
  conversation, keep the first identical `updated_skills_context` set and
  `context.project_rules` payload. Later copies are removed; if a tool event also
  carries signal fields, only the repeated envelope fields are stripped. Changed
  rule content and all signal events remain intact.

If a task blob's walk hit malformed bytes, its events carry
`meta.confidence: "heuristic"`.

### Schema-aware field paths (`agent_message` events)

The walker is schema-less (it never assumes a schema, so it never silently
lies), but recovered field paths are then **renamed** against a generated copy (`npm run gen:schema`)
of Warp's protobuf schema (`warp.multi_agent.v1.Task` and friends, sourced from
`warpdotdev/warp-proto-apis`). The schema is reflected at dev time into
`src/adapters/protoSchema.ts` (no protobuf dependency at runtime). For each
walked node the overlay:

- rewrites the numbered path to a semantic one, e.g. `5.4.2.1` →
  `messages.tool_call.run_shell_command.command`;
- extracts two oneof variants that drive decision tracing, emitted in `meta`:
  - `message_kind` — the `Message` oneof variant (`user_query`, `tool_call`,
    `tool_call_result`, `agent_output`, `agent_reasoning`, …);
  - `tool` — the `ToolCall` oneof variant (`run_shell_command`, `grep`,
    `apply_file_diffs`, `subagent`, `read_files`, …).
- validates every path segment against the schema and, on any disagreement
  (an absent field number, a leaf the walker recursed past, or a message field
  emitted as a string), keeps the original numbered path and sets
  `meta.confidence: "schema-mismatch"`. It never silently relabels.

The pinned schema rev is recorded in `src/adapters/protoSchema.ts`
(`SCHEMA_REV`). If your installed Warp predates that rev, a larger share of
paths fall back to numbered form with `schema-mismatch` (still correct, just
less readable) — bump the rev and regenerate. A live-DB spot check (200 task
rows, ~20.9k nodes) named ~83% of nodes at the pinned rev.

---

## Project layout

Ports & adapters (clean architecture):

```text
src/
  cli.ts                          — entry point, arg parsing (node:util parseArgs)
  formatRunReport.ts              — pure CLI diagnostics: success/anomaly lines, exit code
  domain/
    models.ts                     — pure domain types
    ports.ts                      — ConversationReader/CaptureRead (use-case port), ReadableDb,
                                     ConversationSource, ConversationSink (Warp SQL internals)
    schemas.ts                    — zod schemas for ai_queries.input / blocks.ai_metadata
  usecases/
    extractBundle.ts              — orchestration: reader.readCapture → classify → order → header
  adapters/
    claudeCodeTranscriptReader.ts — Claude Code JSONL transcript reader
    hermesConversationReader.ts   — Hermes state.db reader + marker/lineage binding
    warpConversationReader.ts     — ConversationReader impl: binds markers + gathers events
    warpSqliteAdapter.ts          — live DB discovery + VACUUM INTO snapshot
    sqliteReadableDb.ts           — better-sqlite3 → ReadableDb port
    jsonlSink.ts                  — CaptureBundle → strict JSONL file
    protobufWalk.ts                — schema-less protobuf wire walker
    compact.ts                     — noise-reduction pass over walked nodes
    schemaOverlay.ts               — schema-aware name overlay (message_kind/tool)
    collapseDeltas.ts              — collapses streaming tool_call/tool_call_result field-deltas
    envelopeDedupe.ts              — removes repeated skill/rule envelopes across task rows
    protoSchema.ts                 — @generated field-number → name lookup (no runtime proto dep)
    ansi.ts                        — ANSI escape stripping
    readers/
      markerReader.ts              — find + bind CAPTURE_MARKER emissions
      queryReader.ts               — ai_queries → query events
      blockReader.ts               — blocks → command events
      taskReader.ts                — agent_tasks → agent_message events
  __tests__/                       — vitest, Given/When/Then
```

The use-case depends only on `ConversationReader`; Warp, Claude Code, and Hermes
are sibling adapters implementing the same port.

## Develop

```bash
npm run typecheck   # tsc --noEmit
npm test            # vitest run
npm run test:watch
npm run gen:schema  # download the Warp protos at the pinned rev and regenerate src/adapters/protoSchema.ts
```

**Warp schema and licensing.** The Warp protobuf schemas
([`warpdotdev/warp-proto-apis`](https://github.com/warpdotdev/warp-proto-apis)) are
AGPL-3.0, so neither the `.proto` files nor the generated lookup are vendored here:
`src/adapters/protoSchema.ts` ships as an empty placeholder and Warp protobuf paths stay
numbered (`meta.confidence: "schema-mismatch"`) until you run `npm run gen:schema`, which
downloads the protos at the pinned rev and writes the readable lookup locally (do not commit
the generated file). Claude Code and Hermes sources never use the schema. The test suite
uses a small hand-trimmed schema (`src/__tests__/fixtures/testProtoSchema.ts`) via a vitest
alias. To bump the rev, change `SCHEMA_REV` in `scripts/gen-schema-lookup.ts` and re-run the
generator; it needs only `protobufjs` (a devDependency) — no `protoc`.

Tests use in-process file-backed fixture DBs (no real Warp DB needed) and follow
Given/When/Then.

## Limitations

- **Marker binding decays.** A `CAPTURE_MARKER` binds to a conversation via a
  `blocks` row at the same `start_ts`; that row can disappear from Warp's DB
  before the marker command does (verified: a capture that bound 199 events one
  morning was fully unbindable the same night). Extract soon after finishing a
  session — don't rely on being able to extract it days later.
  **Mitigation (capture-at-close + decay-safe merge):** capture at the end
  of each session into `.agent-captures/<id>.jsonl` (in the main checkout — add it to
  your `.gitignore`; `capture.sh` targets it even from a worktree). A later capture merges
  fresh + stored decay-safe — fresh events are primary, stored events fill gaps
  left by marker decay or ring-buffer eviction — so a session captured at close is
  recoverable even after its live binding is gone. Always go through the tool
  rather than reading the stored bundle directly. Use `--no-merge` to replace an
  existing bundle instead of merging (e.g. after a corrupt file is removed).
- **`ai_queries` is capped at ~10,000 rows** (a ring buffer) — old prompts are
  evicted, compounding the binding-decay risk above for older sessions. The
  capture-at-close merge recovers evicted prompts from the stored bundle.
- **`did_execute`** (blocks) is read but not currently emitted in event `meta`.
- Protobuf field paths are **schema-aware**: walked paths are renamed against a
  generated copy of the `warp-proto-apis` schema (see Schema-aware field paths above). Paths
  that do not align with the schema rev keep their numbered form and carry
  `meta.confidence: "schema-mismatch"` (never silently relabelled). The pinned
  rev can drift from an installed Warp build — regenerate the lookup when you
  bump Warp.
- Warp default DB discovery is macOS-specific.
- Claude Code extraction depends on the same `CAPTURE_MARKER` command being present
  in the transcript. A marker chained on the command's first line
  (`cd app; : CAPTURE_MARKER …`, `script && : CAPTURE_MARKER …`) still binds, as a
  recovery path; quoted text and later lines (heredoc bodies) never do. A session that only mentions the id but never ran the
  marker is not bound, by design, to avoid heuristic grouping.
- Hermes binds only exact canonical marker lines executed by an assistant shell
  tool. It intentionally ignores prose, tool results, `echo` commands, and
  reordered or extended marker-shaped lines. A conflicting multi-label marker
  session is excluded and blocks inherited lineage until a later explicit marker.

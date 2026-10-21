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
- `langfuse`: a Langfuse project (e.g. an agent tracing from a server), read
  through the v2 observations endpoint (`GET /api/public/v2/observations`).

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
   - Langfuse: an exact marker line as the `command` input of a `TOOL` or `SPAN`
     observation named `terminal`, `run_shell_command` or `Bash`; the
     conversation is the `sessionId` of the observation's trace (Langfuse sets it
     on the trace's root observation only), else its `traceId`.
2. **Reads every event** for those conversations from the selected source:
   - Claude Code `type: "user"` entries → prompts and tool results
     (`kind: "query"` / `kind: "tool_result"`)
   - Claude Code `type: "assistant"` content blocks → assistant text and tool
     calls (`kind: "agent_message"` / `kind: "tool_call"`)
   - Hermes `user`, `assistant`, and `tool` messages → prompts, assistant text,
     tool calls, and tool results. Inactive compacted rows are retained with
     `active` / `compacted` metadata so the original learning history is not lost.
   - Langfuse observations of the last 30 days → a root observation's input
     (`query`), a `GENERATION` output (`agent_message`), a `TOOL`/`SPAN` input and
     output (`tool_call` / `tool_result`, sharing `meta.tool_call_id` = observation
     id); inputs are stamped at the observation's `startTime`, outputs
     (`agent_message`, `tool_result`) at its `endTime` (else `startTime`), and
     `meta.session_source` is the `agent` named in the trace root's metadata.
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

Langfuse is only read (`GET /api/public/v2/observations`): nothing is written to
the project, and the keys are never printed nor written to a bundle.

---

## Prerequisites

- macOS for Warp's default DB path (Claude Code and Hermes extraction work on any
  platform with a readable source store)
- A Rust toolchain (`cargo`, stable >= 1.85) to build the binary, or a prebuilt binary
- Warp terminal installed and used (for `--source warp`)
- Claude Code installed and used (for `--source claude-code`)
- Hermes installed and used (for `--source hermes`)
- A Langfuse project and its public/secret keys (for `--source langfuse`)

For `--source warp`, the default Warp DB path is:

```text
~/Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.Warp-Stable/warp.sqlite
```

## Install

```bash
git clone https://github.com/Amirault/agent-session-capture.git
cd agent-session-capture
cargo install --locked --path .   # puts agent-session-capture on your PATH (~/.cargo/bin)
```

`capture.sh` does not need the install step: it builds `target/release/agent-session-capture`
with `cargo build --release` on first use (and again after the sources change). Without
`cargo`, it falls back to an `agent-session-capture` binary on `PATH`; set `AGENT_CAPTURE_BIN`
to run a prebuilt binary explicitly (e.g. one built on another machine of the same platform:
the binary is self-contained, SQLite included).

## Usage

Go through the `capture.sh` wrapper from **your project's** directory:

```bash
/path/to/agent-session-capture/capture.sh --id <id> --source claude-code
```

It builds the release binary on first use, runs the CLI from your current directory
and defaults `--out` to the **main git checkout's** `.agent-captures/` store, so a capture made
in a disposable git worktree survives the worktree's removal. Set
`AGENT_CAPTURE_CARGO_RUNNER="mise exec rust@stable --"` (or similar) to run cargo through a version
manager, or `AGENT_CAPTURE_BIN=/path/to/agent-session-capture` to use a prebuilt binary.
Every other flag is passed through; an explicit `--out` wins.

The raw binary (`agent-session-capture …`) resolves the default `--out out` relative to the cwd it
is run from.

### Extract one capture from Warp

```bash
agent-session-capture --id <id> --source warp
# custom output directory
agent-session-capture --id <id> --source warp --out /tmp/bundles
# point at a specific DB (debugging)
agent-session-capture --id <id> --source warp --db-path /path/to/warp.sqlite
```

### Extract one capture from Claude Code

Claude Code stores transcripts as JSONL files under
`~/.claude/projects/<encoded-project-path>/<session-id>.jsonl`. The same
`CAPTURE_MARKER` no-op command must have been emitted in each session.

```bash
agent-session-capture \
  --source claude-code \
  --id <id>

# custom transcript root (debugging / fixture / copied Claude home)
agent-session-capture \
  --source claude-code \
  --claude-root /path/to/.claude/projects \
  --id <id>
```

### Extract one capture from Hermes

Hermes stores every profile's sessions in `<HERMES_HOME>/state.db`. A shell tool
inherits the current profile's `HERMES_HOME`, so this works directly from Hermes:

```bash
agent-session-capture \
  --source hermes \
  --id <id>

# explicit SQLite-consistent backup / fixture
agent-session-capture \
  --source hermes \
  --hermes-db-path /path/to/state.db \
  --id <id>
```

Without `HERMES_HOME`, the fallback matches Hermes itself: `~/.hermes/state.db`
on POSIX and `%LOCALAPPDATA%\hermes\state.db` on Windows. Other profiles are not
scanned automatically.

### Extract one capture from Langfuse

Reads the project's observations of the last 30 days, read-only, with
`LANGFUSE_HOST` (default `https://cloud.langfuse.com`), `LANGFUSE_PUBLIC_KEY` and
`LANGFUSE_SECRET_KEY`, each taken from the environment, else the `--langfuse-env`
file, else `~/.config/langfuse/capture.env` (dotenv lines; keep it outside any
repository). Key values are never printed nor written to a bundle.

```bash
/path/to/agent-session-capture/capture.sh --id <id> --source langfuse
# explicit credentials file
/path/to/agent-session-capture/capture.sh --id <id> --source langfuse --langfuse-env /path/to/capture.env
```

`capture.sh` stores Langfuse bundles in the `langfuse/` sub-store
(`.agent-captures/langfuse/<id>.jsonl`), so an id keeps one bundle per source.
Missing keys exit `2`; an API or network failure exits `1` naming the HTTP status
and host (`error: Langfuse API 401 at https://cloud.langfuse.com`) and leaves the
stored bundle untouched.

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

| Code | Meaning                                                                    |
| ---- | -------------------------------------------------------------------------- |
| `0`  | Bundle written.                                                            |
| `1`  | Nothing written — zero conversations bound, or the Langfuse API failed.    |
| `2`  | Usage error (bad/missing flag, missing Langfuse credentials).              |

### CLI flags

| Flag                      | Description                                                                                                                |
| ------------------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `--id <id>`               | Capture id to extract (required).                                                                                          |
| `--no-merge`              | Replace the existing bundle instead of merging (default: merge decay-safe); required when intentionally switching sources. |
| `--out <dir>`             | Output directory (default `out`; `capture.sh` defaults to the main checkout's `.agent-captures/`).                         |
| `--source <source>`       | Conversation source: `warp`, `claude-code`, `hermes`, or `langfuse`; required.                                             |
| `--db-path <path>`        | Override the live Warp DB path (`--source warp`).                                                                          |
| `--claude-root <dir>`     | Override Claude Code transcript root (`--source claude-code`, default `~/.claude/projects`).                               |
| `--hermes-db-path <path>` | Override Hermes `state.db` (`--source hermes`, default `$HERMES_HOME/state.db`).                                           |
| `--langfuse-env <file>`   | Langfuse credentials file read after the environment (`--source langfuse`, then `~/.config/langfuse/capture.env`).         |
| `--sessions`              | Session mode: export every conversation, no marker — see [Session mode](#session-mode).                                    |
| `--since <ISO-8601>`     | Session mode only: only conversations whose last activity is at/after the cutoff.                                        |
| `--conversation <cid>`   | Session mode only: export exactly one conversation (debugging).                                                          |
| `-h, --help`              | Print usage.                                                                                                               |

---

## Session mode

`--sessions` is the non-marker export: instead of binding a `CAPTURE_MARKER` to
conversations, it exports **every conversation** in the source store — one JSONL
file per conversation, without `capture_id`/`label` (a session is not a capture:
it is every conversation in the store, not the subset a marker bound). It exists
for history you never tagged and as a safety net against marker-binding decay.

Every source implements it.

```bash
agent-session-capture --sessions --source warp
# custom output directory / explicit DB
agent-session-capture --sessions --source warp --out /tmp/sessions --db-path /path/to/warp.sqlite
# recent conversations only, or exactly one
agent-session-capture --sessions --source warp --since 2026-07-01
agent-session-capture --sessions --source warp --conversation <cid>
```

With `--source claude-code` (transcripts under `--claude-root`, default
`~/.claude/projects`), a session is one `sessionId`: the `<session>.jsonl`
transcript plus its `<session>/subagents/agent-*.jsonl` files, so subagent
events join their parent's file. Its stamp is the latest mtime of those files
(ISO-8601 UTC), and events map exactly as in marker mode:

```bash
agent-session-capture --sessions --source claude-code --since 2026-10-01
# wrote out/sessions/<sessionId>.jsonl (N events)
```

With `--source hermes` (`--hermes-db-path`, default `$HERMES_HOME/state.db`),
a session is a lineage root of `sessions` plus the descendants the marker-mode
lineage rule joins to it: compression continuations and delegate subagents
join their root's file, while `/branch`, generic and tool children get their
own. Its stamp is the latest message timestamp of the group (ISO-8601 UTC),
every event carries the root's id as its conversation, and events map exactly
as in marker mode:

```bash
agent-session-capture --sessions --source hermes --since 2026-10-01
# wrote out/sessions/<root session id>.jsonl (N events)
```

With `--source langfuse` (credentials as in marker mode), a session is a Langfuse
`sessionId` — every trace whose root carries it — or a trace without session,
named by its `traceId`. One content-less pass (`fields=core,basic,time`, from
`--since` on, else the whole project) enumerates them; its stamp is the latest
`updatedAt` of the observations that pass sees (ISO-8601 UTC). Only the changed
sessions are then fetched, trace by trace (`traceId` filter), and mapped exactly
as in marker mode. With `--since`, a trace whose root started before the cutoff
is looked up by `traceId` to find its session, and a changed session first lists
its roots by `sessionId`, so its file also holds its traces started before the
cutoff. `capture.sh` stores them in the `langfuse/` sub-store
(`.agent-captures/langfuse/sessions/<id>.jsonl`):

```bash
/path/to/agent-session-capture/capture.sh --sessions --source langfuse --since 2026-10-01
# wrote …/.agent-captures/langfuse/sessions/<sessionId or traceId>.jsonl (N events)
```

### Sessions layout and incremental semantics

Each conversation is written to `<outDir>/sessions/<file>.jsonl` (atomic write:
tmp file + rename). The file name is the conversation id when that is already a
safe single path segment (`[A-Za-z0-9._-]+`, not `.`/`..`); anything else is
sanitized plus a digest suffix, so a conversation id can never traverse out of
`sessions/`.

Line 1 is a `session_header`:

```json
{
  "type": "session_header",
  "conversation_id": "…",
  "source": "warp",
  "last_modified": "2026-06-30 10:30:00.000000",
  "extracted_at": "2026-06-30T10:45:00.000Z"
}
```

`last_modified` is the source-native last-activity stamp — for Warp the `MAX` of
`agent_conversations.last_modified_at` and `agent_tasks.last_modified_at`,
durable tables that do not evict, so a conversation present only in
`agent_tasks` is listed too. Every subsequent line is a session event: the same
event shape as the bundle format minus `capture_id`/`label`, with a per-session
`seq`:

```json
{
  "conversation_id": "…",
  "seq": 1,
  "ts": "2026-06-30 10:10:00.000000",
  "role": "user",
  "kind": "query",
  "content": "fix the login redirect",
  "meta": { "cwd": "/…" }
}
```

Re-runs are incremental:

- a conversation whose stored `last_modified` equals the fresh stamp is
  **unchanged** — not re-read, not rewritten (raw string equality, no parsing);
- each gathered conversation is merged **decay-safe** with its stored file (fresh
events primary, stored events fill gaps left by ring-buffer eviction), then
  re-ordered and re-`seq`'d;
- stored files that fall out of the enumeration (source eviction, a `--since`
  window, a `--conversation` pick) are **untouched** — the files are the durable
  record, never GC'd.

Guards mirror the marker mode's stored-bundle guards: a corrupt session file, a
header-vs-filename conversation mismatch, or a file of another source for a
conversation this run gathers fails the run loudly (nothing overwritten) unless
`--no-merge` downgrades each violation to a warning and replaces the file. The
local sources share `sessions/`: a file of another source is otherwise left
untouched. Guard warnings and skipped rows are reported one
per line, never as bare counts.

### Session exit codes

| Code | Meaning |
| ---- | ------- |
| `0`  | At least one session written, or everything already up to date. |
| `1`  | Zero sessions enumerated (`--since` too recent, unknown `--conversation`), or a read/guard failure (a Langfuse API failure included; stored files untouched). |
| `2`  | Usage error (`--id` with `--sessions`; `--since`/`--conversation` without `--sessions`; unparseable `--since`), or missing Langfuse credentials. |

### Session limitations

- **Langfuse stamps with `--since`** cover the observations the windowed pass
  sees: a later update to an observation started before the cutoff, with no
  newer change in the window, stays stale in the file until a run without
  `--since`.
- **Task events carry `last_modified_at` as `ts`** — the protobuf walk has no
  per-message timestamp yet, so a session's agent messages keep their source
  order under the causal sort. A per-message timestamp patch is a planned
  follow-up.
- Enumeration uses only the durable `agent_conversations`/`agent_tasks` tables,
  so a conversation evicted from both is no longer enumerated — but its exported
  file is never deleted.

---

## Output format

Strict NDJSON: one JSON object per physical line (newlines inside values are
escaped as `JSON.stringify` escapes them, so no value spans multiple lines). The file ends with
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
- **Collapse streaming deltas** (`collapse_deltas.rs`, after the schema overlay):
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
- **Dedupe static context across task rows** (`envelope_dedupe.rs`): per
  conversation, keep the first identical `updated_skills_context` set and
  `context.project_rules` payload. Later copies are removed; if a tool event also
  carries signal fields, only the repeated envelope fields are stripped. Changed
  rule content and all signal events remain intact.

If a task blob's walk hit malformed bytes, its events carry
`meta.confidence: "heuristic"`.

### Schema-aware field paths (`agent_message` events)

The walker is schema-less (it never assumes a schema, so it never silently
lies), but recovered field paths are then **renamed** against a generated copy (`cargo run --example gen-schema-lookup`)
of Warp's protobuf schema (`warp.multi_agent.v1.Task` and friends, sourced from
`warpdotdev/warp-proto-apis`). The schema is reflected at dev time into
`generated/proto_schema.rs`, compiled in by `build.rs` (no protobuf dependency at runtime). For each
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

The pinned schema rev is recorded in `examples/gen-schema-lookup.rs`
(`SCHEMA_REV`). If your installed Warp predates that rev, a larger share of
paths fall back to numbered form with `schema-mismatch` (still correct, just
less readable) — bump the rev and regenerate. A live-DB spot check (200 task
rows, ~20.9k nodes) named ~83% of nodes at the pinned rev.

---

## Project layout

Ports & adapters (clean architecture), one Cargo package at the repository root:

```text
src/
  main.rs                          — binary entry point: run the CLI, print, exit
  cli.rs                           — argument parsing (Node `util.parseArgs` strict-mode contract), dispatch
  format_run_report.rs             — pure CLI diagnostics: success/anomaly lines, exit code
  domain/
    models.rs                      — pure domain types
    ports.rs                       — ConversationReader/CaptureRead + SessionReader (use-case ports),
                                     ConversationSource (Warp snapshot)
    schemas.rs                     — validation of ai_queries.input / blocks.ai_metadata
    capture_marker.rs              — exact CAPTURE_MARKER line parser (Hermes, Langfuse)
    skip_unchanged.rs              — should_gather: the skip-unchanged rule shared by every SessionReader
    source_errors.rs               — ReadError: source access / configuration errors are never masked by a stored bundle
    merge_bundle.rs                — decay-safe merge of stored and fresh events
    bundle_header.rs, causal_sort.rs, stamps.rs, ordered_map.rs
  usecases/
    extract_bundle.rs              — orchestration: read_capture → classify → order → header
    extract_sessions.rs            — sessions orchestration: enumerate → gather changed → merge → write
  adapters/
    claude_code_transcript_reader.rs — Claude Code JSONL transcript reader, enumerates sessions by sessionId
    hermes_conversation_reader.rs  — Hermes state.db reader + marker/lineage binding, enumerates sessions by lineage root
    langfuse_conversation_reader.rs — Langfuse observations → marker binding + events, sessions (client injected)
    langfuse_observation_client.rs — HTTP client of the v2 endpoint: credentials, pagination, API errors
    langfuse_observation_lister.rs — the lister port the reader needs, the client implements
    warp_conversation_reader.rs    — ConversationReader + SessionReader impl: binds markers, gathers events, enumerates sessions
    warp_sqlite_adapter.rs         — live DB discovery + VACUUM INTO snapshot
    sqlite_readable_db.rs          — read-only SQLite access (rusqlite, SQLite bundled)
    jsonl_sink.rs                  — bundles and session files → strict JSONL (atomic writes)
    jsonl_bundle_reader.rs, jsonl_session_reader.rs — stored bundles / session files, guarded
    protobuf_walk.rs               — schema-less protobuf wire walker
    compact.rs                     — noise-reduction pass over walked nodes
    schema_overlay.rs              — schema-aware name overlay (message_kind/tool)
    collapse_deltas.rs             — collapses streaming tool_call/tool_call_result field-deltas
    envelope_dedupe.rs             — removes repeated skill/rule envelopes across task rows
    proto_schema.rs                — field-number → name lookup (table generated into generated/proto_schema.rs)
    ansi.rs                        — ANSI escape stripping
    readers/
      marker_reader.rs             — find + bind CAPTURE_MARKER emissions
      query_reader.rs              — ai_queries → query events
      block_reader.rs              — blocks → command events
      task_reader.rs               — agent_tasks → agent_message events
  js/                              — the JavaScript semantics the output contract depends on (the tool
                                     was first written in TypeScript): JSON key order, number and date
                                     formatting, V8 JSON.parse messages, Node path normalization, UTF-16 strings
  tests/                           — unit tests, Given/When/Then (+ fixtures/)
tests/                             — CLI integration tests (run the built binary)
examples/gen-schema-lookup.rs      — Warp schema lookup generator
```

The use cases depend only on the ports; Warp, Claude Code, Hermes and Langfuse
are sibling adapters implementing both `ConversationReader` and `SessionReader`.

## Develop

```bash
cargo build --release              # target/release/agent-session-capture
cargo test                         # unit + CLI integration tests
cargo fmt --check && cargo clippy --all-targets -- -D warnings
cargo run --example gen-schema-lookup  # download the Warp protos at the pinned rev and generate generated/proto_schema.rs
```

**Warp schema and licensing.** The Warp protobuf schemas
([`warpdotdev/warp-proto-apis`](https://github.com/warpdotdev/warp-proto-apis)) are
AGPL-3.0, so neither the `.proto` files nor the generated lookup are vendored here:
without `generated/proto_schema.rs` the table is empty and Warp protobuf paths stay
numbered (`meta.confidence: "schema-mismatch"`) until you run
`cargo run --example gen-schema-lookup`, which downloads the protos at the pinned rev and
writes the readable lookup locally (`generated/` is git-ignored: do not commit it), then
rebuild. `build.rs` compiles it in when present (`PROTO_SCHEMA_RS=<file>` points it
elsewhere). Claude Code, Hermes and Langfuse sources never use the schema. The unit tests
use a small hand-trimmed schema (`src/tests/fixtures/test_proto_schema.rs`). To bump the
rev, change `SCHEMA_REV` in `examples/gen-schema-lookup.rs` and re-run the generator; it
needs no `protoc` and no protobuf library.

Tests use file-backed fixture DBs (no real Warp DB needed) and an in-memory or loopback
(`127.0.0.1`) fake of the Langfuse v2 endpoint (never the real API), and follow
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

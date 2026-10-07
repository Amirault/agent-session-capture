# agent-session-capture

**Tag a coding-agent session with one line. Export every session carrying that tag — from Claude Code, Hermes or Warp — as a single chronological JSONL.**

```text
: CAPTURE_MARKER v=1 id=checkout-bug
```

That is the whole integration. It is a shell no-op: the agent (or you) runs it once at the
start of a session, and `agent-session-capture` later finds every conversation that ran it —
across sessions, across subagents, across machines' worth of history — and exports them as one
time-ordered bundle.

## Why a marker

Agent runtimes each store sessions differently and none of them group by _task_. Heuristics
("sessions that mention this ticket") produce false positives and misses. An explicit,
executed marker is deterministic, runtime-agnostic and **easy to share**:

- put it in a rules file or skill so agents emit it at the start of a task;
- paste it into a PR description or chat so a teammate's sessions can be exported by the
  same `id`;
- have an orchestrator inject it, with a per-run id, into every subagent prompt.

Prose that merely mentions the marker never binds — only an executed command does.

## Quick start

```bash
git clone https://github.com/Amirault/agent-session-capture.git ~/tools/agent-session-capture
cd ~/tools/agent-session-capture && npm install        # Node >= 22.2 (npm may print a harmless install-scripts warning)
```

1. In any agent session, run (or ask the agent to run) the marker:

   ```text
   : CAPTURE_MARKER v=1 id=checkout-bug
   ```

2. Export, from your project directory:

   ```bash
   ~/tools/agent-session-capture/capture.sh --id checkout-bug --source claude-code
   # wrote /path/to/your/project/.agent-captures/checkout-bug.jsonl (199 events, 3 conversations)
   ```

Optional: `label=<name>` distinguishes several conversations of one id (a step, a role, a
retry) — `: CAPTURE_MARKER v=1 id=checkout-bug label=review`. Ready-made agent instructions:
[`skills/capture-marker`](skills/capture-marker/SKILL.md).

## Adapters

| `--source`    | Reads                                                    | Notes                                                       |
| ------------- | -------------------------------------------------------- | ----------------------------------------------------------- |
| `claude-code` | `~/.claude/projects/**/*.jsonl`                          | Subagent transcripts join their parent session.             |
| `hermes`      | `<HERMES_HOME>/state.db` (SQLite, read-only)             | Compression continuations and delegate subagents followed.  |
| `warp`        | Warp's local SQLite DB (macOS), via a read-only snapshot | Recovers assistant text from protobuf blobs, schema-less.   |

Every adapter implements one small port, so [adding another](docs/ADAPTERS.md) (Cursor, Codex
CLI, Aider, …) is a single `ConversationReader`.

## Output

Strict NDJSON. Line 1 is a header, every other line is one event:

```json
{"type":"bundle_header","capture_id":"checkout-bug","labels":["default"],"conversations_per_label":{"default":3},"conversation_ids":["…"],"extracted_at":"…","source":"claude-code"}
{"capture_id":"checkout-bug","label":"default","conversation_id":"…","seq":1,"ts":"2026-06-30T10:10:00Z","role":"user","kind":"query","content":"fix the login redirect","meta":{"cwd":"/…","git_branch":"main"}}
```

`role` is `user|assistant|tool`; `kind` is `query|agent_message|command|tool_call|tool_result`.
Events are chronological across conversations while each conversation keeps its original
message order. Full format: [docs/REFERENCE.md](docs/REFERENCE.md#output-format).

## Session stores decay — capture at the end too

Warp keeps prompts in a ~10,000-row ring buffer and evicts the rows that bind a marker to its
conversation. So re-running the export is **decay-safe**: fresh events win, events already in
`.agent-captures/<id>.jsonl` fill the gaps. Export at the end of a session and the history
stays recoverable after the runtime forgets it. Captures made in a disposable git worktree land
in the **main checkout's** `.agent-captures/`, so they survive the worktree.

## Docs

- [The `CAPTURE_MARKER` convention](docs/MARKER.md) — format, emission rules, how each runtime binds it
- [Adapters](docs/ADAPTERS.md) — how sources are read, how to write one
- [Reference](docs/REFERENCE.md) — CLI flags, exit codes, output schema, limitations

## Safety

Live stores are opened **read-only**: Warp through a `VACUUM INTO` snapshot that is deleted
afterwards, Hermes through one read-only transaction, Claude Code transcripts are plain files.
Nothing is sent anywhere — but bundles contain your raw prompts and tool output, so
add `.agent-captures/` to your project's `.gitignore` and review a bundle before sharing it.

## Warp schema note

The Warp protobuf schemas are AGPL-3.0 and are not vendored here. Without them Warp assistant
text is still recovered, with numbered field paths. Run `npm run gen:schema` to download them
at the pinned revision and get readable paths. See [REFERENCE](docs/REFERENCE.md#develop).

## Development

```bash
npm test            # vitest, fixture-based, no real session store needed
npm run typecheck
```

## License

[MIT](LICENSE)

# The `CAPTURE_MARKER` convention

A one-line, greppable token that an agent emits **inside a session** so that session can be
found later, in any supported agent runtime, without heuristics.

You pick an `id` — a ticket number, a task slug, an experiment name, a PR. Every session
that carries a marker with that `id` ends up in one bundle.

## Format

```text
: CAPTURE_MARKER v=1 id=<id> [label=<label>]
```

| Field         | Required | Meaning                                                                                              |
| ------------- | -------- | ---------------------------------------------------------------------------------------------------- |
| `v=1`         | yes      | Format version, so adapters can evolve without breaking old data.                                    |
| `id=<id>`     | yes      | What to group by: letters, digits, `.`, `_`, `-`, starting with a letter or digit (it names the output file). `JIRA-123`, `fix-login-bug`, `exp-2026-10-a`. Quotes are not interpreted. |
| `label=<l>`   | no       | A free-form tag for this conversation within the id: a step, a role, a retry number. Default `default`. |

Labels carry no semantics for the tool — they are copied into every event and tallied in the
bundle header. Use them if you want to tell sessions of one id apart (`label=plan`,
`label=build`, `label=review`, `label=retry-2`); ignore them otherwise.

## Examples

```text
: CAPTURE_MARKER v=1 id=JIRA-123
: CAPTURE_MARKER v=1 id=exp-2026-10-a label=retry-2
```

## Emission

The agent runs the marker as a **literal no-op shell command** through its runtime's shell
tool. The leading `:` is a shell builtin that does nothing (exit 0, no side effect).

| Runtime     | Shell tool                        |
| ----------- | --------------------------------- |
| Warp        | `run_shell_command`               |
| Claude Code | `Bash`                            |
| Hermes      | `terminal` or `run_shell_command` |

Rules:

- **Run it; do not just print it.** Adapters read executed commands, not prose.
- **Run it as its own shell call** — the whole command is the marker line alone: no `cd`
  prefix, no `;`/`&&` chaining, nothing after it. Warp and Hermes only bind a command that
  _starts_ with `: CAPTURE_MARKER`; the Claude Code adapter tolerates a marker chained on the
  first line as a recovery path, not as a way to emit it. Hermes additionally rejects any extra
  token after the marker; Claude Code ignores trailing tokens, but do not rely on that.
- **`id` must be the resolved literal**, never a `$(...)` substitution or shell variable —
  adapters match the submitted command text.
- **Emit once per session**, as early as possible.
- Emitting the same `id` from several sessions is the point: all of them are collected and
  ordered by time. No dedup, no state.

## Who emits it — and how it gets shared

The marker is deliberately easy to _suggest_ rather than hard-code:

- **A human** asks the agent to run it — paste the one-line prompt from the README into the
  session. The command must be executed by the agent through its shell tool; typing it in a
  separate terminal of your own is not part of the agent's session and will not bind.
- **A skill / rules file** can instruct agents to emit it at the start of a task — see
  [`skills/capture-marker`](../skills/capture-marker/SKILL.md).
- **A teammate** can send you the line (`id=…`) in a PR description or a chat; if their
  sessions ran it, `capture.sh --id <id>` on their machine produces the bundle to share.
- **An orchestrator** can inject it into every subagent prompt with a per-run `id`.

## How each source binds a marker to a conversation

- **Warp** — the marker text lives in `commands.command`; the conversation id comes from the
  `blocks` row sharing the same `start_ts`. Subagent blocks share the parent
  `conversation_id`, so they come along.
- **Claude Code** — a `Bash` `tool_use` block inside the session JSONL transcript. Subagent
  transcripts (`<session>/subagents/agent-<id>.jsonl`) join their parent session; when one
  session's markers carry several labels, each subagent is bound to its own label.
- **Hermes** — an exact marker line in an assistant `terminal`/`run_shell_command` tool call
  stored in `messages.tool_calls`. Compression continuations and delegate subagents are
  expanded through `parent_session_id`.

Prose that merely mentions the marker never binds: a session is grouped only if it actually
ran the command. That is deliberate — no heuristic grouping, no false positives.

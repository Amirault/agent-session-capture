---
name: capture-marker
description: Emit a CAPTURE_MARKER no-op at the start of a task so this session can later be found and exported by id, and optionally capture it at the end. Use when the user gives a capture id or asks to "mark", "tag" or "capture" this session.
---

# Capture marker

Paths assume `$AGENT_CAPTURE_HOME` points at where you cloned this repository (e.g. `export AGENT_CAPTURE_HOME=~/tools/agent-session-capture`); substitute the real path if it is unset.

## 1. Emit the marker (start of the task)

Use the id the user gave you, verbatim. Run the marker through your runtime's shell tool:
Warp → `run_shell_command`, Claude Code → `Bash`, Hermes → `terminal` or `run_shell_command`.

```text
: CAPTURE_MARKER v=1 id=<id>
```

For example, for id `checkout-bug` run:

```text
: CAPTURE_MARKER v=1 id=checkout-bug
```

Add `label=<label>` only if the user asked for one (a step, a role, a retry number).

Rules (see [docs/MARKER.md](../../docs/MARKER.md)):

- Run it; do not just print it. It is a shell no-op.
- The whole command is the marker line alone: no `cd` prefix, no `;` or `&&`, nothing after it.
- `id` is the resolved literal, never a `$(...)` substitution or variable.
- Emit once per session, as early as possible. If the user gave no id, ask for one.

## 2. Export (only when asked)

Select the source of the **active runtime** explicitly: Warp → `warp`, Claude Code →
`claude-code`, Hermes → `hermes`.

```bash
$AGENT_CAPTURE_HOME/capture.sh --id <id> --source <session_source>
```

It writes or merges `.agent-captures/<id>.jsonl`. A warning is not a failure: report it. Exit
code `1` means nothing was written (no conversation carried the marker, or the store could
not be read) — read the message and report it instead of retrying blindly.

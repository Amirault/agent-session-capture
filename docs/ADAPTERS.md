# Adapters

An adapter turns one agent runtime's session store into the common bundle format.

| Source        | Store read                                             | Marker binding                                    |
| ------------- | ------------------------------------------------------ | ------------------------------------------------- |
| `claude-code` | `~/.claude/projects/**/<session>.jsonl`                | `Bash` `tool_use` blocks (+ `subagents/`)         |
| `hermes`      | `<HERMES_HOME>/state.db` (read-only, one read txn)     | exact marker line in an assistant shell tool call |
| `langfuse`    | a Langfuse project, v2 observations API (read-only)    | exact marker line as a `TOOL`/`SPAN` shell `command` |
| `warp`        | Warp's SQLite DB (`VACUUM INTO` snapshot, read-only)   | `commands` ⋈ `blocks` on `start_ts`               |

Event mapping per source is documented in [REFERENCE.md](REFERENCE.md#what-it-does).

## Writing a new adapter

The use case ([`src/usecases/extract_bundle.rs`](../src/usecases/extract_bundle.rs)) depends on a
single port, defined in [`src/domain/ports.rs`](../src/domain/ports.rs):

```rust
pub trait ConversationReader {
    // bound conversations + events + warnings
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError>;
}
```

To add a runtime (Cursor, Aider, Codex CLI, …):

1. **Find where it stores sessions**, and whether shell tool calls are recorded verbatim —
   the [marker](MARKER.md) is only as reliable as that record.
2. **Implement `ConversationReader`** in `src/adapters/<name>_conversation_reader.rs`:
   - find marker emissions for `captureId` (reuse `parse_marker` from
     [`readers/marker_reader.rs`](../src/adapters/readers/marker_reader.rs), or
     `exact_markers_in_command` from [`domain/capture_marker.rs`](../src/domain/capture_marker.rs)
     when the source records each shell call's raw command);
   - bind each marker to a conversation id and a label;
   - emit events with `role` (`user|assistant|tool`), `kind`
     (`query|agent_message|command|tool_call|tool_result`), `ts`, `content` and a `meta` bag;
   - keep each conversation's original message order — the use case orders across
     conversations, never within one;
   - report anomalies as warnings (unbindable marker, truncated read), never silently.
3. **Open the store read-only.** Never write to a live session store. A remote source (see
   [`langfuse_conversation_reader.rs`](../src/adapters/langfuse_conversation_reader.rs)) takes its
   client by injection, so tests never touch the network, and throws a `SourceAccessError`
   (exit 1) or `ReadError::SourceConfiguration` (exit 2) from
   [`domain/source_errors.rs`](../src/domain/source_errors.rs) when it cannot be read: a stored
   bundle never masks those.
4. **Wire it** into `reader_for` in [`src/cli.rs`](../src/cli.rs), and add it to `BundleSource`
   in [`src/domain/models.rs`](../src/domain/models.rs).
5. **Test it with a fixture store** (see `src/tests/claude_code_transcript_reader.rs` for the
   pattern): Given/When/Then, no real user data.
6. Add the runtime's shell tool name to the table in [MARKER.md](MARKER.md#emission).

## Session mode

`--sessions` (see [REFERENCE.md](REFERENCE.md#session-mode)) exports every
conversation without markers, through a second port,
[`SessionReader`](../src/domain/ports.rs):

```rust
pub trait SessionReader {
    // enumerated sessions + gathered events
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError>;
}
```

An adapter implementing it must:

1. **Enumerate every conversation** with its last-activity stamp
   (`SessionInfo: { conversation_id, last_modified }`), applying the
   `--since`/`--conversation` filters. Prefer a *durable* table — one that does
   not evict old rows (Warp uses `agent_conversations` + `agent_tasks`, not
   `ai_queries`/`blocks`). `last_modified: None` means "no durable stamp" and
   forces a re-extract on every run — return a stamp if the store has one.
2. **Skip unchanged conversations**: when a stored header's `last_modified`
   equals the fresh stamp (raw string equality, no parsing — the stamp's
   representation must never change), skip gathering that conversation
   entirely. `options.stored_by_cid` holds the stored headers. A new
   conversation, a changed stamp, or the `--conversation` pick is always
   gathered — `should_gather` in
   [`domain/skip_unchanged.rs`](../src/domain/skip_unchanged.rs) implements the rule.
3. **Gather events for the rest** reusing the same `EventDraft`s as marker mode
   (queries, blocks, tasks…), inside one snapshot for the whole run, reporting
   skipped rows as warnings.

Everything else — scanning stored session files, guards, decay-safe merging,
atomic writes, reporting — is owned by the use case
([`src/usecases/extract_sessions.rs`](../src/usecases/extract_sessions.rs)).

Every adapter implements both ports: `reader_for` in [`src/cli.rs`](../src/cli.rs) returns a
`Box<dyn Reader>` (both ports). See
[`warp_conversation_reader.rs`](../src/adapters/warp_conversation_reader.rs) for a worked
example over a local store, and
[`langfuse_conversation_reader.rs`](../src/adapters/langfuse_conversation_reader.rs) over a
remote API.

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

The use case ([`src/usecases/extractBundle.ts`](../src/usecases/extractBundle.ts)) depends on a
single port, defined in [`src/domain/ports.ts`](../src/domain/ports.ts):

```ts
interface ConversationReader {
  readCapture(captureId: string): Promise<CaptureRead>; // bound conversations + events + warnings
}
```

To add a runtime (Cursor, Aider, Codex CLI, …):

1. **Find where it stores sessions**, and whether shell tool calls are recorded verbatim —
   the [marker](MARKER.md) is only as reliable as that record.
2. **Implement `ConversationReader`** in `src/adapters/<name>ConversationReader.ts`:
   - find marker emissions for `captureId` (reuse `parseMarker` from
     [`readers/markerReader.ts`](../src/adapters/readers/markerReader.ts), or
     `exactMarkersInCommand` from [`domain/captureMarker.ts`](../src/domain/captureMarker.ts)
     when the source records each shell call's raw command);
   - bind each marker to a conversation id and a label;
   - emit events with `role` (`user|assistant|tool`), `kind`
     (`query|agent_message|command|tool_call|tool_result`), `ts`, `content` and a `meta` bag;
   - keep each conversation's original message order — the use case orders across
     conversations, never within one;
   - report anomalies as warnings (unbindable marker, truncated read), never silently.
3. **Open the store read-only.** Never write to a live session store. A remote source (see
   [`langfuseConversationReader.ts`](../src/adapters/langfuseConversationReader.ts)) takes its
   client by injection, so tests never touch the network, and throws a `SourceAccessError`
   (exit 1) or `SourceConfigurationError` (exit 2) from
   [`domain/sourceErrors.ts`](../src/domain/sourceErrors.ts) when it cannot be read: a stored
   bundle never masks those.
4. **Wire it** into `readerFor`, `parseSource` and `CliSource` in [`src/cli.ts`](../src/cli.ts),
   and add it to `BundleSource` in [`src/domain/models.ts`](../src/domain/models.ts).
5. **Test it with a fixture store** (see `claudeCodeTranscriptReader.test.ts` for the
   pattern): Given/When/Then, no real user data.
6. Add the runtime's shell tool name to the table in [MARKER.md](MARKER.md#emission).

## Session mode

`--sessions` (see [REFERENCE.md](REFERENCE.md#session-mode)) exports every
conversation without markers, through a second port,
[`SessionReader`](../src/domain/ports.ts):

```ts
interface SessionReader {
  readSessions(options: ReadSessionsOptions): Promise<SessionsRead>; // enumerated sessions + gathered events
}
```

An adapter implementing it must:

1. **Enumerate every conversation** with its last-activity stamp
   (`SessionInfo: { conversation_id, last_modified }`), applying the
   `--since`/`--conversation` filters. Prefer a *durable* table — one that does
   not evict old rows (Warp uses `agent_conversations` + `agent_tasks`, not
   `ai_queries`/`blocks`). `last_modified: null` means "no durable stamp" and
   forces a re-extract on every run — return a stamp if the store has one.
2. **Skip unchanged conversations**: when a stored header's `last_modified`
   equals the fresh stamp (raw string equality, no parsing — the stamp's
   representation must never change), skip gathering that conversation
   entirely. `options.storedByCid` holds the stored headers. A new
   conversation, a changed stamp, or the `--conversation` pick is always
   gathered — `shouldGather` in
   [`domain/skipUnchanged.ts`](../src/domain/skipUnchanged.ts) implements the rule.
3. **Gather events for the rest** reusing the same `EventDraft`s as marker mode
   (queries, blocks, tasks…), inside one snapshot for the whole run, reporting
   skipped rows as warnings.

Everything else — scanning stored session files, guards, decay-safe merging,
atomic writes, reporting — is owned by the use case
([`src/usecases/extractSessions.ts`](../src/usecases/extractSessions.ts)).

Every adapter implements both ports: `readerFor` in [`src/cli.ts`](../src/cli.ts) returns a
`ConversationReader & SessionReader`. See
[`warpConversationReader.ts`](../src/adapters/warpConversationReader.ts) for a worked
example over a local store, and
[`langfuseConversationReader.ts`](../src/adapters/langfuseConversationReader.ts) over a
remote API.

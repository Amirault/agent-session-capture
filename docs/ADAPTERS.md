# Adapters

An adapter turns one agent runtime's local session store into the common bundle format.

| Source        | Store read                                             | Marker binding                                    |
| ------------- | ------------------------------------------------------ | ------------------------------------------------- |
| `claude-code` | `~/.claude/projects/**/<session>.jsonl`                | `Bash` `tool_use` blocks (+ `subagents/`)         |
| `hermes`      | `<HERMES_HOME>/state.db` (read-only, one read txn)     | exact marker line in an assistant shell tool call |
| `warp`        | Warp's SQLite DB (`VACUUM INTO` snapshot, read-only)   | `commands` ⋈ `blocks` on `start_ts`               |

Event mapping per source is documented in [REFERENCE.md](REFERENCE.md#what-it-does).

## Writing a new adapter

The use case ([`src/usecases/extractBundle.ts`](../src/usecases/extractBundle.ts)) depends on a
single port, defined in [`src/domain/ports.ts`](../src/domain/ports.ts):

```ts
interface ConversationReader {
  readCapture(captureId: string): CaptureRead; // bound conversations + events + warnings
}
```

To add a runtime (Cursor, Aider, Codex CLI, …):

1. **Find where it stores sessions**, and whether shell tool calls are recorded verbatim —
   the [marker](MARKER.md) is only as reliable as that record.
2. **Implement `ConversationReader`** in `src/adapters/<name>ConversationReader.ts`:
   - find marker emissions for `captureId` (reuse `parseMarker` from
     [`readers/markerReader.ts`](../src/adapters/readers/markerReader.ts));
   - bind each marker to a conversation id and a label;
   - emit events with `role` (`user|assistant|tool`), `kind`
     (`query|agent_message|command|tool_call|tool_result`), `ts`, `content` and a `meta` bag;
   - keep each conversation's original message order — the use case orders across
     conversations, never within one;
   - report anomalies as warnings (unbindable marker, truncated read), never silently.
3. **Open the store read-only.** Never write to a live session store.
4. **Wire it** into `readerFor` and `parseSource` in [`src/cli.ts`](../src/cli.ts).
5. **Test it with a fixture store** (see `claudeCodeTranscriptReader.test.ts` for the
   pattern): Given/When/Then, no real user data.
6. Add the runtime's shell tool name to the table in [MARKER.md](MARKER.md#emission).

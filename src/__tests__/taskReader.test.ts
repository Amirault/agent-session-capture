import { describe, it, expect, beforeEach, afterEach } from "vitest";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createFixture, seedTask } from "./fixtures/fixtureDb.js";
import { wrapReadableDb } from "../adapters/sqliteReadableDb.js";
import { readTasks } from "../adapters/readers/taskReader.js";
import { encodeString, encodeMessage, encodeVarint, encodeTag } from "./fixtures/protobuf.js";

const CID = "c-1";

function contextEnvelopeTask(
  turn: number,
  ruleContent = "# Shared project rules\nKeep tests focused."
): Buffer {
  const ruleFile = Buffer.concat([
    encodeString(1, "/repo/AGENTS.md"),
    encodeString(2, ruleContent),
  ]);
  const projectRules = encodeMessage(2, ruleFile);
  const skills = Buffer.concat([
    encodeMessage(
      1,
      Buffer.concat([encodeString(1, "skills/plan/SKILL.md"), encodeString(2, "plan")])
    ),
    encodeMessage(
      1,
      Buffer.concat([encodeString(1, "skills/review/SKILL.md"), encodeString(2, "review")])
    ),
  ]);
  const inputContext = Buffer.concat([
    encodeMessage(10, projectRules),
    encodeMessage(12, skills),
  ]);
  const envelope = encodeMessage(
    5,
    encodeMessage(
      5,
      Buffer.concat([encodeString(1, `context-${turn}`), encodeMessage(11, inputContext)])
    )
  );
  const mixedEnvelope = encodeMessage(
    5,
    encodeMessage(
      5,
      Buffer.concat([
        encodeString(1, `mixed-${turn}`),
        encodeMessage(2, encodeString(1, `output ${turn}`)),
        encodeMessage(11, inputContext),
      ])
    )
  );
  const userQuery = encodeMessage(5, encodeMessage(2, encodeString(1, `query ${turn}`)));
  const agentReasoning = encodeMessage(
    5,
    encodeMessage(15, encodeString(1, `reasoning ${turn}`))
  );
  const updateTodos = encodeMessage(
    5,
    encodeMessage(10, encodeMessage(1, encodeMessage(1, encodeString(2, `todo ${turn}`))))
  );
  const receivedMessage = encodeMessage(
    5,
    encodeMessage(24, encodeMessage(1, encodeString(4, `agent update ${turn}`)))
  );
  const toolCall = encodeMessage(
    5,
    encodeMessage(
      4,
      Buffer.concat([
        encodeString(1, `command-${turn}`),
        encodeMessage(2, encodeString(1, `echo turn-${turn}`)),
      ])
    )
  );
  return Buffer.concat([
    envelope,
    mixedEnvelope,
    userQuery,
    agentReasoning,
    updateTodos,
    receivedMessage,
    toolCall,
  ]);
}

describe("§9.6 taskReader", () => {
  let tmp: string;
  let dbPath: string;

  beforeEach(() => {
    tmp = fs.mkdtempSync(path.join(os.tmpdir(), "wa-96-"));
    dbPath = path.join(tmp, "f.db");
  });
  afterEach(() => {
    fs.rmSync(tmp, { recursive: true, force: true });
  });

  it("Given a Task BLOB with description, dependencies, and summary, When read, Then agent_message events carry semantic field_paths", () => {
    // Given — Task: description(1), dependencies.parent_task_id(3.1), summary(6)
    const db = createFixture(dbPath);
    const blob = Buffer.concat([
      encodeString(1, "Task title"),
      encodeMessage(3, encodeString(1, "parent-task-id")),
      encodeString(6, "command output"),
    ]);
    seedTask(db, {
      conversation_id: CID,
      task: blob,
      last_modified_at: "2026-06-30 12:00:00.000000",
    });

    // When
    const { drafts, skipped } = readTasks(wrapReadableDb(db), [CID]);

    // Then
    expect(skipped).toEqual([]);
    expect(drafts.map((d) => d.content)).toEqual([
      "Task title",
      "parent-task-id",
      "command output",
    ]);
    expect(drafts[0]).toMatchObject({
      conversation_id: CID,
      role: "assistant",
      kind: "agent_message",
      ts: "2026-06-30 12:00:00.000000",
    });
    // field 1 = Task.id, field 3 = Task.dependencies, field 6 = Task.summary
    expect(drafts[0]!.meta.field_path).toBe("id");
    expect(drafts[1]!.meta.field_path).toBe("dependencies.parent_task_id");
    expect(drafts.map((draft) => draft.meta.task_id)).toEqual([
      `${CID}-task`,
      `${CID}-task`,
      `${CID}-task`,
    ]);
    expect(drafts.map((draft) => draft.meta.task_event_index)).toEqual([0, 1, 2]);
    expect(drafts[0]!.meta).not.toHaveProperty("confidence");
    db.close();
  });

  it("Given repeated static context envelopes across task rows, When read, Then the envelope appears once and every signal event survives", () => {
    // Given
    const db = createFixture(dbPath);
    for (let turn = 1; turn <= 3; turn++) {
      seedTask(db, {
        conversation_id: CID,
        task_id: `task-${turn}`,
        task: contextEnvelopeTask(turn),
        last_modified_at: `2026-06-30 12:0${turn}:00.000000`,
      });
    }

    // When
    const { drafts } = readTasks(wrapReadableDb(db), [CID]);

    // Then
    expect(drafts.filter((draft) => Array.isArray(draft.meta.skills))).toHaveLength(1);
    expect(
      drafts.filter((draft) =>
        Object.keys((draft.meta.fields as Record<string, unknown> | undefined) ?? {}).some(
          (field) => field.startsWith("context.project_rules.")
        )
      )
    ).toHaveLength(1);
    for (const messageKind of [
      "user_query",
      "agent_reasoning",
      "update_todos",
      "messages_received_from_agents",
    ]) {
      expect(drafts.filter((draft) => draft.meta.message_kind === messageKind)).toHaveLength(3);
    }
    expect(drafts).toHaveLength(19);
    expect(
      drafts.filter(
        (draft) =>
          draft.meta.message_kind === "tool_call" && draft.meta.tool === "run_shell_command"
      )
    ).toHaveLength(3);
    const commandOutputs = drafts.filter((draft) => {
      const fields = draft.meta.fields as Record<string, string> | undefined;
      return fields?.["run_shell_command.output"] !== undefined;
    });
    expect(commandOutputs).toHaveLength(3);
    expect(
      commandOutputs.map(
        (draft) =>
          (draft.meta.fields as Record<string, string>)["run_shell_command.output"]
      )
    ).toEqual(["output 1", "output 2", "output 3"]);
    expect(drafts.map((draft) => draft.content)).toEqual(
      expect.arrayContaining([
        "query 1",
        "query 2",
        "query 3",
        "reasoning 1",
        "reasoning 2",
        "reasoning 3",
        "todo 1",
        "todo 2",
        "todo 3",
        "agent update 1",
        "agent update 2",
        "agent update 3",
      ])
    );
    db.close();
  });

  it("Given project rules change between task rows, When read, Then both rule versions survive", () => {
    // Given
    const db = createFixture(dbPath);
    seedTask(db, {
      conversation_id: CID,
      task_id: "task-1",
      task: contextEnvelopeTask(1, "rules version one"),
      last_modified_at: "2026-06-30 12:01:00.000000",
    });
    seedTask(db, {
      conversation_id: CID,
      task_id: "task-2",
      task: contextEnvelopeTask(2, "rules version two"),
      last_modified_at: "2026-06-30 12:02:00.000000",
    });

    // When
    const { drafts } = readTasks(wrapReadableDb(db), [CID]);

    // Then
    const ruleContents = drafts.flatMap((draft) => {
      const fields = draft.meta.fields as Record<string, string | string[]> | undefined;
      const content = fields?.["context.project_rules.active_rule_files.content"];
      return content === undefined ? [] : [content];
    });
    expect(ruleContents).toEqual(["rules version one", "rules version two"]);
    db.close();
  });

  it("Given a malformed task BLOB, When read, Then partial results are returned marked confidence:heuristic without throwing", () => {
    // Given — a recoverable string then a truncated length-delimited field
    const db = createFixture(dbPath);
    const blob = Buffer.concat([
      encodeString(1, "recovered text"),
      Buffer.concat([encodeTag(2, 2), encodeVarint(100), Buffer.from("abc")]),
    ]);
    seedTask(db, { conversation_id: CID, task: blob });

    // When
    const { drafts } = readTasks(wrapReadableDb(db), [CID]);

    // Then
    const ev = drafts.find((d) => d.content === "recovered text");
    expect(ev).toBeDefined();
    expect(ev!.meta.field_path).toBe("id");
    expect(ev!.meta.confidence).toBe("heuristic");
    db.close();
  });

  it("Given an empty task BLOB, When read, Then the row is skipped and the run continues", () => {
    // Given
    const db = createFixture(dbPath);
    seedTask(db, { conversation_id: CID, task: Buffer.alloc(0) });

    // When
    const { drafts, skipped } = readTasks(wrapReadableDb(db), [CID]);

    // Then
    expect(drafts).toEqual([]);
    expect(skipped).toHaveLength(1);
    expect(skipped[0]!.table).toBe("agent_tasks");
    expect(skipped[0]!.reason).toBe("empty task");
    db.close();
  });

  it("Given no conversation ids, When reading, Then empty results (no empty IN clause)", () => {
    const db = createFixture(dbPath);
    expect(readTasks(wrapReadableDb(db), [])).toEqual({ drafts: [], skipped: [] });
    db.close();
  });
});

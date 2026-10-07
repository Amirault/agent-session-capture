// Minimal hand-trimmed schema for tests: only the field numbers the test suite exercises.
// The full Warp schema is not vendored (AGPL-3.0 upstream); see `npm run gen:schema`.
import type { Schema } from "../../adapters/protoSchema.js";

export { SCHEMA_REV, type FieldInfo, type Schema } from "../../adapters/protoSchema.js";

export const ROOT_TYPE = "warp.multi_agent.v1.Task";

export const PROTO_SCHEMA: Schema = {
  "warp.multi_agent.v1.CreateTodoList": {
    "1": {
      "name": "initial_todos",
      "kind": "message",
      "child": "warp.multi_agent.v1.TodoItem"
    }
  },
  "warp.multi_agent.v1.FileContent": {
    "1": {
      "name": "file_path",
      "kind": "leaf"
    },
    "2": {
      "name": "content",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.InputContext": {
    "10": {
      "name": "project_rules",
      "kind": "message",
      "child": "warp.multi_agent.v1.InputContext.ProjectRules"
    },
    "12": {
      "name": "updated_skills_context",
      "kind": "message",
      "child": "warp.multi_agent.v1.InputContext.SkillsContext"
    }
  },
  "warp.multi_agent.v1.InputContext.ProjectRules": {
    "2": {
      "name": "active_rule_files",
      "kind": "message",
      "child": "warp.multi_agent.v1.FileContent"
    }
  },
  "warp.multi_agent.v1.InputContext.SkillsContext": {
    "1": {
      "name": "available_skills",
      "kind": "message",
      "child": "warp.multi_agent.v1.SkillDescriptor"
    }
  },
  "warp.multi_agent.v1.Message": {
    "2": {
      "name": "user_query",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.UserQuery",
      "oneof": "message"
    },
    "3": {
      "name": "agent_output",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.AgentOutput",
      "oneof": "message"
    },
    "4": {
      "name": "tool_call",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.ToolCall",
      "oneof": "message"
    },
    "5": {
      "name": "tool_call_result",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.ToolCallResult",
      "oneof": "message"
    },
    "10": {
      "name": "update_todos",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.UpdateTodos",
      "oneof": "message"
    },
    "15": {
      "name": "agent_reasoning",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.AgentReasoning",
      "oneof": "message"
    },
    "24": {
      "name": "messages_received_from_agents",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.MessagesReceivedFromAgents",
      "oneof": "message"
    }
  },
  "warp.multi_agent.v1.Message.AgentOutput": {
    "1": {
      "name": "text",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Message.AgentReasoning": {
    "1": {
      "name": "reasoning",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Message.MessagesReceivedFromAgents": {
    "1": {
      "name": "messages",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.MessagesReceivedFromAgents.ReceivedMessage"
    }
  },
  "warp.multi_agent.v1.Message.MessagesReceivedFromAgents.ReceivedMessage": {
    "4": {
      "name": "subject",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Message.ToolCall": {
    "1": {
      "name": "tool_call_id",
      "kind": "leaf"
    },
    "2": {
      "name": "run_shell_command",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.ToolCall.RunShellCommand",
      "oneof": "tool"
    },
    "6": {
      "name": "apply_file_diffs",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs",
      "oneof": "tool"
    }
  },
  "warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs": {
    "2": {
      "name": "diffs",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs.FileDiff"
    }
  },
  "warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs.FileDiff": {
    "1": {
      "name": "file_path",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Message.ToolCall.RunShellCommand": {
    "1": {
      "name": "command",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Message.ToolCallResult": {
    "1": {
      "name": "tool_call_id",
      "kind": "leaf"
    },
    "2": {
      "name": "run_shell_command",
      "kind": "message",
      "child": "warp.multi_agent.v1.RunShellCommandResult",
      "oneof": "result"
    },
    "11": {
      "name": "context",
      "kind": "message",
      "child": "warp.multi_agent.v1.InputContext"
    }
  },
  "warp.multi_agent.v1.Message.UpdateTodos": {
    "1": {
      "name": "create_todo_list",
      "kind": "message",
      "child": "warp.multi_agent.v1.CreateTodoList",
      "oneof": "operation"
    }
  },
  "warp.multi_agent.v1.Message.UserQuery": {
    "1": {
      "name": "query",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.RunShellCommandResult": {
    "1": {
      "name": "output",
      "kind": "leaf"
    },
    "3": {
      "name": "command",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.SkillDescriptor": {
    "1": {
      "name": "path",
      "kind": "leaf",
      "oneof": "skill_reference"
    },
    "2": {
      "name": "name",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Task": {
    "1": {
      "name": "id",
      "kind": "leaf"
    },
    "3": {
      "name": "dependencies",
      "kind": "message",
      "child": "warp.multi_agent.v1.Task.Dependencies"
    },
    "5": {
      "name": "messages",
      "kind": "message",
      "child": "warp.multi_agent.v1.Message"
    },
    "6": {
      "name": "summary",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.Task.Dependencies": {
    "1": {
      "name": "parent_task_id",
      "kind": "leaf"
    }
  },
  "warp.multi_agent.v1.TodoItem": {
    "2": {
      "name": "title",
      "kind": "leaf"
    }
  }
};

//! Minimal hand-trimmed schema for tests: only the field numbers the test suite
//! exercises (the full Warp schema is generated on demand, never vendored).

use crate::adapters::proto_schema::{FieldInfo, Kind, Schema, TypeEntry};

pub static TEST_SCHEMA: Schema = Schema { types: TYPES };

static TYPES: &[TypeEntry] = &[
    TypeEntry {
        name: "warp.multi_agent.v1.CreateTodoList",
        fields: &[FieldInfo {
            number: 1,
            name: "initial_todos",
            kind: Kind::Message,
            child: Some("warp.multi_agent.v1.TodoItem"),
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.FileContent",
        fields: &[
            FieldInfo {
                number: 1,
                name: "file_path",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
            FieldInfo {
                number: 2,
                name: "content",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.InputContext",
        fields: &[
            FieldInfo {
                number: 10,
                name: "project_rules",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.InputContext.ProjectRules"),
                oneof: None,
            },
            FieldInfo {
                number: 12,
                name: "updated_skills_context",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.InputContext.SkillsContext"),
                oneof: None,
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.InputContext.ProjectRules",
        fields: &[FieldInfo {
            number: 2,
            name: "active_rule_files",
            kind: Kind::Message,
            child: Some("warp.multi_agent.v1.FileContent"),
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.InputContext.SkillsContext",
        fields: &[FieldInfo {
            number: 1,
            name: "available_skills",
            kind: Kind::Message,
            child: Some("warp.multi_agent.v1.SkillDescriptor"),
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message",
        fields: &[
            FieldInfo {
                number: 2,
                name: "user_query",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.UserQuery"),
                oneof: Some("message"),
            },
            FieldInfo {
                number: 3,
                name: "agent_output",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.AgentOutput"),
                oneof: Some("message"),
            },
            FieldInfo {
                number: 4,
                name: "tool_call",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.ToolCall"),
                oneof: Some("message"),
            },
            FieldInfo {
                number: 5,
                name: "tool_call_result",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.ToolCallResult"),
                oneof: Some("message"),
            },
            FieldInfo {
                number: 10,
                name: "update_todos",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.UpdateTodos"),
                oneof: Some("message"),
            },
            FieldInfo {
                number: 15,
                name: "agent_reasoning",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.AgentReasoning"),
                oneof: Some("message"),
            },
            FieldInfo {
                number: 24,
                name: "messages_received_from_agents",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.MessagesReceivedFromAgents"),
                oneof: Some("message"),
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.AgentOutput",
        fields: &[FieldInfo {
            number: 1,
            name: "text",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.AgentReasoning",
        fields: &[FieldInfo {
            number: 1,
            name: "reasoning",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.MessagesReceivedFromAgents",
        fields: &[FieldInfo {
            number: 1,
            name: "messages",
            kind: Kind::Message,
            child: Some("warp.multi_agent.v1.Message.MessagesReceivedFromAgents.ReceivedMessage"),
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.MessagesReceivedFromAgents.ReceivedMessage",
        fields: &[FieldInfo {
            number: 4,
            name: "subject",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.ToolCall",
        fields: &[
            FieldInfo {
                number: 1,
                name: "tool_call_id",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
            FieldInfo {
                number: 2,
                name: "run_shell_command",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.ToolCall.RunShellCommand"),
                oneof: Some("tool"),
            },
            FieldInfo {
                number: 6,
                name: "apply_file_diffs",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs"),
                oneof: Some("tool"),
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs",
        fields: &[FieldInfo {
            number: 2,
            name: "diffs",
            kind: Kind::Message,
            child: Some("warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs.FileDiff"),
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.ToolCall.ApplyFileDiffs.FileDiff",
        fields: &[FieldInfo {
            number: 1,
            name: "file_path",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.ToolCall.RunShellCommand",
        fields: &[FieldInfo {
            number: 1,
            name: "command",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.ToolCallResult",
        fields: &[
            FieldInfo {
                number: 1,
                name: "tool_call_id",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
            FieldInfo {
                number: 2,
                name: "run_shell_command",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.RunShellCommandResult"),
                oneof: Some("result"),
            },
            FieldInfo {
                number: 11,
                name: "context",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.InputContext"),
                oneof: None,
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.UpdateTodos",
        fields: &[FieldInfo {
            number: 1,
            name: "create_todo_list",
            kind: Kind::Message,
            child: Some("warp.multi_agent.v1.CreateTodoList"),
            oneof: Some("operation"),
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Message.UserQuery",
        fields: &[FieldInfo {
            number: 1,
            name: "query",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.RunShellCommandResult",
        fields: &[
            FieldInfo {
                number: 1,
                name: "output",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
            FieldInfo {
                number: 3,
                name: "command",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.SkillDescriptor",
        fields: &[
            FieldInfo {
                number: 1,
                name: "path",
                kind: Kind::Leaf,
                child: None,
                oneof: Some("skill_reference"),
            },
            FieldInfo {
                number: 2,
                name: "name",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Task",
        fields: &[
            FieldInfo {
                number: 1,
                name: "id",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
            FieldInfo {
                number: 3,
                name: "dependencies",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Task.Dependencies"),
                oneof: None,
            },
            FieldInfo {
                number: 5,
                name: "messages",
                kind: Kind::Message,
                child: Some("warp.multi_agent.v1.Message"),
                oneof: None,
            },
            FieldInfo {
                number: 6,
                name: "summary",
                kind: Kind::Leaf,
                child: None,
                oneof: None,
            },
        ],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.Task.Dependencies",
        fields: &[FieldInfo {
            number: 1,
            name: "parent_task_id",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
    TypeEntry {
        name: "warp.multi_agent.v1.TodoItem",
        fields: &[FieldInfo {
            number: 2,
            name: "title",
            kind: Kind::Leaf,
            child: None,
            oneof: None,
        }],
    },
];

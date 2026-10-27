//! Synthetic `oz run get` / `oz run conversation get` JSON (invented ids,
//! prompts and paths), shaped like Warp's output.

use crate::js::{Object, Value, json};

pub const RUN_ID: &str = "run-0001";
pub const CONVERSATION_ID: &str = "conv-0001";
pub const MODEL_ID: &str = "model-large";

fn object(text: &str) -> Object {
    match json::parse(text).expect("fixture JSON") {
        Value::Object(o) => o,
        other => panic!("fixture is not an object: {other:?}"),
    }
}

/// `oz run get <run id> --output-format json`.
pub fn run_json(run_id: &str, conversation_id: &str, updated_at: &str) -> String {
    format!(
        r#"{{"run_id":"{run_id}","conversation_id":"{conversation_id}","state":"SUCCEEDED","title":"Fix the greeting","created_at":"2026-10-20T09:00:00.000000Z","updated_at":"{updated_at}","agent_config":{{"model_id":"{MODEL_ID}"}},"request_usage":{{"inference_cost_usd":0.25,"total_tokens":1500,"model_token_usage":[{{"model_id":"{MODEL_ID}","total_tokens":1500,"tokens_by_category":{{"primary_agent":1500}}}}]}}}}"#
    )
}

pub fn run(run_id: &str, conversation_id: &str, updated_at: &str) -> Object {
    object(&run_json(run_id, conversation_id, updated_at))
}

/// One message of a conversation step.
pub fn message(role: &str, timestamp: &str, request_id: &str, content: &[String]) -> String {
    format!(
        r#"{{"role":"{role}","timestamp":"{timestamp}","request_id":"{request_id}","message_ids":["m-{timestamp}"],"content":[{}]}}"#,
        content.join(",")
    )
}

pub fn text(text: &str) -> String {
    format!(r#"{{"type":"text","text":{}}}"#, json::quote(text))
}

pub fn action(id: &str, name: &str, input: &str) -> String {
    format!(r#"{{"type":"action","id":"{id}","name":"{name}","category":"command","input":{input}}}"#)
}

pub fn run_command(id: &str, command: &str) -> String {
    action(
        id,
        "run_command",
        &format!(r#"{{"command":{},"uses_pager":false}}"#, json::quote(command)),
    )
}

pub fn command_result(action_id: &str, command: &str, output: &str, exit_code: i64) -> String {
    format!(
        r#"{{"type":"action_result","action_id":"{action_id}","name":"run_command","state":"completed","output":{{"command":{},"command_finished":{{"output":{},"exit_code":{exit_code}}}}}}}"#,
        json::quote(command),
        json::quote(output)
    )
}

pub fn action_result(action_id: &str, name: &str, output: &str) -> String {
    format!(
        r#"{{"type":"action_result","action_id":"{action_id}","name":"{name}","state":"completed","output":{output}}}"#
    )
}

/// `oz run conversation get <conversation id> --output-format json`.
pub fn conversation_json(conversation_id: &str, steps: &[String]) -> String {
    format!(
        r#"{{"conversation_id":"{conversation_id}","steps":[{}]}}"#,
        steps.join(",")
    )
}

pub fn step(messages: &[String], nested: &[String]) -> String {
    format!(
        r#"{{"id":"step","description":"step","messages":[{}],"steps":[{}]}}"#,
        messages.join(","),
        nested.join(",")
    )
}

pub fn conversation(conversation_id: &str, steps: &[String]) -> Object {
    object(&conversation_json(conversation_id, steps))
}

/// A marked conversation: prompt, answer + marker call, its result, a file
/// read and its result.
pub fn marked_conversation(conversation_id: &str, marker: &str) -> Object {
    conversation(
        conversation_id,
        &[step(
            &[
                message(
                    "user",
                    "2026-10-20T09:00:01.5Z",
                    "req-1",
                    &[text("Make the greeting say hello")],
                ),
                message(
                    "assistant",
                    "2026-10-20T09:00:02.123456789Z",
                    "req-1",
                    &[text("Tagging the session first."), run_command("call-1", marker)],
                ),
                message(
                    "tool",
                    "2026-10-20T09:00:03Z",
                    "req-1",
                    &[command_result("call-1", marker, "", 0)],
                ),
                message(
                    "assistant",
                    "2026-10-20T09:00:04.25Z",
                    "req-2",
                    &[action(
                        "call-2",
                        "read_files",
                        r#"{"files":[{"name":"src/greeting.txt"}]}"#,
                    )],
                ),
                message(
                    "tool",
                    "2026-10-20T09:00:05Z",
                    "req-2",
                    &[action_result(
                        "call-2",
                        "read_files",
                        r#"{"success":{"files":[{"file":{"path":"src/greeting.txt","content":"hi"}}]}}"#,
                    )],
                ),
            ],
            &[],
        )],
    )
}

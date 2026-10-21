//! Validation of the JSON columns Warp stores.
//!
//! Parsing is lenient on shape: unknown extra keys pass through, but the
//! fields the readers bind on must be present and correctly typed. Rows that
//! fail are reported and skipped — never silently dropped, never crash the
//! run. The error text is the issue list the original zod schemas produced
//! (`JSON.stringify(issues, null, 2)`), since it is printed in skipped-row lines.

use crate::js::json::{self, quote};
use crate::js::{Object, Value};

/// One validation issue (`invalid_type` or `too_small`).
enum Issue {
    InvalidType {
        expected: &'static str,
        path: Vec<PathSegment>,
        received: &'static str,
    },
    TooSmallArray {
        path: Vec<PathSegment>,
    },
}

#[derive(Clone)]
enum PathSegment {
    Index(usize),
    Key(&'static str),
}

fn received(value: Option<&Value>) -> &'static str {
    match value {
        None => "undefined",
        Some(Value::Null) => "null",
        Some(Value::Bool(_)) => "boolean",
        Some(Value::Number(_)) => "number",
        Some(Value::String(_)) => "string",
        Some(Value::Array(_)) => "array",
        Some(Value::Object(_)) => "object",
    }
}

fn format_path(path: &[PathSegment], indent: &str) -> String {
    if path.is_empty() {
        return "[]".to_string();
    }
    let items: Vec<String> = path
        .iter()
        .map(|segment| match segment {
            PathSegment::Index(i) => format!("{indent}    {i}"),
            PathSegment::Key(k) => format!("{indent}    {}", quote(k)),
        })
        .collect();
    format!("[\n{}\n{indent}  ]", items.join(",\n"))
}

fn format_issues(issues: &[Issue]) -> String {
    let blocks: Vec<String> = issues
        .iter()
        .map(|issue| match issue {
            Issue::InvalidType { expected, path, received } => format!(
                "  {{\n    \"expected\": \"{expected}\",\n    \"code\": \"invalid_type\",\n    \"path\": {},\n    \"message\": \"Invalid input: expected {expected}, received {received}\"\n  }}",
                format_path(path, "  ")
            ),
            Issue::TooSmallArray { path } => format!(
                "  {{\n    \"origin\": \"array\",\n    \"code\": \"too_small\",\n    \"minimum\": 1,\n    \"inclusive\": true,\n    \"path\": {},\n    \"message\": \"Too small: expected array to have >=1 items\"\n  }}",
                format_path(path, "  ")
            ),
        })
        .collect();
    format!("[\n{}\n]", blocks.join(",\n"))
}

/// `JSON.parse` then validate, collapsing both failure modes into one message.
fn parse_json(raw: &str) -> Result<Value, String> {
    json::parse(raw).map_err(|message| format!("invalid JSON: {message}"))
}

/// `ai_queries.input` — a non-empty JSON array whose elements carry the
/// prompt under `Query.text`: `[{"Query":{"text":"...","context":...}}]`.
/// Returns the first element's prompt text.
pub fn parse_query_input(raw: &str) -> Result<String, String> {
    let value = parse_json(raw)?;
    let Value::Array(items) = &value else {
        return Err(format_issues(&[Issue::InvalidType {
            expected: "array",
            path: vec![],
            received: received(Some(&value)),
        }]));
    };
    let mut issues = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Value::Object(element) = item else {
            issues.push(Issue::InvalidType {
                expected: "object",
                path: vec![PathSegment::Index(i)],
                received: received(Some(item)),
            });
            continue;
        };
        let query = element.get("Query");
        let Some(Value::Object(query)) = query else {
            issues.push(Issue::InvalidType {
                expected: "object",
                path: vec![PathSegment::Index(i), PathSegment::Key("Query")],
                received: received(query),
            });
            continue;
        };
        let text = query.get("text");
        if !matches!(text, Some(Value::String(_))) {
            issues.push(Issue::InvalidType {
                expected: "string",
                path: vec![
                    PathSegment::Index(i),
                    PathSegment::Key("Query"),
                    PathSegment::Key("text"),
                ],
                received: received(text),
            });
        }
    }
    if items.is_empty() {
        issues.push(Issue::TooSmallArray { path: vec![] });
    }
    if !issues.is_empty() {
        return Err(format_issues(&issues));
    }
    let text = items[0]
        .as_object()
        .and_then(|e| e.get("Query"))
        .and_then(Value::as_object)
        .and_then(|q| q.get_str("text"));
    Ok(text.expect("validated").to_string())
}

/// `blocks.ai_metadata` — binds a block to a conversation (and tags subagent
/// work). `None` for a NULL column.
pub struct BlockAiMetadata {
    pub conversation_id: Option<String>,
    pub subagent_task_id: Option<String>,
}

pub fn parse_block_ai_metadata(raw: Option<&str>) -> Option<Result<BlockAiMetadata, String>> {
    let raw = raw?;
    Some(parse_block(raw))
}

fn parse_block(raw: &str) -> Result<BlockAiMetadata, String> {
    let value = parse_json(raw)?;
    let Value::Object(object) = &value else {
        return Err(format_issues(&[Issue::InvalidType {
            expected: "object",
            path: vec![],
            received: received(Some(&value)),
        }]));
    };
    let mut issues = Vec::new();
    let field = |object: &Object, key: &'static str, issues: &mut Vec<Issue>| -> Option<String> {
        match object.get(key) {
            None | Some(Value::Null) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(other) => {
                issues.push(Issue::InvalidType {
                    expected: "string",
                    path: vec![PathSegment::Key(key)],
                    received: received(Some(other)),
                });
                None
            }
        }
    };
    let conversation_id = field(object, "conversation_id", &mut issues);
    let subagent_task_id = field(object, "subagent_task_id", &mut issues);
    if !issues.is_empty() {
        return Err(format_issues(&issues));
    }
    Ok(BlockAiMetadata {
        conversation_id,
        subagent_task_id,
    })
}

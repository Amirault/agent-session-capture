//! Domain models.
//!
//! The tool reads local agent session stores and exports, raw and time-ordered,
//! every conversation tagged with one capture id through a CAPTURE_MARKER.
//! These types are pure (no I/O) so they can be reused by every port and
//! adapter.
//!
//! Written events and headers are JSON objects ([`Object`]): a stored bundle is
//! merged back byte for byte, extra keys and key order included.

use crate::js::{Object, Value};

/// A free-form tag a marker may carry (`label=<label>`) to distinguish several
/// conversations of one capture id. Markers without a label get [`DEFAULT_LABEL`].
pub type Label = String;
pub const DEFAULT_LABEL: &str = "default";

/// Who emitted an event in the conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }
}

/// Kind of conversation event.
/// - query        : a user prompt
/// - agent_message: assistant text
/// - command      : a shell command execution (Warp blocks)
/// - tool_call    : a tool invocation
/// - tool_result  : a tool/command output
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Query,
    AgentMessage,
    Command,
    ToolCall,
    ToolResult,
}

impl EventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            EventKind::Query => "query",
            EventKind::AgentMessage => "agent_message",
            EventKind::Command => "command",
            EventKind::ToolCall => "tool_call",
            EventKind::ToolResult => "tool_result",
        }
    }
}

/// A reader-produced event before the use case attaches capture_id, label, and seq.
#[derive(Clone, Debug, PartialEq)]
pub struct EventDraft {
    pub conversation_id: String,
    /// ISO-8601 (or source-native) timestamp.
    pub ts: String,
    pub role: Role,
    pub kind: EventKind,
    pub content: String,
    pub meta: Object,
}

/// One written event line: a `ConversationEvent` (marker mode:
/// `capture_id, label, conversation_id, seq, ts, role, kind, content, meta`)
/// or a `SessionEvent` (session mode: the same without `capture_id`/`label`).
pub type EventRecord = Object;

/// Build a marker-mode `ConversationEvent` from a draft.
pub fn conversation_event(capture_id: &str, label: &str, draft: &EventDraft, seq: usize) -> EventRecord {
    let mut event = Object::new();
    event.insert("capture_id", capture_id);
    event.insert("label", label);
    insert_draft_fields(&mut event, draft, seq);
    event
}

/// Build a session-mode `SessionEvent` from a draft.
pub fn session_event(draft: &EventDraft, seq: usize) -> EventRecord {
    let mut event = Object::new();
    insert_draft_fields(&mut event, draft, seq);
    event
}

fn insert_draft_fields(event: &mut Object, draft: &EventDraft, seq: usize) {
    event.insert("conversation_id", draft.conversation_id.as_str());
    event.insert("seq", seq);
    event.insert("ts", draft.ts.as_str());
    event.insert("role", draft.role.as_str());
    event.insert("kind", draft.kind.as_str());
    event.insert("content", draft.content.as_str());
    event.insert("meta", draft.meta.clone());
}

/// The source a bundle or session file was read from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleSource {
    Warp,
    ClaudeCode,
    Hermes,
    Langfuse,
}

impl BundleSource {
    pub fn as_str(self) -> &'static str {
        match self {
            BundleSource::Warp => "warp",
            BundleSource::ClaudeCode => "claude-code",
            BundleSource::Hermes => "hermes",
            BundleSource::Langfuse => "langfuse",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "warp" => Some(BundleSource::Warp),
            "claude-code" => Some(BundleSource::ClaudeCode),
            "hermes" => Some(BundleSource::Hermes),
            "langfuse" => Some(BundleSource::Langfuse),
            _ => None,
        }
    }
}

/// A bundle: line 1 is the `bundle_header` object
/// (`type, capture_id, labels, conversations_per_label, conversation_ids,
/// source, extracted_at`), then one event per line.
#[derive(Clone, Debug, PartialEq)]
pub struct CaptureBundle {
    pub header: Object,
    pub events: Vec<EventRecord>,
}

impl CaptureBundle {
    /// `header.conversation_ids.length` (0 when not an array).
    pub fn conversation_count(&self) -> usize {
        self.header
            .get("conversation_ids")
            .and_then(Value::as_array)
            .map_or(0, Vec::len)
    }
}

/// First line of every `sessions/<file>.jsonl` file.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionHeader {
    pub conversation_id: String,
    /// The header's `source` value as stored (any JSON value).
    pub source: Value,
    /// Source-native last-activity stamp; `None` means "no durable stamp".
    pub last_modified: Option<String>,
    /// The parsed line, for writing it back unchanged.
    pub raw: Object,
}

impl SessionHeader {
    /// A fresh header: `type, conversation_id, source, last_modified, extracted_at`.
    pub fn fresh(conversation_id: &str, source: BundleSource, last_modified: Option<&str>, extracted_at: &str) -> Self {
        let mut raw = Object::new();
        raw.insert("type", "session_header");
        raw.insert("conversation_id", conversation_id);
        raw.insert("source", source.as_str());
        raw.insert("last_modified", last_modified.map_or(Value::Null, Value::from));
        raw.insert("extracted_at", extracted_at);
        Self {
            conversation_id: conversation_id.to_string(),
            source: Value::from(source.as_str()),
            last_modified: last_modified.map(str::to_string),
            raw,
        }
    }
}

/// A stored session file: its header (line 1) plus its events (lines 2..N).
#[derive(Clone, Debug)]
pub struct SessionFile {
    pub header: SessionHeader,
    pub events: Vec<EventRecord>,
}

/// One enumerated session, before any events are gathered for it.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionInfo {
    pub conversation_id: String,
    /// Source-native last-activity stamp; `None` means "no durable stamp".
    pub last_modified: Option<String>,
}

/// Uniform return shape for every Warp sub-reader.
#[derive(Clone, Debug, Default)]
pub struct ReaderResult {
    pub drafts: Vec<EventDraft>,
    pub skipped: Vec<SkippedRow>,
}

/// Result of binding one marker emission to a conversation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedStatus {
    Bound,
    Unbindable,
    Collision,
}

/// How confident we are in a bound binding (absent means "certain").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SeedConfidence {
    Certain,
    Heuristic,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeedMatch {
    /// The conversation id, or `None` when the marker could not be bound.
    pub conversation_id: Option<String>,
    pub label: Label,
    pub marker_command: String,
    pub start_ts: String,
    pub status: SeedStatus,
    pub confidence: Option<SeedConfidence>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SkippedRow {
    pub table: String,
    pub reason: String,
    pub detail: String,
}

impl SkippedRow {
    pub fn new(table: &str, reason: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            table: table.to_string(),
            reason: reason.into(),
            detail: detail.into(),
        }
    }
}

/// Human-readable run summary (also returned to the CLI).
#[derive(Clone, Debug, Default)]
pub struct RunSummary {
    pub capture_id: String,
    pub conversations: usize,
    pub events: usize,
    pub unbindable: Vec<SeedMatch>,
    pub collisions: Vec<SeedMatch>,
    /// Seeds bound via the ai_queries temporal-proximity fallback.
    pub heuristic_bindings: Vec<SeedMatch>,
    pub skipped_rows: Vec<SkippedRow>,
    /// Set when the fresh read failed and the result fell back to a stored bundle.
    pub fresh_read_error: Option<String>,
}

/// One file written by a session-mode run.
#[derive(Clone, Debug, PartialEq)]
pub struct WrittenSession {
    pub conversation_id: String,
    /// Path actually written (`<outDir>/sessions/<file>.jsonl`).
    pub path: String,
    pub events: usize,
}

/// Result of one `--sessions` run (feeds `format_session_report`).
#[derive(Clone, Debug, Default)]
pub struct SessionRunSummary {
    /// Raw `--since` value, when given.
    pub since: Option<String>,
    /// Raw `--conversation` value, when given.
    pub conversation: Option<String>,
    /// Sessions enumerated in this run, after the --since/--conversation filters.
    pub enumerated: usize,
    /// Sessions written or rewritten this run, in write order.
    pub written: Vec<WrittenSession>,
    /// Conversation ids whose stored stamp matched — skipped, file untouched.
    pub unchanged: Vec<String>,
    /// Stored files not touched by this run (not in this run's enumeration).
    pub untouched: Vec<String>,
    /// Reader-reported skipped rows (surfaced one per line).
    pub skipped_rows: Vec<SkippedRow>,
    /// Guard warnings downgraded from errors by --no-merge (one per line).
    pub warnings: Vec<String>,
}

/// `String(value)` / template-literal interpolation of a JSON value
/// (`undefined` when absent).
pub fn js_display(value: Option<&Value>) -> String {
    match value {
        None => "undefined".to_string(),
        Some(Value::Null) => "null".to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(Value::Number(n)) => crate::js::number::number_to_string(*n),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => js_display(Some(other)),
            })
            .collect::<Vec<_>>()
            .join(","),
        Some(Value::Object(_)) => "[object Object]".to_string(),
    }
}

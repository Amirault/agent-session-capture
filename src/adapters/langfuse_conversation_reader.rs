//! Marker and sessions modes over a Langfuse project, read-only.
//!
//! A conversation is the `sessionId` of a trace, else its `traceId`. In marker
//! mode it binds to a label through an exact marker line run as the `command`
//! of a `TOOL`/`SPAN` observation named `terminal`, `run_shell_command` or
//! `Bash`. The client is injected, so the reader never touches the network.

use std::collections::{HashMap, HashSet};

use super::langfuse_observation_lister::{LangfuseObservation, LangfuseObservationLister, ObservationQuery};
use crate::domain::capture_marker::{ExactMarker, exact_markers_in_command};
use crate::domain::merge_bundle::stable_sort_by;
use crate::domain::models::{BundleSource, EventDraft, EventKind, Label, Role, SessionInfo, SkippedRow};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{
    CaptureRead, ConversationReader, ReadSessionsOptions, SessionGather, SessionReader, SessionsRead,
};
use crate::domain::skip_unchanged::should_gather;
use crate::domain::source_errors::ReadError;
use crate::domain::stamps::parse_since_utc;
use crate::js::string::locale_compare;
use crate::js::{Object, Value, date, json};

const LOOKBACK_MS: f64 = 30.0 * 24.0 * 60.0 * 60.0 * 1000.0;
const MARKER_TYPES: [&str; 2] = ["TOOL", "SPAN"];
const MARKER_TOOLS: [&str; 3] = ["terminal", "run_shell_command", "Bash"];

pub struct LangfuseConversationReader<C: LangfuseObservationLister> {
    client: C,
    now: Box<dyn Fn() -> f64>,
}

impl<C: LangfuseObservationLister> LangfuseConversationReader<C> {
    pub fn new(client: C) -> Self {
        Self {
            client,
            now: Box::new(date::now_ms),
        }
    }

    /// A reader with a fixed clock (epoch milliseconds), for tests.
    pub fn with_now(client: C, now: impl Fn() -> f64 + 'static) -> Self {
        Self {
            client,
            now: Box::new(now),
        }
    }

    /// A trace whose root started before the cutoff looks up its root, so it
    /// joins its session instead of becoming a session-less trace of its own.
    fn session_roots_of_cut_traces(
        &self,
        activity: &[LangfuseObservation],
    ) -> Result<Vec<LangfuseObservation>, ReadError> {
        let resolved: HashSet<&str> = trace_ids_in(
            activity
                .iter()
                .filter(|o| is_root(o) || truthy_str(o, "sessionId").is_some()),
        )
        .collect();
        let mut cut: Vec<&str> = Vec::new();
        for trace_id in trace_ids_in(activity.iter()) {
            if !resolved.contains(trace_id) && !cut.contains(&trace_id) {
                cut.push(trace_id);
            }
        }
        let mut roots = Vec::new();
        for trace_id in cut {
            let trace = self.client.list_activity(&ObservationQuery {
                trace_id: Some(trace_id.to_string()),
                ..Default::default()
            })?;
            roots.extend(trace.into_iter().filter(|o| truthy_str(o, "sessionId").is_some()));
        }
        Ok(roots)
    }

    fn gather_group(&self, group: &ConversationGroup, windowed: bool) -> Result<SessionGather, ReadError> {
        let mut observations = Vec::new();
        for trace_id in self.trace_ids_of(group, windowed)? {
            observations.extend(self.client.list_observations(&ObservationQuery {
                trace_id: Some(trace_id),
                ..Default::default()
            })?);
        }
        let cid = group.info.conversation_id.clone();
        Ok(SessionGather {
            drafts: drafts_from_observations(&observations, &|_| cid.clone())?,
            skipped: vec![],
            last_modified: group.info.last_modified.clone(),
        })
    }

    /// A windowed activity pass only sees a session's recent traces: its roots name them all.
    fn trace_ids_of(&self, group: &ConversationGroup, windowed: bool) -> Result<Vec<String>, ReadError> {
        if !windowed || !group.is_session {
            return Ok(group.trace_ids.clone());
        }
        let roots = self.client.list_activity(&ObservationQuery {
            session_id: Some(group.info.conversation_id.clone()),
            ..Default::default()
        })?;
        let mut ids: Vec<String> = Vec::new();
        for id in trace_ids_in(roots.iter())
            .map(str::to_string)
            .chain(group.trace_ids.iter().cloned())
        {
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

impl<C: LangfuseObservationLister> ConversationReader for LangfuseConversationReader<C> {
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError> {
        let from = date::to_iso_string((self.now)() - LOOKBACK_MS).expect("valid date");
        let observations = self.client.list_observations(&ObservationQuery {
            from_start_time: Some(from),
            ..Default::default()
        })?;
        let mut skipped = Vec::new();
        let conversation_of = conversations_of(&observations);
        let label_by_cid = bind_conversations(&observations, &conversation_of, capture_id, &mut skipped);
        let bound: Vec<LangfuseObservation> = observations
            .iter()
            .filter(|o| label_by_cid.contains_key(&conversation_of(o)))
            .cloned()
            .collect();
        Ok(CaptureRead {
            source: BundleSource::Langfuse,
            drafts: drafts_from_observations(&bound, &conversation_of)?,
            label_by_cid,
            skipped,
            unbindable: vec![],
            collisions: vec![],
            heuristic_bindings: vec![],
        })
    }
}

impl<C: LangfuseObservationLister> SessionReader for LangfuseConversationReader<C> {
    /// One content-less pass enumerates the conversations active at/after
    /// `--since` (every trace of a session is one group, stamped with the
    /// latest `updatedAt`); only the changed groups are then fetched, trace by
    /// trace.
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError> {
        let cutoff = options.since.as_deref().and_then(parse_since_utc);
        let query = match cutoff {
            None => ObservationQuery::default(),
            Some(cutoff) => ObservationQuery {
                from_start_time: Some(
                    date::to_iso_string(cutoff).ok_or_else(|| ReadError::Other("Invalid time value".to_string()))?,
                ),
                ..Default::default()
            },
        };
        let mut activity = self.client.list_activity(&query)?;
        if cutoff.is_some() {
            let roots = self.session_roots_of_cut_traces(&activity)?;
            activity.extend(roots);
        }
        let mut groups = group_by_conversation(&activity)?;
        if let Some(conversation) = &options.conversation {
            groups.retain(|g| &g.info.conversation_id == conversation);
        }
        let mut reads = OrderedMap::new();
        for group in &groups {
            if !should_gather(&group.info, options) {
                continue;
            }
            reads.insert(
                group.info.conversation_id.clone(),
                self.gather_group(group, cutoff.is_some())?,
            );
        }
        Ok(SessionsRead {
            source: BundleSource::Langfuse,
            sessions: groups.into_iter().map(|g| g.info).collect(),
            reads,
        })
    }
}

struct ConversationGroup {
    info: SessionInfo,
    trace_ids: Vec<String>,
    is_session: bool,
}

/// A string field that is present and non-empty (JavaScript truthiness).
fn truthy_str<'a>(o: &'a Object, key: &str) -> Option<&'a str> {
    o.get_str(key).filter(|s| !s.is_empty())
}

fn trace_id(o: &Object) -> Option<&str> {
    o.get_str("traceId")
}

fn trace_ids_in<'a>(observations: impl Iterator<Item = &'a LangfuseObservation>) -> impl Iterator<Item = &'a str> {
    observations.filter_map(trace_id)
}

/// Activity → one group per conversation, its traces and its latest `updatedAt`.
fn group_by_conversation(activity: &[LangfuseObservation]) -> Result<Vec<ConversationGroup>, ReadError> {
    let conversation_of = conversations_of(activity);
    let mut groups: OrderedMap<String, (Vec<String>, Option<f64>, bool)> = OrderedMap::new();
    for observation in activity {
        let cid = conversation_of(observation);
        let group = groups.entry_or_insert_with(cid.clone(), || (Vec::new(), None, false));
        if let Some(trace_id) = trace_id(observation)
            && !group.0.iter().any(|t| t == trace_id)
        {
            group.0.push(trace_id.to_string());
        }
        if observation.get_str("sessionId") == Some(cid.as_str()) {
            group.2 = true;
        }
        let updated_at = truthy_str(observation, "updatedAt").map_or(f64::NAN, date::parse);
        if !updated_at.is_nan() {
            group.1 = Some(group.1.unwrap_or(updated_at).max(updated_at));
        }
    }
    let mut out = Vec::new();
    for (cid, (trace_ids, latest, is_session)) in groups.into_entries() {
        let last_modified = match latest {
            None => None,
            Some(ms) => {
                Some(date::to_iso_string(ms).ok_or_else(|| ReadError::Other("Invalid time value".to_string()))?)
            }
        };
        out.push(ConversationGroup {
            info: SessionInfo {
                conversation_id: cid,
                last_modified,
            },
            trace_ids,
            is_session,
        });
    }
    stable_sort_by(&mut out, |a, b| {
        locale_compare(&a.info.conversation_id, &b.info.conversation_id)
    });
    Ok(out)
}

/// Langfuse v2 returns `sessionId` only on the observation that carries the
/// session (usually the trace root), so the session is resolved per trace:
/// any non-empty `sessionId` of the trace, else its `traceId`.
fn conversations_of(observations: &[LangfuseObservation]) -> impl Fn(&LangfuseObservation) -> String + use<> {
    let mut session_by_trace: HashMap<String, String> = HashMap::new();
    for observation in observations {
        if let (Some(trace_id), Some(session_id)) = (trace_id(observation), truthy_str(observation, "sessionId")) {
            session_by_trace.insert(trace_id.to_string(), session_id.to_string());
        }
    }
    move |observation| {
        trace_id(observation)
            .and_then(|t| session_by_trace.get(t).filter(|s| !s.is_empty()).cloned())
            .or_else(|| truthy_str(observation, "sessionId").map(str::to_string))
            .or_else(|| truthy_str(observation, "traceId").map(str::to_string))
            .unwrap_or_else(|| crate::domain::models::js_display(observation.get("id")))
    }
}

fn start_time_ms(o: &Object) -> f64 {
    o.get_str("startTime").map_or(f64::NAN, date::parse)
}

fn bind_conversations(
    observations: &[LangfuseObservation],
    conversation_of: &dyn Fn(&LangfuseObservation) -> String,
    capture_id: &str,
    skipped: &mut Vec<SkippedRow>,
) -> OrderedMap<String, Label> {
    let mut sorted: Vec<&LangfuseObservation> = observations.iter().collect();
    stable_sort_by(&mut sorted, |a, b| {
        let d = start_time_ms(a) - start_time_ms(b);
        if d < 0.0 {
            std::cmp::Ordering::Less
        } else if d > 0.0 {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
    let mut labels_by_cid: OrderedMap<String, Vec<Label>> = OrderedMap::new();
    for observation in sorted {
        for marker in markers_of(observation) {
            if marker.capture_id != capture_id {
                continue;
            }
            let labels = labels_by_cid.entry_or_insert_with(conversation_of(observation), Vec::new);
            if !labels.contains(&marker.label) {
                labels.push(marker.label);
            }
        }
    }
    let mut label_by_cid = OrderedMap::new();
    for (cid, labels) in labels_by_cid.into_entries() {
        if labels.len() == 1 {
            label_by_cid.insert(cid, labels[0].clone());
        } else {
            skipped.push(SkippedRow::new(
                "observations",
                "same session has markers for multiple labels",
                format!("{cid}: {}", labels.join(", ")),
            ));
        }
    }
    label_by_cid
}

fn markers_of(observation: &Object) -> Vec<ExactMarker> {
    if !observation.get_str("type").is_some_and(|t| MARKER_TYPES.contains(&t)) {
        return vec![];
    }
    command_of(observation)
        .map(|c| exact_markers_in_command(&c))
        .unwrap_or_default()
}

/// Observations → events: a root observation's input is the `query`, a
/// `GENERATION` output an `agent_message`, a `TOOL`/`SPAN` input a
/// `tool_call` and its output a `tool_result` (both carry the observation id as
/// `tool_call_id`). Inputs are stamped at `startTime`, outputs at `endTime`
/// (else `startTime`).
fn drafts_from_observations(
    observations: &[LangfuseObservation],
    conversation_of: &dyn Fn(&LangfuseObservation) -> String,
) -> Result<Vec<EventDraft>, ReadError> {
    let agent_by_trace = agents_by_trace(observations);
    let invalid = || ReadError::Other("Invalid time value".to_string());
    let mut drafts = Vec::new();
    for observation in observations {
        let cid = conversation_of(observation);
        let started = date::to_iso_string(start_time_ms(observation)).ok_or_else(invalid)?;
        let end = match observation.get("endTime") {
            None | Some(Value::Null) => observation.get("startTime"),
            some => some,
        };
        let ended =
            date::to_iso_string(end.and_then(Value::as_str).map_or(f64::NAN, date::parse)).ok_or_else(invalid)?;

        let mut meta = Object::new();
        meta.insert("observation_id", observation.get("id").cloned().unwrap_or(Value::Null));
        meta.insert(
            "observation_type",
            observation.get("type").cloned().unwrap_or(Value::Null),
        );
        meta.insert("trace_id", observation.get("traceId").cloned().unwrap_or(Value::Null));
        if let Some(agent) = trace_id(observation).and_then(|t| agent_by_trace.get(t)) {
            meta.insert("session_source", agent.as_str());
        }
        let meta: Object = meta
            .iter()
            .filter(|(_, v)| !v.is_null())
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        let mut event = |ts: &str, role: Role, kind: EventKind, content: Option<String>, extra: &Object| {
            let Some(content) = content.filter(|c| !c.is_empty()) else {
                return;
            };
            let mut event_meta = meta.clone();
            for (k, v) in extra.iter() {
                event_meta.insert(k.as_str(), v.clone());
            }
            drafts.push(EventDraft {
                conversation_id: cid.clone(),
                ts: ts.to_string(),
                role,
                kind,
                content,
                meta: event_meta,
            });
        };
        let none = Object::new();
        let kind = observation.get_str("type");
        if is_root(observation) {
            event(
                &started,
                Role::User,
                EventKind::Query,
                text_of(observation.get("input")),
                &none,
            );
            continue;
        }
        if kind == Some("GENERATION") {
            event(
                &ended,
                Role::Assistant,
                EventKind::AgentMessage,
                text_of(observation.get("output")),
                &none,
            );
            continue;
        }
        if !kind.is_some_and(|k| MARKER_TYPES.contains(&k)) {
            continue;
        }
        let mut tool = Object::new();
        for (key, value) in [
            ("tool", observation.get("name")),
            ("tool_call_id", observation.get("id")),
        ] {
            if let Some(value) = value.filter(|v| !v.is_null()) {
                tool.insert(key, value.clone());
            }
        }
        let call = command_of(observation).or_else(|| text_of(observation.get("input")));
        event(&started, Role::Assistant, EventKind::ToolCall, call, &tool);
        event(
            &ended,
            Role::Tool,
            EventKind::ToolResult,
            text_of(observation.get("output")),
            &tool,
        );
    }
    Ok(drafts)
}

/// The agent each trace names in its metadata (`agent` on the trace's root observation).
fn agents_by_trace(observations: &[LangfuseObservation]) -> HashMap<String, String> {
    let mut agents = HashMap::new();
    for observation in observations {
        let agent = observation
            .get("metadata")
            .and_then(Value::as_object)
            .and_then(|m| m.get_str("agent"));
        if let (true, Some(trace_id), Some(agent)) = (is_root(observation), trace_id(observation), agent) {
            agents.insert(trace_id.to_string(), agent.to_string());
        }
    }
    agents
}

/// `isRootObservation ?? parentObservationId === null`.
fn is_root(observation: &Object) -> bool {
    match observation.get("isRootObservation") {
        None | Some(Value::Null) => matches!(observation.get("parentObservationId"), Some(Value::Null)),
        Some(Value::Bool(b)) => *b,
        Some(other) => {
            !matches!(other, Value::Number(n) if *n == 0.0 || n.is_nan())
                && !matches!(other, Value::String(s) if s.is_empty())
        }
    }
}

/// The shell command of a tool observation whose input is `{"command": "…"}`.
fn command_of(observation: &Object) -> Option<String> {
    let name = observation.get_str("name").unwrap_or("");
    if !MARKER_TOOLS.contains(&name) {
        return None;
    }
    match parse_json(observation.get("input")?) {
        Value::Object(input) => input.get_str("command").map(str::to_string),
        _ => None,
    }
}

/// Event text of a raw v2 payload: a JSON string is unwrapped, anything else kept raw.
fn text_of(value: Option<&Value>) -> Option<String> {
    match value {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => match parse_json(&Value::String(s.clone())) {
            Value::String(parsed) => Some(parsed),
            _ => Some(s.clone()),
        },
        Some(other) => Some(other.to_json()),
    }
}

fn parse_json(value: &Value) -> Value {
    match value {
        Value::String(s) => json::parse(s).unwrap_or_else(|_| value.clone()),
        other => other.clone(),
    }
}

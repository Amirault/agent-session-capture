//! Port of `langfuseSessionReader.test.ts`: sessions mode over Langfuse.

use std::cell::RefCell;
use std::rc::Rc;

use super::fixtures::TempDir;
use super::fixtures::langfuse_observations::{generation, terminal_call, trace_root};
use crate::adapters::langfuse_conversation_reader::LangfuseConversationReader;
use crate::adapters::langfuse_observation_lister::{LangfuseObservation, LangfuseObservationLister, ObservationQuery};
use crate::domain::models::{BundleSource, EventDraft, SessionHeader, SessionInfo, WrittenSession};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{ReadSessionsOptions, SessionReader};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value, date, json, path};
use crate::usecases::extract_sessions::{ExtractSessionsOptions, extract_sessions};

type RecordedQuery = (&'static str, ObservationQuery);
type Queries = Rc<RefCell<Vec<RecordedQuery>>>;

/// The fields `fields=core,basic,time` returns: no input, output nor metadata.
fn activity_of(observation: &LangfuseObservation) -> LangfuseObservation {
    observation
        .iter()
        .filter(|(k, _)| !matches!(k.as_str(), "input" | "output" | "metadata"))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// In-memory observation client filtering as the Langfuse v2 endpoint does
/// (`fromStartTime` on `startTime`, `sessionId` and `traceId` exact),
/// recording every query.
struct FakeLister {
    observations: Vec<LangfuseObservation>,
    queries: Queries,
}

impl LangfuseObservationLister for FakeLister {
    fn list_activity(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        self.queries.borrow_mut().push(("activity", query.clone()));
        Ok(self
            .observations
            .iter()
            .filter(|o| {
                query
                    .from_start_time
                    .as_deref()
                    .is_none_or(|from| o.get_str("startTime").is_some_and(|s| s >= from))
            })
            .filter(|o| {
                query
                    .session_id
                    .as_deref()
                    .is_none_or(|id| o.get_str("sessionId") == Some(id))
            })
            .filter(|o| {
                query
                    .trace_id
                    .as_deref()
                    .is_none_or(|id| o.get_str("traceId") == Some(id))
            })
            .map(activity_of)
            .collect())
    }

    fn list_observations(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        self.queries.borrow_mut().push(("observations", query.clone()));
        Ok(self
            .observations
            .iter()
            .filter(|o| {
                query
                    .trace_id
                    .as_deref()
                    .is_none_or(|id| o.get_str("traceId") == Some(id))
            })
            .cloned()
            .collect())
    }
}

fn create_reader(observations: Vec<LangfuseObservation>) -> (LangfuseConversationReader<FakeLister>, Queries) {
    let queries: Queries = Rc::default();
    let client = FakeLister {
        observations,
        queries: queries.clone(),
    };
    (LangfuseConversationReader::new(client), queries)
}

fn s(value: &str) -> Value {
    Value::from(value)
}

/// One trace as Langfuse v2 returns it: the root carries `sessionId` (`""` for a
/// session-less trace), every other observation `""`; `updatedAt` = `startTime`
/// unless given.
fn trace(
    trace_id: &str,
    session_id: &str,
    start_time: &str,
    prompt: &str,
    answer: &str,
    updated_at: Option<&str>,
) -> Vec<LangfuseObservation> {
    let later = date::to_iso_string(date::parse(start_time) + 1000.0).expect("valid date");
    vec![
        trace_root(
            prompt,
            &[
                ("id", s(&format!("{trace_id}-root"))),
                ("traceId", s(trace_id)),
                ("sessionId", s(session_id)),
                ("startTime", s(start_time)),
                ("updatedAt", s(start_time)),
            ],
        ),
        generation(
            answer,
            &[
                ("id", s(&format!("{trace_id}-answer"))),
                ("traceId", s(trace_id)),
                ("sessionId", s("")),
                ("parentObservationId", s(&format!("{trace_id}-root"))),
                ("startTime", s(&later)),
                ("updatedAt", s(updated_at.unwrap_or(&later))),
            ],
        ),
    ]
}

/// Example 4: `hermes-0412` last active 2026-09-28, `hermes-0419` (two traces) 2026-10-17.
fn example_four_sessions() -> Vec<LangfuseObservation> {
    let mut all = trace(
        "tr-0412",
        "hermes-0412",
        "2026-09-28T10:00:00.000Z",
        "old",
        "old answer",
        None,
    );
    all.extend(trace(
        "tr-0419a",
        "hermes-0419",
        "2026-10-16T09:00:00.000Z",
        "first run",
        "first answer",
        None,
    ));
    all.extend(trace(
        "tr-0419b",
        "hermes-0419",
        "2026-10-17T14:00:00.000Z",
        "second run",
        "second answer",
        Some("2026-10-17T14:02:11.000Z"),
    ));
    all
}

fn stored_header(conversation_id: &str, last_modified: Option<&str>) -> SessionHeader {
    SessionHeader::fresh(
        conversation_id,
        BundleSource::Langfuse,
        last_modified,
        "2026-10-18T00:00:00.000Z",
    )
}

fn stored(headers: Vec<SessionHeader>) -> OrderedMap<String, SessionHeader> {
    headers.into_iter().map(|h| (h.conversation_id.clone(), h)).collect()
}

fn contents_by_conversation(drafts: &[EventDraft]) -> Vec<(&str, &str)> {
    drafts
        .iter()
        .map(|d| (d.conversation_id.as_str(), d.content.as_str()))
        .collect()
}

fn info(conversation_id: &str, last_modified: Option<&str>) -> SessionInfo {
    SessionInfo {
        conversation_id: conversation_id.to_string(),
        last_modified: last_modified.map(str::to_string),
    }
}

fn since(from: &str) -> ObservationQuery {
    ObservationQuery {
        from_start_time: Some(from.to_string()),
        ..Default::default()
    }
}

fn by_session(session_id: &str) -> ObservationQuery {
    ObservationQuery {
        session_id: Some(session_id.to_string()),
        ..Default::default()
    }
}

fn by_trace(trace_id: &str) -> ObservationQuery {
    ObservationQuery {
        trace_id: Some(trace_id.to_string()),
        ..Default::default()
    }
}

fn options(since: Option<&str>, conversation: Option<&str>) -> ReadSessionsOptions {
    ReadSessionsOptions {
        since: since.map(str::to_string),
        conversation: conversation.map(str::to_string),
        stored_by_cid: OrderedMap::new(),
    }
}

// LangfuseConversationReader.readSessions

#[test]
fn since_enumerates_only_the_recent_session_with_its_traces_grouped_under_the_root_session_id() {
    // Given
    let (reader, queries) = create_reader(example_four_sessions());

    // When
    let result = reader
        .read_sessions(&options(Some("2026-10-01"), None))
        .expect("read sessions");

    // Then
    assert_eq!(result.source, BundleSource::Langfuse);
    assert_eq!(
        result.sessions,
        vec![info("hermes-0419", Some("2026-10-17T14:02:11.000Z"))]
    );
    assert_eq!(
        contents_by_conversation(&result.reads.get(&"hermes-0419".to_string()).expect("read").drafts),
        vec![
            ("hermes-0419", "first run"),
            ("hermes-0419", "first answer"),
            ("hermes-0419", "second run"),
            ("hermes-0419", "second answer"),
        ]
    );
    assert_eq!(queries.borrow()[0], ("activity", since("2026-10-01T00:00:00.000Z")));
}

#[test]
fn since_exports_every_trace_of_a_session_not_only_the_recent_one() {
    // Given
    let mut observations = trace(
        "tr-0419-old",
        "hermes-0419",
        "2026-09-20T09:00:00.000Z",
        "first run",
        "first answer",
        None,
    );
    observations.extend(trace(
        "tr-0419-new",
        "hermes-0419",
        "2026-10-17T14:00:00.000Z",
        "second run",
        "second answer",
        None,
    ));
    let (reader, queries) = create_reader(observations);

    // When
    let result = reader
        .read_sessions(&options(Some("2026-10-01"), None))
        .expect("read sessions");

    // Then
    assert_eq!(
        result.sessions,
        vec![info("hermes-0419", Some("2026-10-17T14:00:01.000Z"))]
    );
    assert_eq!(
        contents_by_conversation(&result.reads.get(&"hermes-0419".to_string()).expect("read").drafts),
        vec![
            ("hermes-0419", "first run"),
            ("hermes-0419", "first answer"),
            ("hermes-0419", "second run"),
            ("hermes-0419", "second answer"),
        ]
    );
    assert_eq!(
        *queries.borrow(),
        vec![
            ("activity", since("2026-10-01T00:00:00.000Z")),
            ("activity", by_session("hermes-0419")),
            ("observations", by_trace("tr-0419-old")),
            ("observations", by_trace("tr-0419-new")),
        ]
    );
}

#[test]
fn since_joins_a_cut_trace_to_its_session_once_without_a_file_of_its_own() {
    // Given
    let mut observations = trace(
        "tr-0419-cut",
        "hermes-0419",
        "2026-09-30T23:59:59.500Z",
        "first run",
        "first answer",
        None,
    );
    observations.extend(trace(
        "tr-0419-new",
        "hermes-0419",
        "2026-10-17T14:00:00.000Z",
        "second run",
        "second answer",
        None,
    ));
    let (reader, queries) = create_reader(observations);

    // When
    let result = reader
        .read_sessions(&options(Some("2026-10-01"), None))
        .expect("read sessions");

    // Then
    assert_eq!(
        result.sessions,
        vec![info("hermes-0419", Some("2026-10-17T14:00:01.000Z"))]
    );
    assert_eq!(
        contents_by_conversation(&result.reads.get(&"hermes-0419".to_string()).expect("read").drafts),
        vec![
            ("hermes-0419", "first run"),
            ("hermes-0419", "first answer"),
            ("hermes-0419", "second run"),
            ("hermes-0419", "second answer"),
        ]
    );
    assert_eq!(
        *queries.borrow(),
        vec![
            ("activity", since("2026-10-01T00:00:00.000Z")),
            ("activity", by_trace("tr-0419-cut")),
            ("activity", by_session("hermes-0419")),
            ("observations", by_trace("tr-0419-cut")),
            ("observations", by_trace("tr-0419-new")),
        ]
    );
}

#[test]
fn conversation_gathers_exactly_that_one_even_with_an_unchanged_stored_stamp() {
    // Given
    let (reader, queries) = create_reader(example_four_sessions());
    let mut read_options = options(None, Some("hermes-0412"));
    read_options.stored_by_cid = stored(vec![stored_header("hermes-0412", Some("2026-09-28T10:00:01.000Z"))]);

    // When
    let result = reader.read_sessions(&read_options).expect("read sessions");

    // Then
    assert_eq!(
        result.sessions,
        vec![info("hermes-0412", Some("2026-09-28T10:00:01.000Z"))]
    );
    assert_eq!(result.reads.keys().collect::<Vec<_>>(), vec!["hermes-0412"]);
    assert_eq!(
        contents_by_conversation(&result.reads.get(&"hermes-0412".to_string()).expect("read").drafts),
        vec![("hermes-0412", "old"), ("hermes-0412", "old answer")]
    );
    assert_eq!(
        *queries.borrow(),
        vec![
            ("activity", ObservationQuery::default()),
            ("observations", by_trace("tr-0412"))
        ]
    );
}

#[test]
fn unchanged_stored_stamp_skips_the_fetch_while_a_changed_session_is_fetched_trace_by_trace() {
    // Given
    let mut observations = example_four_sessions();
    observations.extend(trace(
        "tr-0420",
        "hermes-0420",
        "2026-10-18T08:00:00.000Z",
        "new",
        "new answer",
        None,
    ));
    let (reader, queries) = create_reader(observations);
    let mut read_options = options(Some("2026-10-01"), None);
    read_options.stored_by_cid = stored(vec![
        stored_header("hermes-0419", Some("2026-10-17T14:02:11.000Z")),
        stored_header("hermes-0420", Some("2026-10-18T07:00:00.000Z")),
    ]);

    // When
    let result = reader.read_sessions(&read_options).expect("read sessions");

    // Then
    assert_eq!(
        result
            .sessions
            .iter()
            .map(|s| s.conversation_id.as_str())
            .collect::<Vec<_>>(),
        vec!["hermes-0419", "hermes-0420"]
    );
    assert_eq!(result.reads.keys().collect::<Vec<_>>(), vec!["hermes-0420"]);
    assert_eq!(
        *queries.borrow(),
        vec![
            ("activity", since("2026-10-01T00:00:00.000Z")),
            ("activity", by_session("hermes-0420")),
            ("observations", by_trace("tr-0420")),
        ]
    );
}

#[test]
fn trace_without_session_is_its_own_session_named_by_its_trace_id() {
    // Given
    let (reader, _) = create_reader(trace(
        "tr-solo",
        "",
        "2026-10-18T08:00:00.000Z",
        "solo",
        "solo answer",
        None,
    ));

    // When
    let result = reader.read_sessions(&options(None, None)).expect("read sessions");

    // Then
    assert_eq!(result.sessions, vec![info("tr-solo", Some("2026-10-18T08:00:01.000Z"))]);
    assert_eq!(
        contents_by_conversation(&result.reads.get(&"tr-solo".to_string()).expect("read").drafts),
        vec![("tr-solo", "solo"), ("tr-solo", "solo answer")]
    );
}

#[test]
fn since_with_a_session_less_trace_wholly_in_the_window_makes_no_lookup() {
    // Given
    let (reader, queries) = create_reader(trace(
        "tr-solo",
        "",
        "2026-10-18T08:00:00.000Z",
        "solo",
        "solo answer",
        None,
    ));

    // When
    let result = reader
        .read_sessions(&options(Some("2026-10-01"), None))
        .expect("read sessions");

    // Then
    assert_eq!(
        result
            .sessions
            .iter()
            .map(|s| s.conversation_id.as_str())
            .collect::<Vec<_>>(),
        vec!["tr-solo"]
    );
    assert_eq!(
        *queries.borrow(),
        vec![
            ("activity", since("2026-10-01T00:00:00.000Z")),
            ("observations", by_trace("tr-solo"))
        ]
    );
}

#[test]
fn child_tool_call_maps_as_in_marker_mode_with_the_agent_of_the_trace_metadata() {
    // Given
    let mut metadata = Object::new();
    metadata.insert("agent", "hermes");
    let (reader, _) = create_reader(vec![
        trace_root(
            "implement",
            &[
                ("id", s("obs-1")),
                ("traceId", s("tr-0419")),
                ("sessionId", s("hermes-0419")),
                ("metadata", Value::from(metadata)),
            ],
        ),
        terminal_call(
            "cat spec.md",
            &[
                ("id", s("obs-3")),
                ("traceId", s("tr-0419")),
                ("sessionId", s("")),
                ("output", Value::from(json::quote("# Spec: …"))),
                ("startTime", s("2026-10-19T09:00:05.000Z")),
            ],
        ),
    ]);

    // When
    let result = reader.read_sessions(&options(None, None)).expect("read sessions");

    // Then
    let drafts = &result.reads.get(&"hermes-0419".to_string()).expect("read").drafts;
    let mapped: Vec<(&str, Option<&str>, Option<&str>)> = drafts
        .iter()
        .map(|d| {
            (
                d.kind.as_str(),
                d.meta.get_str("tool_call_id"),
                d.meta.get_str("session_source"),
            )
        })
        .collect();
    assert_eq!(
        mapped,
        vec![
            ("query", None, Some("hermes")),
            ("tool_call", Some("obs-3"), Some("hermes")),
            ("tool_result", Some("obs-3"), Some("hermes")),
        ]
    );
}

#[test]
fn observations_without_updated_at_leave_the_session_unstamped_and_always_gathered() {
    // Given
    let root = trace_root(
        "no stamp",
        &[("id", s("obs-1")), ("traceId", s("tr-x")), ("sessionId", s("hermes-x"))],
    );
    let (reader, _) = create_reader(vec![root]);
    let mut read_options = options(None, None);
    read_options.stored_by_cid = stored(vec![stored_header("hermes-x", None)]);

    // When
    let result = reader.read_sessions(&read_options).expect("read sessions");

    // Then
    assert_eq!(result.sessions, vec![info("hermes-x", None)]);
    assert_eq!(result.reads.keys().collect::<Vec<_>>(), vec!["hermes-x"]);
}

// extractSessions over Langfuse

#[test]
fn extract_sessions_rerun_reports_the_exported_session_unchanged_without_fetching_observations() {
    // Given
    let out_dir = TempDir::new("lf-sessions-");
    let (reader, queries) = create_reader(example_four_sessions());
    let run = || {
        extract_sessions(
            &reader,
            ExtractSessionsOptions {
                out_dir: out_dir.path(),
                source: BundleSource::Langfuse,
                since: Some("2026-10-01".to_string()),
                conversation: None,
                no_merge: false,
            },
        )
        .expect("extract sessions")
    };
    let first = run();

    // When
    let second = run();

    // Then
    let file = path::join(&[&out_dir.path(), "sessions", "hermes-0419.jsonl"]);
    assert_eq!(
        first.written,
        vec![WrittenSession {
            conversation_id: "hermes-0419".to_string(),
            path: file.clone(),
            events: 4
        }]
    );
    let text = std::fs::read_to_string(&file).expect("session file");
    let header = json::parse(text.split('\n').next().expect("header line")).expect("header JSON");
    let header = header.as_object().expect("header object");
    assert_eq!(header.get_str("conversation_id"), Some("hermes-0419"));
    assert_eq!(header.get_str("source"), Some("langfuse"));
    assert_eq!(header.get_str("last_modified"), Some("2026-10-17T14:02:11.000Z"));
    assert!(second.written.is_empty());
    assert_eq!(second.unchanged, vec!["hermes-0419"]);
    let fetched: Vec<RecordedQuery> = queries
        .borrow()
        .iter()
        .filter(|(kind, _)| *kind == "observations")
        .cloned()
        .collect();
    assert_eq!(
        fetched,
        vec![
            ("observations", by_trace("tr-0419a")),
            ("observations", by_trace("tr-0419b"))
        ]
    );
}

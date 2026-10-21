//! Port of `langfuseConversationReader.test.ts`: marker mode over Langfuse.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use super::fixtures::langfuse_observations::{
    any_observation, generation, marker_command, terminal_call, trace_root, with,
};
use crate::adapters::langfuse_conversation_reader::LangfuseConversationReader;
use crate::adapters::langfuse_observation_lister::{LangfuseObservation, LangfuseObservationLister, ObservationQuery};
use crate::domain::models::{EventDraft, SkippedRow};
use crate::domain::ports::{CaptureRead, ConversationReader};
use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value, date, json};
use crate::usecases::extract_bundle::{ExtractOptions, ExtractResult, extract_bundle};

const CAPTURE_ID: &str = "checkout-bug";
const NOW: &str = "2026-10-19T12:00:00.000Z";

/// In-memory observation client: returns the fixture, records every query.
struct FakeLister {
    observations: Vec<LangfuseObservation>,
    queries: Rc<RefCell<Vec<ObservationQuery>>>,
}

impl LangfuseObservationLister for FakeLister {
    fn list_observations(&self, query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        self.queries.borrow_mut().push(query.clone());
        Ok(self.observations.clone())
    }

    fn list_activity(&self, _query: &ObservationQuery) -> Result<Vec<LangfuseObservation>, ReadError> {
        panic!("marker mode never lists activity")
    }
}

fn create_reader(
    observations: Vec<LangfuseObservation>,
) -> (
    LangfuseConversationReader<FakeLister>,
    Rc<RefCell<Vec<ObservationQuery>>>,
) {
    let queries = Rc::new(RefCell::new(Vec::new()));
    let client = FakeLister {
        observations,
        queries: queries.clone(),
    };
    (
        LangfuseConversationReader::with_now(client, || date::parse(NOW)),
        queries,
    )
}

fn s(value: &str) -> Value {
    Value::from(value)
}

fn json_string(text: &str) -> Value {
    Value::from(json::quote(text))
}

fn agent_metadata(agent: &str) -> Value {
    let mut metadata = Object::new();
    metadata.insert("agent", agent);
    Value::from(metadata)
}

/// Example 8: one Hermes session, a prompt, an answer and one terminal call.
fn example_eight_session() -> Vec<LangfuseObservation> {
    vec![
        trace_root(
            "implement increment 2",
            &[
                ("id", s("obs-1")),
                ("traceId", s("tr-0419")),
                ("startTime", s("2026-10-19T09:00:00.000Z")),
                ("metadata", agent_metadata("hermes")),
            ],
        ),
        terminal_call(
            "cat spec.md",
            &[
                ("id", s("obs-3")),
                ("traceId", s("tr-0419")),
                ("output", json_string("# Spec: …")),
                ("startTime", s("2026-10-19T09:00:05.000Z")),
            ],
        ),
        generation(
            "I will read the spec first",
            &[
                ("id", s("obs-2")),
                ("traceId", s("tr-0419")),
                ("startTime", s("2026-10-19T09:00:04.000Z")),
            ],
        ),
        terminal_call(
            &marker_command(CAPTURE_ID, "implement"),
            &[
                ("id", s("obs-91")),
                ("traceId", s("tr-0419")),
                ("startTime", s("2026-10-19T09:00:01.000Z")),
            ],
        ),
    ]
}

/// A trace as Langfuse v2 returns it for an OTLP export setting `session.id`
/// on the root span only: the root carries `sessionId`, every other
/// observation of the trace an empty string (`""` = no session).
fn session_on_root_only(session_id: &str, trace_id: &str, label: &str) -> Vec<LangfuseObservation> {
    vec![
        trace_root(
            "run the workflow",
            &[
                ("id", s(&format!("{trace_id}-root"))),
                ("traceId", s(trace_id)),
                ("sessionId", s(session_id)),
            ],
        ),
        terminal_call(
            &marker_command(CAPTURE_ID, label),
            &[
                ("id", s(&format!("{trace_id}-marker"))),
                ("traceId", s(trace_id)),
                ("sessionId", s("")),
                ("parentObservationId", s(&format!("{trace_id}-root"))),
                ("startTime", s("2026-10-19T09:00:01.000Z")),
            ],
        ),
        generation(
            "done",
            &[
                ("id", s(&format!("{trace_id}-answer"))),
                ("traceId", s(trace_id)),
                ("sessionId", s("")),
                ("parentObservationId", s(&format!("{trace_id}-root"))),
                ("startTime", s("2026-10-19T09:00:02.000Z")),
            ],
        ),
    ]
}

fn extract(reader: &LangfuseConversationReader<FakeLister>) -> ExtractResult {
    extract_bundle(reader, CAPTURE_ID, ExtractOptions::default()).expect("extract")
}

fn read(reader: &LangfuseConversationReader<FakeLister>) -> CaptureRead {
    reader.read_capture(CAPTURE_ID).expect("read capture")
}

fn labels(read: &CaptureRead) -> Vec<(String, String)> {
    read.label_by_cid.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

fn pairs(expected: &[(&str, &str)]) -> Vec<(String, String)> {
    expected.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

fn meta_str<'a>(draft: &'a EventDraft, key: &str) -> Option<&'a str> {
    draft.meta.get_str(key)
}

// LangfuseConversationReader marker mode (binding)

#[test]
fn exact_marker_in_terminal_tool_binds_session_and_all_observations_become_langfuse_events() {
    // Given
    let answers = (0..36).map(|i| {
        generation(
            &format!("answer {i}"),
            &[
                ("id", s(&format!("obs-gen-{i}"))),
                ("startTime", s(&format!("2026-10-19T09:01:{i:02}.000Z"))),
            ],
        )
    });
    let mut observations = vec![
        trace_root("implement the spec", &[("id", s("obs-root"))]),
        terminal_call(
            &marker_command(CAPTURE_ID, "implement"),
            &[("id", s("obs-91")), ("startTime", s("2026-10-19T09:00:30.000Z"))],
        ),
    ];
    observations.extend(answers);
    observations.push(generation(
        "unrelated session",
        &[
            ("id", s("obs-other")),
            ("traceId", s("tr-other")),
            ("sessionId", s("hermes-other")),
        ],
    ));
    let (reader, _) = create_reader(observations);

    // When
    let ExtractResult { bundle, summary } = extract(&reader);

    // Then
    assert_eq!(summary.events, 38);
    assert_eq!(bundle.header.get_str("source"), Some("langfuse"));
    assert_eq!(strings(bundle.header.get("labels")), vec!["implement"]);
    assert_eq!(strings(bundle.header.get("conversation_ids")), vec!["hermes-0419"]);
    let conversations: HashSet<&str> = bundle
        .events
        .iter()
        .filter_map(|e| e.get_str("conversation_id"))
        .collect();
    assert_eq!(conversations, HashSet::from(["hermes-0419"]));
}

fn assert_marker_binds_for(name: &str, kind: &str) {
    // Given
    let (reader, _) = create_reader(vec![terminal_call(
        &marker_command(CAPTURE_ID, "review"),
        &[("name", s(name)), ("type", s(kind))],
    )]);

    // When
    let read = read(&reader);

    // Then
    assert_eq!(labels(&read), pairs(&[("hermes-0419", "review")]));
}

#[test]
fn exact_marker_in_bash_tool_binds() {
    assert_marker_binds_for("Bash", "TOOL");
}

#[test]
fn exact_marker_in_run_shell_command_tool_binds() {
    assert_marker_binds_for("run_shell_command", "TOOL");
}

#[test]
fn exact_marker_in_terminal_span_binds() {
    assert_marker_binds_for("terminal", "SPAN");
}

#[test]
fn marker_outside_a_shell_command_input_binds_nothing() {
    // Given
    let marker = marker_command(CAPTURE_ID, "specify");
    let (reader, _) = create_reader(vec![
        with(
            any_observation(),
            &[
                ("id", s("gen")),
                ("type", s("GENERATION")),
                ("input", json_string(&format!("run {marker}"))),
            ],
        ),
        terminal_call("cat skill.md", &[("id", s("out")), ("output", json_string(&marker))]),
        terminal_call(&format!("echo '{marker}'"), &[("id", s("echo"))]),
        terminal_call(&marker, &[("id", s("read")), ("name", s("read_file"))]),
        terminal_call(&marker, &[("id", s("llm-tool")), ("type", s("GENERATION"))]),
    ]);

    // When
    let ExtractResult { bundle, summary } = extract(&reader);

    // Then
    assert_eq!(summary.conversations, 0);
    assert!(bundle.events.is_empty());
}

#[test]
fn exact_marker_without_label_binds_to_default_label() {
    // Given
    let (reader, _) = create_reader(vec![terminal_call(
        &format!(": CAPTURE_MARKER v=1 id={CAPTURE_ID}"),
        &[],
    )]);

    // When
    let read = read(&reader);

    // Then
    assert_eq!(labels(&read), pairs(&[("hermes-0419", "default")]));
}

#[test]
fn marker_of_another_capture_id_binds_nothing() {
    // Given
    let (reader, _) = create_reader(vec![terminal_call(
        &marker_command("another-capture", "implement"),
        &[],
    )]);

    // When
    let read = read(&reader);

    // Then
    assert_eq!(read.label_by_cid.len(), 0);
}

#[test]
fn bound_marker_without_session_id_uses_trace_id_as_conversation() {
    // Given
    let (reader, _) = create_reader(vec![terminal_call(
        &marker_command(CAPTURE_ID, "specify"),
        &[
            ("id", s("obs-12")),
            ("traceId", s("tr-5f0c")),
            ("sessionId", Value::Null),
        ],
    )]);

    // When
    let read = read(&reader);

    // Then
    assert_eq!(labels(&read), pairs(&[("tr-5f0c", "specify")]));
    assert_eq!(
        read.drafts
            .iter()
            .map(|d| d.conversation_id.as_str())
            .collect::<Vec<_>>(),
        vec!["tr-5f0c"]
    );
}

#[test]
fn session_id_only_on_root_binds_the_session_and_every_observation_of_the_trace_joins_it() {
    // Given
    let (reader, _) = create_reader(session_on_root_only("capture-probe", "tr-a", "implement"));

    // When
    let read = read(&reader);

    // Then
    assert_eq!(labels(&read), pairs(&[("capture-probe", "implement")]));
    let conversations: HashSet<&str> = read.drafts.iter().map(|d| d.conversation_id.as_str()).collect();
    assert_eq!(conversations, HashSet::from(["capture-probe"]));
}

#[test]
fn session_trace_and_session_less_trace_stay_two_conversations() {
    // Given
    let mut observations = session_on_root_only("capture-probe", "tr-a", "implement");
    observations.extend(session_on_root_only("", "tr-b", "review"));
    let (reader, _) = create_reader(observations);

    // When
    let read = read(&reader);

    // Then
    assert_eq!(
        labels(&read),
        pairs(&[("capture-probe", "implement"), ("tr-b", "review")])
    );
    assert_eq!(read.skipped, Vec::<SkippedRow>::new());
}

#[test]
fn same_label_marker_in_two_sessions_keeps_both_in_marker_order() {
    // Given
    let (reader, _) = create_reader(vec![
        terminal_call(
            &marker_command(CAPTURE_ID, "implement"),
            &[
                ("id", s("rerun")),
                ("traceId", s("tr-0422")),
                ("sessionId", s("hermes-0422")),
                ("startTime", s("2026-10-19T15:00:00.000Z")),
            ],
        ),
        terminal_call(
            &marker_command(CAPTURE_ID, "implement"),
            &[
                ("id", s("first")),
                ("sessionId", s("hermes-0419")),
                ("startTime", s("2026-10-19T09:00:00.000Z")),
            ],
        ),
    ]);

    // When
    let ExtractResult { bundle, summary } = extract(&reader);

    // Then
    let per_label = bundle
        .header
        .get("conversations_per_label")
        .and_then(Value::as_object)
        .expect("tally");
    assert_eq!(per_label.get("implement").and_then(Value::as_f64), Some(2.0));
    assert_eq!(
        strings(bundle.header.get("conversation_ids")),
        vec!["hermes-0419", "hermes-0422"]
    );
    assert!(summary.collisions.is_empty());
}

#[test]
fn one_session_with_markers_of_two_labels_is_skipped() {
    // Given
    let (reader, _) = create_reader(vec![
        terminal_call(&marker_command(CAPTURE_ID, "implement"), &[("id", s("a"))]),
        terminal_call(&marker_command(CAPTURE_ID, "review"), &[("id", s("b"))]),
    ]);

    // When
    let read = read(&reader);

    // Then
    assert_eq!(read.label_by_cid.len(), 0);
    assert_eq!(
        read.skipped,
        vec![SkippedRow::new(
            "observations",
            "same session has markers for multiple labels",
            "hermes-0419: implement, review"
        )]
    );
}

#[test]
fn reading_a_capture_id_asks_for_the_observations_of_the_last_30_days() {
    // Given
    let (reader, queries) = create_reader(vec![]);

    // When
    read(&reader);

    // Then
    assert_eq!(
        *queries.borrow(),
        vec![ObservationQuery {
            from_start_time: Some("2026-09-19T12:00:00.000Z".to_string()),
            ..Default::default()
        }]
    );
}

// LangfuseConversationReader marker mode (mapping)

#[test]
fn one_session_maps_to_query_agent_message_tool_call_and_tool_result_ordered_by_ts() {
    // Given
    let (reader, _) = create_reader(example_eight_session());

    // When
    let ExtractResult { bundle, .. } = extract(&reader);

    // Then
    let events: Vec<(f64, &str, &str, &str, &str)> = bundle
        .events
        .iter()
        .map(|e| {
            (
                e.get("seq").and_then(Value::as_f64).expect("seq"),
                e.get_str("ts").expect("ts"),
                e.get_str("role").expect("role"),
                e.get_str("kind").expect("kind"),
                e.get_str("content").expect("content"),
            )
        })
        .collect();
    let marker = marker_command(CAPTURE_ID, "implement");
    assert_eq!(
        events,
        vec![
            (
                1.0,
                "2026-10-19T09:00:00.000Z",
                "user",
                "query",
                "implement increment 2"
            ),
            (
                2.0,
                "2026-10-19T09:00:01.000Z",
                "assistant",
                "tool_call",
                marker.as_str()
            ),
            (
                3.0,
                "2026-10-19T09:00:04.000Z",
                "assistant",
                "agent_message",
                "I will read the spec first"
            ),
            (4.0, "2026-10-19T09:00:05.000Z", "assistant", "tool_call", "cat spec.md"),
            (5.0, "2026-10-19T09:00:05.000Z", "tool", "tool_result", "# Spec: …"),
        ]
    );
}

#[test]
fn tool_call_is_stamped_at_start_time_and_its_result_at_end_time() {
    // Given
    let (reader, _) = create_reader(vec![
        trace_root(
            "implement increment 2",
            &[("id", s("obs-root")), ("traceId", s("tr-0419"))],
        ),
        terminal_call(
            &marker_command(CAPTURE_ID, "implement"),
            &[
                ("id", s("obs-test")),
                ("traceId", s("tr-0419")),
                ("output", json_string("Tests 12 passed")),
                ("startTime", s("2026-10-19T09:00:05.000Z")),
                ("endTime", s("2026-10-19T09:00:36.000Z")),
            ],
        ),
    ]);

    // When
    let read = read(&reader);

    // Then
    let tool: Vec<(&str, &str)> = read
        .drafts
        .iter()
        .filter(|d| meta_str(d, "observation_id") == Some("obs-test"))
        .map(|d| (d.kind.as_str(), d.ts.as_str()))
        .collect();
    assert_eq!(
        tool,
        vec![
            ("tool_call", "2026-10-19T09:00:05.000Z"),
            ("tool_result", "2026-10-19T09:00:36.000Z")
        ]
    );
}

#[test]
fn generation_answer_is_stamped_at_its_end_time() {
    // Given
    let mut observations = example_eight_session();
    observations.push(generation(
        "# Workflow Report",
        &[
            ("id", s("obs-report")),
            ("traceId", s("tr-0419")),
            ("startTime", s("2026-10-19T09:01:00.000Z")),
            ("endTime", s("2026-10-19T09:02:09.000Z")),
        ],
    ));
    let (reader, _) = create_reader(observations);

    // When
    let read = read(&reader);

    // Then
    let answer = read
        .drafts
        .iter()
        .find(|d| meta_str(d, "observation_id") == Some("obs-report"));
    assert_eq!(answer.map(|d| d.ts.as_str()), Some("2026-10-19T09:02:09.000Z"));
}

#[test]
fn tool_result_with_null_end_time_falls_back_to_start_time() {
    // Given
    let (reader, _) = create_reader(vec![terminal_call(
        &marker_command(CAPTURE_ID, "implement"),
        &[
            ("id", s("obs-open")),
            ("output", json_string("partial")),
            ("startTime", s("2026-10-19T09:00:05.000Z")),
            ("endTime", Value::Null),
        ],
    )]);

    // When
    let read = read(&reader);

    // Then
    let result = read.drafts.iter().find(|d| d.kind.as_str() == "tool_result");
    assert_eq!(result.map(|d| d.ts.as_str()), Some("2026-10-19T09:00:05.000Z"));
}

#[test]
fn tool_call_and_result_share_the_observation_id_as_tool_call_id() {
    // Given
    let (reader, _) = create_reader(example_eight_session());

    // When
    let read = read(&reader);

    // Then
    let obs3: Vec<(&str, Option<&str>, Option<&str>)> = read
        .drafts
        .iter()
        .filter(|d| meta_str(d, "observation_id") == Some("obs-3"))
        .map(|d| (d.kind.as_str(), meta_str(d, "tool_call_id"), meta_str(d, "tool")))
        .collect();
    assert_eq!(
        obs3,
        vec![
            ("tool_call", Some("obs-3"), Some("terminal")),
            ("tool_result", Some("obs-3"), Some("terminal"))
        ]
    );
}

#[test]
fn trace_metadata_agent_is_recorded_as_session_source_on_every_event() {
    // Given
    let (reader, _) = create_reader(example_eight_session());

    // When
    let read = read(&reader);

    // Then
    let sources: Vec<Option<&str>> = read.drafts.iter().map(|d| meta_str(d, "session_source")).collect();
    assert_eq!(sources, vec![Some("hermes"); 5]);
}

#[test]
fn trace_whose_root_names_no_agent_has_no_session_source() {
    // Given
    let (reader, _) = create_reader(vec![terminal_call(
        &marker_command(CAPTURE_ID, "implement"),
        &[("metadata", agent_metadata("not-the-trace"))],
    )]);

    // When
    let read = read(&reader);

    // Then
    let has_source: Vec<bool> = read
        .drafts
        .iter()
        .map(|d| d.meta.contains_key("session_source"))
        .collect();
    assert_eq!(has_source, vec![false]);
}

#[test]
fn observations_without_prompt_answer_or_tool_payload_produce_no_event() {
    // Given
    let (reader, _) = create_reader(vec![
        terminal_call(&marker_command(CAPTURE_ID, "implement"), &[("id", s("marker"))]),
        with(
            any_observation(),
            &[
                ("id", s("prompt")),
                ("type", s("GENERATION")),
                ("input", json_string("system prompt")),
            ],
        ),
        with(
            any_observation(),
            &[
                ("id", s("event")),
                ("type", s("EVENT")),
                ("input", json_string("log line")),
            ],
        ),
        with(
            any_observation(),
            &[("id", s("empty")), ("type", s("TOOL")), ("name", s("read_file"))],
        ),
    ]);

    // When
    let read = read(&reader);

    // Then
    let ids: Vec<Option<&str>> = read.drafts.iter().map(|d| meta_str(d, "observation_id")).collect();
    assert_eq!(ids, vec![Some("marker")]);
}

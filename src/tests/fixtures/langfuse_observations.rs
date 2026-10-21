//! Synthetic Langfuse v2 observations (`GET /api/public/v2/observations`).
//! Input and output are raw strings, as the v2 endpoint returns them;
//! `sessionId` is set only on the observation carrying the session.

#![allow(dead_code)]

use crate::js::{Object, Value, json};

/// The base observation every helper starts from.
pub fn any_observation() -> Object {
    let mut o = Object::new();
    o.insert("id", "obs-any");
    o.insert("traceId", "tr-any");
    o.insert("sessionId", "hermes-0419");
    o.insert("type", "SPAN");
    o.insert("name", "step");
    o.insert("startTime", "2026-10-19T09:00:00.000Z");
    o.insert("parentObservationId", "obs-root");
    o.insert("isRootObservation", false);
    o.insert("input", Value::Null);
    o.insert("output", Value::Null);
    o.insert("metadata", Object::new());
    o
}

pub fn marker_command(capture_id: &str, label: &str) -> String {
    format!(": CAPTURE_MARKER v=1 id={capture_id} label={label}")
}

/// Apply `overrides` (`{...base, ...overrides}`).
pub fn with(mut base: Object, overrides: &[(&str, Value)]) -> Object {
    for (k, v) in overrides {
        base.insert(*k, v.clone());
    }
    base
}

/// A `TOOL` observation named `terminal` whose input runs `command`.
pub fn terminal_call(command: &str, overrides: &[(&str, Value)]) -> Object {
    let mut input = Object::new();
    input.insert("command", command);
    let base = with(
        any_observation(),
        &[("type", "TOOL".into()), ("name", "terminal".into()), ("input", Value::from(input.to_json()))],
    );
    with(base, overrides)
}

/// A `GENERATION` observation whose output is the assistant text.
pub fn generation(text: &str, overrides: &[(&str, Value)]) -> Object {
    let base = with(
        any_observation(),
        &[("type", "GENERATION".into()), ("name", "llm".into()), ("output", Value::from(json::quote(text)))],
    );
    with(base, overrides)
}

/// The root observation of a trace, carrying the user's prompt as input.
pub fn trace_root(prompt: &str, overrides: &[(&str, Value)]) -> Object {
    let base = with(
        any_observation(),
        &[
            ("type", "AGENT".into()),
            ("name", "hermes-run".into()),
            ("parentObservationId", Value::Null),
            ("isRootObservation", true.into()),
            ("input", Value::from(json::quote(prompt))),
        ],
    );
    with(base, overrides)
}

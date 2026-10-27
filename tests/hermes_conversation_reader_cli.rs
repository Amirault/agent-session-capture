//! Port of `hermesConversationReader.test.ts` (CLI cases).

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;
#[path = "../src/tests/hermes_fixture.rs"]
mod hermes_fixture;

use fixtures::TempDir;
use fixtures::fixture_db::{MarkerSeed, QuerySeed, create_fixture, seed_marker as seed_warp_marker, seed_query};
use hermes_fixture::{
    SPEC, SessionOptions, calls, command_args, create_hermes_db, marker, read_jsonl, seed_marker, seed_session,
    seed_text, seed_tool_calls, tool_call,
};

// Keep the crate-root re-exports the shared fixtures resolve through.
#[allow(unused_imports)]
use {adapters as _, domain as _, js as _};

fn run_cli(args: &[&str], temp: &TempDir, env: &[(&str, &str)]) -> common::CliResult {
    let home = temp.join("home");
    std::fs::create_dir_all(&home).expect("home dir");
    let mut all_env = vec![("NO_COLOR", "1"), ("HOME", home.as_str())];
    all_env.extend_from_slice(env);
    common::run_cli(args, &temp.path(), &all_env)
}

#[test]
fn given_exact_lifecycle_markers_when_the_hermes_cli_runs_should_write_a_complete_bundle() {
    // Given
    let temp = TempDir::new("capture-hermes-test-");
    let db_path = temp.join("state.db");
    let db = create_hermes_db(&db_path, "delete");
    for (index, label) in ["specify", "implement", "review"].iter().enumerate() {
        let session_id = format!("session-{label}");
        let offset = index as f64;
        seed_session(
            &db,
            &session_id,
            SessionOptions {
                started_at: Some(1_784_000_000.0 + offset),
                ..Default::default()
            },
        );
        let command = format!("set -euo pipefail\n  {}  \nprintf done", marker(label));
        seed_tool_calls(
            &db,
            &session_id,
            1_784_000_000.0 + offset,
            &calls(&[tool_call(
                &format!("{label}-marker"),
                "terminal",
                &command_args(&command),
            )]),
            None,
        );
        seed_text(&db, &session_id, &format!("{label}-prompt"), 1_784_000_010.0 + offset);
    }
    drop(db);
    let output_dir = temp.join("out");

    // When
    let result = run_cli(
        &[
            "--source",
            "hermes",
            "--hermes-db-path",
            &db_path,
            "--id",
            SPEC,
            "--out",
            &output_dir,
        ],
        &temp,
        &[],
    );

    // Then
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    let lines = read_jsonl(&format!("{output_dir}/{SPEC}.jsonl"));
    assert_eq!(lines[0].get_str("type"), Some("bundle_header"));
    assert_eq!(lines[0].get_str("source"), Some("hermes"));
    assert_eq!(
        lines[0].get("labels").map(js::Value::to_json).as_deref(),
        Some(r#"["specify","implement","review"]"#)
    );
    let contents: Vec<&str> = lines[1..].iter().filter_map(|line| line.get_str("content")).collect();
    for expected in ["specify-prompt", "implement-prompt", "review-prompt"] {
        assert!(contents.contains(&expected), "missing {expected} in {contents:?}");
    }
}

/// TS resolved the default DB path in-process under a stubbed `HERMES_HOME`;
/// Rust cannot mutate its own environment, so the CLI runs with it set: the
/// trimmed home's `state.db` is read, and an explicit `--hermes-db-path` wins.
#[test]
fn given_hermes_home_and_an_explicit_override_when_resolving_the_db_should_trim_home_and_prefer_override() {
    // Given
    let temp = TempDir::new("capture-hermes-test-");
    let db_path = temp.join("state.db");
    let db = create_hermes_db(&db_path, "delete");
    seed_session(&db, "home-session", SessionOptions::default());
    seed_marker(&db, "home-session", "specify", 1_784_000_000.0);
    seed_text(&db, "home-session", "from HERMES_HOME", 1_784_000_001.0);
    drop(db);
    let padded_home = format!("  {}  ", temp.path());
    let output_dir = temp.join("out");

    // When
    let from_home = run_cli(
        &["--source", "hermes", "--id", SPEC, "--out", &output_dir],
        &temp,
        &[("HERMES_HOME", &padded_home)],
    );
    let overridden = run_cli(
        &[
            "--source",
            "hermes",
            "--hermes-db-path",
            "/override/state.db",
            "--id",
            SPEC,
            "--out",
            &temp.join("override-out"),
        ],
        &temp,
        &[("HERMES_HOME", &padded_home)],
    );

    // Then
    assert_eq!(from_home.exit_code, 0, "stderr: {}", from_home.stderr);
    let lines = read_jsonl(&format!("{output_dir}/{SPEC}.jsonl"));
    assert!(
        lines[1..]
            .iter()
            .any(|line| line.get_str("content") == Some("from HERMES_HOME"))
    );
    assert_ne!(overridden.exit_code, 0);
    assert!(
        overridden.stderr.contains("\"/override/state.db\""),
        "stderr: {}",
        overridden.stderr
    );
}

#[test]
fn given_a_warp_fixture_when_the_cli_uses_warp_source_should_write_a_warp_bundle() {
    // Given
    let temp = TempDir::new("capture-hermes-test-");
    let warp_path = temp.join("warp.sqlite");
    let fixture = create_fixture(&warp_path, false);
    seed_warp_marker(
        &fixture,
        MarkerSeed {
            capture_id: SPEC,
            label: "specify",
            start_ts: "2026-07-15 10:00:00.000000",
            conversation_id: "warp-cid",
        },
    );
    seed_query(
        &fixture,
        QuerySeed {
            conversation_id: "warp-cid",
            start_ts: "2026-07-15 10:00:01.000000",
            text: "warp prompt",
            ..Default::default()
        },
    );
    let output_dir = temp.join("warp-out");

    // When
    let result = run_cli(
        &[
            "--source",
            "warp",
            "--id",
            SPEC,
            "--db-path",
            &warp_path,
            "--out",
            &output_dir,
        ],
        &temp,
        &[],
    );

    // Then
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    assert_eq!(
        read_jsonl(&format!("{output_dir}/{SPEC}.jsonl"))[0].get_str("source"),
        Some("warp")
    );
    drop(fixture);
}

#[test]
fn given_an_unknown_source_when_the_cli_runs_should_list_hermes_without_changing_usage_exit_code() {
    // Given / When
    let temp = TempDir::new("capture-hermes-test-");
    let result = run_cli(&["--source", "unknown", "--id", SPEC], &temp, &[]);

    // Then
    assert_eq!(result.exit_code, 2);
    assert!(
        result
            .stderr
            .contains(r#""warp", "claude-code", "hermes", "langfuse", or "oz""#),
        "stderr: {}",
        result.stderr
    );
}

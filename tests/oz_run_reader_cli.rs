//! `--source oz` end to end: saved runs (`--oz-dir`) and live runs (`--oz-runs`,
//! through a fake `oz` executable on `PATH`).

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;

use fixtures::TempDir;
use fixtures::oz_runs::{conversation_json, message, run_command, run_json, step, text};

// Keep the crate-root re-exports the shared fixtures resolve through.
#[allow(unused_imports)]
use {adapters as _, domain as _, js as _};

const MARKER: &str = ": CAPTURE_MARKER v=1 id=greeting-fix";

fn conversation(cid: &str, marker: &str) -> String {
    conversation_json(
        cid,
        &[step(
            &[
                message("user", "2026-10-20T09:00:01Z", "req-1", &[text("say hello")]),
                message(
                    "assistant",
                    "2026-10-20T09:00:02Z",
                    "req-1",
                    &[run_command("c1", marker)],
                ),
            ],
            &[],
        )],
    )
}

fn run_cli(args: &[&str], temp: &TempDir, env: &[(&str, &str)]) -> common::CliResult {
    let home = temp.join("home");
    std::fs::create_dir_all(&home).expect("home dir");
    let mut all_env = vec![("NO_COLOR", "1"), ("HOME", home.as_str())];
    all_env.extend_from_slice(env);
    common::run_cli(args, &temp.path(), &all_env)
}

fn read_lines(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("read {path}: {e}"))
        .lines()
        .map(str::to_string)
        .collect()
}

#[test]
fn given_saved_runs_when_the_oz_cli_runs_should_write_the_marked_conversation_bundle() {
    // Given
    let temp = TempDir::new("capture-oz-dir-");
    let runs = temp.join("runs");
    std::fs::create_dir_all(&runs).expect("runs dir");
    for (run_id, cid, marker) in [
        ("run-a", "conv-a", MARKER),
        ("run-b", "conv-b", ": CAPTURE_MARKER v=1 id=other"),
    ] {
        let saved = format!(
            r#"{{"run":{},"conversation":{}}}"#,
            run_json(run_id, cid, "2026-10-20T09:10:00Z"),
            conversation(cid, marker)
        );
        std::fs::write(format!("{runs}/{cid}.json"), saved).expect("write run");
    }

    // When
    let result = run_cli(
        &[
            "--source",
            "oz",
            "--id",
            "greeting-fix",
            "--oz-dir",
            &runs,
            "--out",
            "out",
        ],
        &temp,
        &[],
    );

    // Then
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    assert!(
        result.stdout.contains("(2 events, 1 conversations)"),
        "{}",
        result.stdout
    );
    let lines = read_lines(&temp.join("out/greeting-fix.jsonl"));
    assert!(lines[0].contains(r#""source":"oz""#), "{}", lines[0]);
    assert!(lines[0].contains(r#""conversation_ids":["conv-a"]"#), "{}", lines[0]);
    assert!(
        lines[1].contains(r#""kind":"query""#) && lines[1].contains(r#""usage":"#),
        "{}",
        lines[1]
    );
    assert!(lines[2].contains(r#""kind":"command""#), "{}", lines[2]);
}

#[cfg(unix)]
#[test]
fn given_run_ids_when_the_oz_cli_runs_should_fetch_them_through_oz_and_export_sessions() {
    use std::os::unix::fs::PermissionsExt;

    // Given
    let temp = TempDir::new("capture-oz-runs-");
    let bin = temp.join("bin");
    std::fs::create_dir_all(&bin).expect("bin dir");
    std::fs::write(
        temp.join("run.json"),
        run_json("run-a", "conv-a", "2026-10-20T09:10:00Z"),
    )
    .expect("run");
    std::fs::write(temp.join("conversation.json"), conversation("conv-a", MARKER)).expect("conversation");
    let script = format!(
        "#!/bin/sh\necho \"$@\" >> '{log}'\nif [ \"$2\" = conversation ]; then cat '{conversation}'; else cat '{run}'; fi\n",
        log = temp.join("oz.log"),
        conversation = temp.join("conversation.json"),
        run = temp.join("run.json"),
    );
    let oz = format!("{bin}/oz");
    std::fs::write(&oz, script).expect("fake oz");
    std::fs::set_permissions(&oz, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = format!("{bin}:{}", std::env::var("PATH").unwrap_or_default());

    // When
    let result = run_cli(
        &["--sessions", "--source", "oz", "--oz-runs", "run-a", "--out", "out"],
        &temp,
        &[("PATH", &path)],
    );

    // Then
    assert_eq!(result.exit_code, 0, "{}", result.stderr);
    let lines = read_lines(&temp.join("out/sessions/conv-a.jsonl"));
    assert!(lines[0].contains(r#""source":"oz""#), "{}", lines[0]);
    assert!(
        lines[0].contains(r#""last_modified":"2026-10-20T09:10:00Z""#),
        "{}",
        lines[0]
    );
    assert_eq!(lines.len(), 3);
    assert_eq!(
        read_lines(&temp.join("oz.log")),
        vec![
            "run get run-a --output-format json",
            "run conversation get conv-a --output-format json",
        ]
    );
}

#[test]
fn given_no_runs_when_the_oz_cli_runs_should_exit_with_a_usage_error() {
    // Given
    let temp = TempDir::new("capture-oz-none-");

    // When
    let result = run_cli(&["--source", "oz", "--id", "greeting-fix"], &temp, &[]);

    // Then
    assert_eq!(result.exit_code, 2, "{}", result.stderr);
    assert!(result.stderr.contains("--oz-runs"), "{}", result.stderr);
}

//! Port of `hermesSessionReader.test.ts` (CLI case).

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;
#[path = "../src/tests/hermes_fixture.rs"]
mod hermes_fixture;

use fixtures::TempDir;
use hermes_fixture::{BRANCH_ID, ROOT_ID, seed_example3};

// Keep the crate-root re-exports the shared fixtures resolve through.
#[allow(unused_imports)]
use {adapters as _, domain as _, js as _};

#[test]
fn given_example3_on_disk_when_the_cli_reruns_sessions_source_hermes_then_both_exit_0_and_rerun_prints_2_unchanged() {
    // Given
    let temp = TempDir::new("wa-hermes-sessions-");
    let db_path = temp.join("state.db");
    let out_dir = temp.join("out");
    let home = temp.join("home");
    std::fs::create_dir_all(&home).expect("home dir");
    seed_example3(&db_path);
    let run = || {
        common::run_cli(
            &[
                "--sessions",
                "--source",
                "hermes",
                "--hermes-db-path",
                &db_path,
                "--out",
                &out_dir,
            ],
            &temp.path(),
            &[("HOME", &home)],
        )
    };
    let first = run();

    // When
    let second = run();

    // Then
    assert_eq!(first.exit_code, 0, "stderr: {}", first.stderr);
    assert!(std::path::Path::new(&format!("{out_dir}/sessions/{ROOT_ID}.jsonl")).exists());
    assert!(std::path::Path::new(&format!("{out_dir}/sessions/{BRANCH_ID}.jsonl")).exists());
    assert_eq!(second.exit_code, 0, "stderr: {}", second.stderr);
    assert_eq!(second.stdout, "2 sessions: 0 written, 2 unchanged, 0 untouched\n");
}

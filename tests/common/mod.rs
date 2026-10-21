//! Helpers for the CLI integration tests: run the built binary.

#![allow(dead_code)]

use std::process::{Command, Output};

/// The CLI's captured result.
pub struct CliResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

impl From<Output> for CliResult {
    fn from(output: Output) -> Self {
        Self {
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
            exit_code: output.status.code().unwrap_or(1),
        }
    }
}

/// Run the CLI with `args` from `cwd`, adding `env` and removing `unset`
/// variables (every `LANGFUSE_*` variable is always removed, and `HOME` must
/// be set by callers that read credentials).
pub fn run_cli(args: &[&str], cwd: &str, env: &[(&str, &str)]) -> CliResult {
    let mut command = Command::new(env!("CARGO_BIN_EXE_agent-session-capture"));
    command.args(args).current_dir(cwd);
    for (name, _) in std::env::vars() {
        if name.starts_with("LANGFUSE_") {
            command.env_remove(name);
        }
    }
    for (k, v) in env {
        command.env(k, v);
    }
    command.output().expect("run the CLI").into()
}

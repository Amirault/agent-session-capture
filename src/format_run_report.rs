//! Pure CLI diagnostics: success/anomaly lines and exit code.

use crate::domain::models::{RunSummary, SessionRunSummary, SkippedRow};

/// What the CLI should print and exit with for one run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunReport {
    pub exit_code: i32,
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
}

const BINDING_DECAY_HINT: &str = "a marker was found but its binding block is gone — Warp can evict blocks rows over time; extract soon after the session, before the binding decays.";

fn skipped_line(r: &SkippedRow) -> String {
    format!("skipped row: table={} reason=\"{}\" ({})", r.table, r.reason, r.detail)
}

/// One line per anomaly (never a bare count): a binding failure or a skipped
/// row hides its cause behind a number.
fn anomaly_lines(summary: &RunSummary) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(error) = &summary.fresh_read_error {
        lines.push(format!("fresh read failed: {error} — using stored bundle"));
    }
    for s in &summary.heuristic_bindings {
        lines.push(format!(
            "heuristic binding: label={} start_ts={} — bound via ai_queries temporal fallback (local orchestrated subagent); verify conversation is correct",
            s.label, s.start_ts
        ));
    }
    for s in &summary.unbindable {
        lines.push(format!("unbindable marker: label={} start_ts={}", s.label, s.start_ts));
    }
    for s in &summary.collisions {
        lines.push(format!(
            "collision marker: label={} start_ts={} (bound to multiple conversations)",
            s.label, s.start_ts
        ));
    }
    lines.extend(summary.skipped_rows.iter().map(skipped_line));
    lines
}

/// Format the report for one marker-mode run. `out_path` is the file written,
/// or `None` when nothing was written because zero conversations bound.
pub fn format_run_report(summary: &RunSummary, out_path: Option<&str>) -> RunReport {
    let anomalies = anomaly_lines(summary);
    if let Some(out_path) = out_path {
        let stdout = vec![format!(
            "wrote {out_path} ({} events, {} conversations)",
            summary.events, summary.conversations
        )];
        let stderr = if anomalies.is_empty() {
            vec![]
        } else {
            std::iter::once("warnings:".to_string()).chain(anomalies).collect()
        };
        return RunReport {
            exit_code: 0,
            stdout,
            stderr,
        };
    }
    let mut stderr = vec![format!(
        "no conversations bound for capture id \"{}\" — nothing written.",
        summary.capture_id
    )];
    stderr.extend(anomalies);
    if !summary.unbindable.is_empty() {
        stderr.push(BINDING_DECAY_HINT.to_string());
    }
    RunReport {
        exit_code: 1,
        stdout: vec![],
        stderr,
    }
}

/// Format the report for one `--sessions` run: one `wrote <path> (N events)`
/// line per written file, then the `N sessions: W written, S unchanged, U
/// untouched` tally; exit 1 when zero sessions were enumerated.
pub fn format_session_report(summary: &SessionRunSummary) -> RunReport {
    let mut warnings = summary.warnings.clone();
    warnings.extend(summary.skipped_rows.iter().map(skipped_line));

    if summary.enumerated == 0 {
        let mut reasons = Vec::new();
        if let Some(since) = &summary.since {
            reasons.push(format!("--since {since} too recent"));
        }
        if let Some(conversation) = &summary.conversation {
            reasons.push(format!("unknown --conversation {conversation}"));
        }
        let reason_text = if reasons.is_empty() {
            String::new()
        } else {
            format!(" ({})", reasons.join(", "))
        };
        let mut stderr = vec![format!("no sessions enumerated{reason_text} — nothing written.")];
        stderr.extend(warnings);
        return RunReport {
            exit_code: 1,
            stdout: vec![],
            stderr,
        };
    }

    let mut stdout: Vec<String> = summary
        .written
        .iter()
        .map(|w| format!("wrote {} ({} events)", w.path, w.events))
        .collect();
    stdout.push(format!(
        "{} sessions: {} written, {} unchanged, {} untouched",
        summary.enumerated,
        summary.written.len(),
        summary.unchanged.len(),
        summary.untouched.len()
    ));
    let stderr = if warnings.is_empty() {
        vec![]
    } else {
        std::iter::once("warnings:".to_string()).chain(warnings).collect()
    };
    RunReport {
        exit_code: 0,
        stdout,
        stderr,
    }
}

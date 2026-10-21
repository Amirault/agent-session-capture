//! Port of `formatSessionReport.test.ts` — §9.22 session-mode CLI report and exit codes.

use crate::domain::models::{SessionRunSummary, SkippedRow, WrittenSession};
use crate::format_run_report::format_session_report;

fn written(conversation_id: &str, path: &str, events: usize) -> WrittenSession {
    WrittenSession {
        conversation_id: conversation_id.to_string(),
        path: path.to_string(),
        events,
    }
}

fn session_summary() -> SessionRunSummary {
    SessionRunSummary {
        since: None,
        conversation: None,
        enumerated: 3,
        written: vec![
            written("c1", "/tmp/out/sessions/c1.jsonl", 12),
            written("c2", "/tmp/out/sessions/c2.jsonl", 3),
        ],
        unchanged: vec!["c3".to_string()],
        untouched: vec!["/tmp/out/sessions/c9.jsonl".to_string()],
        skipped_rows: vec![],
        warnings: vec![],
    }
}

/// `/\b\d+ skipped rows\b/`: a run of digits preceded by a non-word boundary,
/// then " skipped rows" followed by a word boundary.
fn has_bare_skipped_rows_count(text: &str) -> bool {
    let bytes = text.as_bytes();
    let is_word = |b: u8| b.is_ascii_alphanumeric() || b == b'_';
    let needle = b" skipped rows";
    for start in 0..bytes.len() {
        if !bytes[start].is_ascii_digit() || (start > 0 && is_word(bytes[start - 1])) {
            continue;
        }
        let mut end = start;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
        }
        if bytes[end..].starts_with(needle) {
            let after = end + needle.len();
            if after >= bytes.len() || !is_word(bytes[after]) {
                return true;
            }
        }
    }
    false
}

#[test]
fn written_unchanged_and_untouched_sessions_print_one_wrote_line_per_file_and_a_tally() {
    // Given
    let summary = session_summary();

    // When
    let report = format_session_report(&summary);

    // Then
    assert_eq!(report.exit_code, 0);
    assert_eq!(
        report.stdout,
        vec![
            "wrote /tmp/out/sessions/c1.jsonl (12 events)",
            "wrote /tmp/out/sessions/c2.jsonl (3 events)",
            "3 sessions: 2 written, 1 unchanged, 1 untouched",
        ]
    );
    assert!(report.stderr.is_empty());
}

#[test]
fn every_session_up_to_date_prints_only_the_tally_and_exits_0() {
    // Given
    let summary = SessionRunSummary {
        written: vec![],
        unchanged: vec!["c1".to_string(), "c2".to_string(), "c3".to_string()],
        untouched: vec![],
        ..session_summary()
    };

    // When
    let report = format_session_report(&summary);

    // Then
    assert_eq!(report.exit_code, 0);
    assert_eq!(report.stdout, vec!["3 sessions: 0 written, 3 unchanged, 0 untouched"]);
    assert!(report.stderr.is_empty());
}

#[test]
fn zero_enumerated_sessions_after_since_filter_exits_1_and_names_the_reason() {
    // Given
    let summary = SessionRunSummary {
        since: Some("2100-01-01".to_string()),
        enumerated: 0,
        written: vec![],
        unchanged: vec![],
        untouched: vec![],
        ..session_summary()
    };

    // When
    let report = format_session_report(&summary);

    // Then
    assert_eq!(report.exit_code, 1);
    assert!(report.stdout.is_empty());
    assert_eq!(
        report.stderr[0],
        "no sessions enumerated (--since 2100-01-01 too recent) — nothing written."
    );
}

#[test]
fn zero_enumerated_sessions_after_conversation_filter_exits_1_and_names_the_unknown_conversation() {
    // Given
    let summary = SessionRunSummary {
        conversation: Some("nope".to_string()),
        enumerated: 0,
        written: vec![],
        unchanged: vec![],
        untouched: vec![],
        ..session_summary()
    };

    // When
    let report = format_session_report(&summary);

    // Then
    assert_eq!(report.exit_code, 1);
    assert_eq!(
        report.stderr[0],
        "no sessions enumerated (unknown --conversation nope) — nothing written."
    );
}

#[test]
fn guard_warnings_and_skipped_rows_print_one_per_line_under_a_warnings_header() {
    // Given
    let summary = SessionRunSummary {
        warnings: vec![
            "replacing stored session (--no-merge): corrupt session file \"/tmp/out/sessions/c2.jsonl\": header line is not valid JSON".to_string(),
        ],
        skipped_rows: vec![
            SkippedRow::new("agent_tasks", "empty task", "task_id=t-1"),
            SkippedRow::new("agent_tasks", "walk threw", "task_id=t-2: truncated varint"),
        ],
        ..session_summary()
    };

    // When
    let report = format_session_report(&summary);

    // Then — two skipped rows => two distinct lines, not "2 skipped rows"
    assert_eq!(report.exit_code, 0);
    assert_eq!(
        report.stderr,
        vec![
            "warnings:",
            "replacing stored session (--no-merge): corrupt session file \"/tmp/out/sessions/c2.jsonl\": header line is not valid JSON",
            "skipped row: table=agent_tasks reason=\"empty task\" (task_id=t-1)",
            "skipped row: table=agent_tasks reason=\"walk threw\" (task_id=t-2: truncated varint)",
        ]
    );
    assert!(!has_bare_skipped_rows_count(&report.stderr.join("\n")));
}

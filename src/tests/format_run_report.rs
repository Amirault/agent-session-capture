//! Port of `formatRunReport.test.ts` — §9.13 formatRunReport, CLI diagnostics.

use crate::domain::models::{RunSummary, SeedConfidence, SeedMatch, SeedStatus, SkippedRow};
use crate::format_run_report::format_run_report;

const SPEC: &str = "add-feature-x";

fn base_summary() -> RunSummary {
    RunSummary {
        capture_id: SPEC.to_string(),
        conversations: 1,
        events: 3,
        ..RunSummary::default()
    }
}

fn marker(label: &str, start_ts: &str, conversation_id: Option<&str>, status: SeedStatus) -> SeedMatch {
    SeedMatch {
        conversation_id: conversation_id.map(str::to_string),
        label: label.to_string(),
        marker_command: format!(": CAPTURE_MARKER v=1 id={SPEC} label={label}"),
        start_ts: start_ts.to_string(),
        status,
        confidence: None,
    }
}

fn heuristic_marker(label: &str, start_ts: &str) -> SeedMatch {
    SeedMatch {
        confidence: Some(SeedConfidence::Heuristic),
        ..marker(label, start_ts, Some("c-heuristic"), SeedStatus::Bound)
    }
}

fn unbindable_marker(label: &str, start_ts: &str) -> SeedMatch {
    marker(label, start_ts, None, SeedStatus::Unbindable)
}

fn collision_marker(label: &str, start_ts: &str) -> SeedMatch {
    marker(label, start_ts, None, SeedStatus::Collision)
}

#[test]
fn fully_bound_spec_with_written_bundle_succeeds_with_no_anomaly_lines() {
    // Given
    let summary = base_summary();
    let out_path = "out/add-feature-x.jsonl";

    // When
    let report = format_run_report(&summary, Some(out_path));

    // Then
    assert_eq!(report.exit_code, 0);
    assert_eq!(report.stdout.len(), 1);
    assert!(report.stdout[0].contains("wrote"));
    assert!(report.stdout[0].contains(out_path));
    assert!(report.stderr.is_empty());
}

#[test]
fn zero_bound_conversations_with_unbindable_marker_exits_1_with_binding_decay_hint_and_detail() {
    // Given — the marker was found but its binding block has decayed
    let summary = RunSummary {
        conversations: 0,
        events: 0,
        unbindable: vec![unbindable_marker("implement", "2026-06-30 11:00:00.000000")],
        ..base_summary()
    };

    // When
    let report = format_run_report(&summary, None);

    // Then
    assert_eq!(report.exit_code, 1);
    assert!(report.stdout.is_empty());
    let joined = report.stderr.join("\n");
    assert!(joined.contains("no conversations bound"));
    assert!(joined.contains(SPEC));
    assert!(joined.contains("label=implement"));
    assert!(joined.contains("2026-06-30 11:00:00.000000"));
    assert!(joined.to_lowercase().contains("binding"));
}

#[test]
fn heuristic_bound_marker_warning_names_label_timestamp_and_fallback_strategy() {
    // Given
    let summary = RunSummary {
        heuristic_bindings: vec![heuristic_marker("implement", "2026-09-11 02:58:24.952686")],
        ..base_summary()
    };
    let out_path = "out/add-feature-x.jsonl";

    // When
    let report = format_run_report(&summary, Some(out_path));

    // Then
    assert_eq!(report.exit_code, 0);
    assert!(report.stdout[0].contains("wrote"));
    let joined = report.stderr.join("\n");
    assert!(joined.contains("heuristic binding"));
    assert!(joined.contains("label=implement"));
    assert!(joined.contains("2026-09-11 02:58:24.952686"));
    assert!(joined.to_lowercase().contains("ai_queries"));
}

#[test]
fn written_bundle_with_unbindable_and_collision_succeeds_and_details_every_anomaly() {
    // Given
    let summary = RunSummary {
        unbindable: vec![unbindable_marker("specify", "2026-06-30 09:00:00.000000")],
        collisions: vec![collision_marker("implement", "2026-06-30 10:00:00.000000")],
        skipped_rows: vec![SkippedRow::new("ai_queries", "input failed schema", "cid=c9 ts=x")],
        ..base_summary()
    };
    let out_path = "out/add-feature-x.jsonl";

    // When
    let report = format_run_report(&summary, Some(out_path));

    // Then
    assert_eq!(report.exit_code, 0);
    assert!(report.stdout[0].contains("wrote"));
    let joined = report.stderr.join("\n");
    assert!(joined.contains("label=specify"));
    assert!(joined.contains("2026-06-30 09:00:00.000000"));
    assert!(joined.contains("label=implement"));
    assert!(joined.contains("2026-06-30 10:00:00.000000"));
    assert!(joined.contains("ai_queries"));
    assert!(joined.contains("input failed schema"));
    // no bare "N unbindable, N collisions" style summary line
    assert!(!joined.contains("1 unbindable, 1 collisions"));
}

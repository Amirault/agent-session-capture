//! Port of `markerReader.test.ts` — parseMarker, and §9.2/§9.3 selection & binding.

use rusqlite::{Connection, params};

use super::fixtures::TempDir;
use super::fixtures::fixture_db::{
    MarkerSeed, QuerySeed, ansi_wrap, create_fixture, seed_marker, seed_query, seed_unbindable_marker,
};
use crate::adapters::readers::marker_reader::{ParsedMarker, find_seeds, parse_marker};
use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::models::{SeedConfidence, SeedMatch, SeedStatus};

fn parsed(capture_id: &str, label: &str) -> Option<ParsedMarker> {
    Some(ParsedMarker {
        capture_id: capture_id.to_string(),
        label: label.to_string(),
    })
}

// ---- parseMarker ----

#[test]
fn parses_a_well_formed_marker_for_each_label() {
    assert_eq!(
        parse_marker(": CAPTURE_MARKER v=1 id=2026-06-30-x label=specify"),
        parsed("2026-06-30-x", "specify")
    );
    assert_eq!(
        parse_marker(": CAPTURE_MARKER v=1 id=y label=review"),
        parsed("y", "review")
    );
    assert_eq!(
        parse_marker(": CAPTURE_MARKER v=1 id=y label=step.2/retry"),
        parsed("y", "step.2/retry")
    );
}

#[test]
fn rejects_non_marker_text_that_merely_mentions_the_marker() {
    assert_eq!(parse_marker("python3 -c 'print(\"CAPTURE_MARKER\")'"), None);
    assert_eq!(parse_marker("grep CAPTURE_MARKER commands"), None);
}

#[test]
fn defaults_the_label_when_the_marker_carries_none() {
    assert_eq!(parse_marker(": CAPTURE_MARKER v=1 id=x"), parsed("x", "default"));
}

#[test]
fn rejects_a_marker_without_the_v1_version_token() {
    assert_eq!(parse_marker(": CAPTURE_MARKER id=x"), None);
    assert_eq!(parse_marker(": CAPTURE_MARKER v=2 id=x"), None);
}

#[test]
fn rejects_a_marker_without_an_id_or_with_an_empty_id() {
    assert_eq!(parse_marker(": CAPTURE_MARKER v=1 label=review"), None);
    assert_eq!(parse_marker(": CAPTURE_MARKER v=1 id= label=review"), None);
}

#[test]
fn parses_a_marker_chained_with_the_real_command_on_the_next_line() {
    assert_eq!(
        parse_marker(
            ": CAPTURE_MARKER v=1 id=2026-06-30-x label=specify\nscripts/validate-spec.sh specs/todo/2026-06-30-x.md"
        ),
        parsed("2026-06-30-x", "specify")
    );
}

fn assert_chained_marker_recognized(command: &str, label: &str) {
    // When
    let result = parse_marker(command);

    // Then
    assert_eq!(result, parsed("2026-06-30-x", label));
}

#[test]
fn marker_chained_after_a_cd_prefix_is_recognized() {
    // Given / When / Then
    assert_chained_marker_recognized(
        "cd /repo/app; : CAPTURE_MARKER v=1 id=2026-06-30-x label=review",
        "review",
    );
}

#[test]
fn marker_chained_after_a_cd_prefix_with_a_follow_up_command_on_the_next_line_is_recognized() {
    // Given / When / Then
    assert_chained_marker_recognized(
        "cd /repo/app; : CAPTURE_MARKER v=1 id=2026-06-30-x label=review\ncat skills/review/SKILL.md",
        "review",
    );
}

#[test]
fn marker_chained_after_a_script_with_and_and_is_recognized() {
    // Given / When / Then
    assert_chained_marker_recognized(
        "scripts/transition-spec.sh specs/todo/2026-06-30-x.md implementation-in-progress && : CAPTURE_MARKER v=1 id=2026-06-30-x label=implement",
        "implement",
    );
}

fn assert_not_an_emission(command: &str) {
    // When
    let result = parse_marker(command);

    // Then
    assert_eq!(result, None);
}

#[test]
fn marker_shaped_text_inside_a_heredoc_body_line_is_not_an_emission() {
    assert_not_an_emission("cat > notes.md <<'EOF'\n: CAPTURE_MARKER v=1 id=2026-06-30-x label=specify\nEOF");
}

#[test]
fn marker_shaped_text_inside_a_single_quoted_echo_argument_is_not_an_emission() {
    assert_not_an_emission("echo ': CAPTURE_MARKER v=1 id=2026-06-30-x label=specify'");
}

#[test]
fn marker_shaped_text_inside_a_quoted_grep_pattern_is_not_an_emission() {
    assert_not_an_emission("grep -c ': CAPTURE_MARKER v=1 id=2026-06-30-x' transcript.jsonl");
}

#[test]
fn marker_shaped_text_after_a_semicolon_inside_single_quotes_is_not_an_emission() {
    assert_not_an_emission("echo 'done; : CAPTURE_MARKER v=1 id=2026-06-30-x label=specify'");
}

#[test]
fn marker_shaped_text_after_and_and_inside_double_quotes_is_not_an_emission() {
    assert_not_an_emission("echo \"done && : CAPTURE_MARKER v=1 id=2026-06-30-x label=specify\"");
}

// ---- §9.2/§9.3 markerReader — selection & binding ----

struct Fixture {
    _tmp: TempDir,
    db: Connection,
}

fn fixture() -> Fixture {
    let tmp = TempDir::new("wa-93-");
    let db = create_fixture(&tmp.join("f.db"), false);
    Fixture { _tmp: tmp, db }
}

fn seeds_of(db: Connection, capture_id: &str) -> Vec<SeedMatch> {
    find_seeds(&ReadableDb::new(db), capture_id).expect("find seeds")
}

#[test]
fn marker_and_block_at_same_start_ts_bind_the_conversation_with_the_marker_label() {
    // Given
    let Fixture { _tmp, db } = fixture();
    seed_marker(
        &db,
        MarkerSeed {
            capture_id: "X",
            label: "specify",
            conversation_id: "C1",
            start_ts: "2026-06-30 10:00:00.000000",
        },
    );

    // When
    let seeds = seeds_of(db, "X");

    // Then
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].conversation_id.as_deref(), Some("C1"));
    assert_eq!(seeds[0].label, "specify");
    assert_eq!(seeds[0].status, SeedStatus::Bound);
}

#[test]
fn two_specs_with_distinct_markers_select_only_their_own_conversations() {
    // Given
    let Fixture { _tmp, db } = fixture();
    seed_marker(
        &db,
        MarkerSeed {
            capture_id: "X",
            label: "specify",
            conversation_id: "CX",
            start_ts: "2026-06-30 10:00:00.000000",
        },
    );
    seed_marker(
        &db,
        MarkerSeed {
            capture_id: "Y",
            label: "implement",
            conversation_id: "CY",
            start_ts: "2026-06-30 11:00:00.000000",
        },
    );
    let readable = ReadableDb::new(db);
    let cids = |capture_id: &str| -> Vec<Option<String>> {
        find_seeds(&readable, capture_id)
            .expect("find seeds")
            .into_iter()
            .map(|s| s.conversation_id)
            .collect()
    };

    // When / Then
    assert_eq!(cids("X"), vec![Some("CX".to_string())]);
    assert_eq!(cids("Y"), vec![Some("CY".to_string())]);
}

#[test]
fn marker_with_no_block_and_no_ai_queries_entry_is_unbindable() {
    // Given
    let Fixture { _tmp, db } = fixture();
    seed_unbindable_marker(&db, "X", "specify", "2026-06-30 10:00:00.000000");
    // no ai_queries row seeded

    // When
    let seeds = seeds_of(db, "X");

    // Then
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].status, SeedStatus::Unbindable);
    assert_eq!(seeds[0].conversation_id, None);
}

#[test]
fn marker_with_no_block_but_ai_queries_entry_within_10_minutes_binds_with_heuristic_confidence() {
    // Given — local orchestrated subagent: commands row, no blocks row, ai_queries entry 2 min before
    let Fixture { _tmp, db } = fixture();
    seed_unbindable_marker(&db, "X", "implement", "2026-06-30 10:02:00.000000");
    seed_query(
        &db,
        QuerySeed {
            conversation_id: "C-heuristic",
            start_ts: "2026-06-30 10:00:00.000000", // 2 min before marker
            text: "implement query",
            ..QuerySeed::default()
        },
    );

    // When
    let seeds = seeds_of(db, "X");

    // Then
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].status, SeedStatus::Bound);
    assert_eq!(seeds[0].conversation_id.as_deref(), Some("C-heuristic"));
    assert_eq!(seeds[0].label, "implement");
    assert_eq!(seeds[0].confidence, Some(SeedConfidence::Heuristic));
}

#[test]
fn marker_with_no_block_and_ai_queries_entry_more_than_10_minutes_before_stays_unbindable() {
    // Given — ai_queries entry is 15 minutes before, outside the 10-min window
    let Fixture { _tmp, db } = fixture();
    seed_unbindable_marker(&db, "X", "implement", "2026-06-30 10:15:00.000000");
    seed_query(
        &db,
        QuerySeed {
            conversation_id: "C-stale",
            start_ts: "2026-06-30 10:00:00.000000", // 15 min before — outside 10-min window
            text: "stale query",
            ..QuerySeed::default()
        },
    );

    // When
    let seeds = seeds_of(db, "X");

    // Then
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].status, SeedStatus::Unbindable);
    assert_eq!(seeds[0].conversation_id, None);
}

#[test]
fn normally_bound_marker_has_no_confidence_field() {
    // Given
    let Fixture { _tmp, db } = fixture();
    seed_marker(
        &db,
        MarkerSeed {
            capture_id: "X",
            label: "specify",
            conversation_id: "C1",
            start_ts: "2026-06-30 10:00:00.000000",
        },
    );

    // When
    let seeds = seeds_of(db, "X");

    // Then
    assert_eq!(seeds.len(), 1);
    assert_eq!(seeds[0].status, SeedStatus::Bound);
    assert_eq!(seeds[0].confidence, None); // certain binding: no confidence field
}

#[test]
fn marker_text_only_in_blocks_stylized_command_is_not_matched() {
    // Given — a block whose stylized_command holds the marker text, but no commands row
    let Fixture { _tmp, db } = fixture();
    db.execute(
        "INSERT INTO blocks (pane_leaf_uuid, stylized_command, stylized_output, pwd, git_branch, exit_code, did_execute, start_ts, ai_metadata)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            vec![1u8],
            ansi_wrap(": CAPTURE_MARKER v=1 id=X label=specify"),
            Vec::<u8>::new(),
            "/repo",
            "main",
            0,
            1,
            "2026-06-30 10:00:00.000000",
            "{\"conversation_id\":\"C-ansi\"}"
        ],
    )
    .expect("insert block");

    // When / Then
    assert_eq!(seeds_of(db, "X"), vec![]);
}

#[test]
fn diagnostic_command_that_merely_mentions_the_marker_is_not_matched() {
    // Given — an exploration script whose command text contains "CAPTURE_MARKER"
    let Fixture { _tmp, db } = fixture();
    db.execute(
        "INSERT INTO commands (command, start_ts, is_agent_executed) VALUES (?, ?, 1)",
        params![
            "python3 -c 'print(\"CAPTURE_MARKER v=1 id=X label=specify\")'",
            "2026-06-30 10:00:00.000000"
        ],
    )
    .expect("insert command");

    // When / Then
    assert_eq!(seeds_of(db, "X"), vec![]);
}

#[test]
fn no_markers_for_the_slug_returns_no_seeds() {
    let Fixture { _tmp, db } = fixture();
    assert_eq!(seeds_of(db, "missing"), vec![]);
}

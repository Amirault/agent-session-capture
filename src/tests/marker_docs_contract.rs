//! Port of `markerDocsContract.test.ts` — documented markers stay parseable.

use crate::adapters::readers::marker_reader::parse_marker;

fn read_repo_file(rel: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// JavaScript `\s`.
fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\t' | '\n' | '\u{b}' | '\u{c}' | '\r' | ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}' | '\u{feff}'
    )
}

/// JavaScript line terminators (where `^`/`$` match under the `m` flag).
fn is_js_line_terminator(c: char) -> bool {
    matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}')
}

fn is_non_space_run(s: &str) -> bool {
    !s.is_empty() && !s.chars().any(is_js_whitespace)
}

/// Every whole line matching `/^: CAPTURE_MARKER v=1 id=\S+( label=\S+)?$/gm`.
fn marker_lines(content: &str) -> Vec<&str> {
    const PREFIX: &str = ": CAPTURE_MARKER v=1 id=";
    content
        .split(is_js_line_terminator)
        .filter(|line| {
            let Some(rest) = line.strip_prefix(PREFIX) else {
                return false;
            };
            if is_non_space_run(rest) {
                return true;
            }
            match rest.split_once(" label=") {
                Some((id, label)) => is_non_space_run(id) && is_non_space_run(label),
                None => false,
            }
        })
        .collect()
}

/// `content.replace(/\s+/g, " ")`.
fn collapse_whitespace(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut in_space = false;
    for c in content.chars() {
        if is_js_whitespace(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

fn assert_concrete_markers_parse(file: &str) {
    // Given
    let content = read_repo_file(file);

    // When
    let concrete: Vec<&str> = marker_lines(&content)
        .into_iter()
        .filter(|line| !line.contains('<'))
        .collect();

    // Then
    assert!(!concrete.is_empty(), "{file} must show a concrete marker");
    for line in concrete {
        assert!(parse_marker(line).is_some(), "{line}");
    }
}

#[test]
fn readme_concrete_marker_examples_all_bind() {
    assert_concrete_markers_parse("README.md");
}

#[test]
fn marker_doc_concrete_marker_examples_all_bind() {
    assert_concrete_markers_parse("docs/MARKER.md");
}

#[test]
fn capture_marker_skill_concrete_marker_examples_all_bind() {
    assert_concrete_markers_parse("skills/capture-marker/SKILL.md");
}

#[test]
fn marker_skill_capture_commands_each_select_the_runtime_source_explicitly() {
    // Given
    let content = collapse_whitespace(&read_repo_file("skills/capture-marker/SKILL.md"));

    // When
    let commands = content.matches("capture.sh --id").count();

    // Then
    assert!(commands > 0);
    assert_eq!(content.matches("--source <session_source>").count(), commands);
    assert!(content.contains("Warp → `warp`, Claude Code → `claude-code`, Hermes → `hermes`"));
}

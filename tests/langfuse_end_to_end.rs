//! Port of `langfuseEndToEnd.test.ts`: CLI -> client -> loopback v2 endpoint -> sink,
//! and the default store `capture.sh` picks for `--source langfuse`.

use agent_session_capture::{adapters, domain, js};
mod common;
#[path = "../src/tests/fixtures/mod.rs"]
mod fixtures;

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

use common::{CliResult, run_cli};
use domain::bundle_header::compute_bundle_header;
use domain::models::BundleSource;
use domain::ordered_map::OrderedMap;
use fixtures::TempDir;
use fixtures::fake_langfuse_server::{FakeServerOptions, TEST_PUBLIC_KEY, TEST_SECRET_KEY, start_fake_langfuse};
use fixtures::langfuse_observations::{generation, marker_command, terminal_call, trace_root};
use js::{Object, Value, date, json, path};

const CAPTURE_ID: &str = "checkout-bug";
const MISSING_CREDENTIALS_ERROR: &str = "error: Langfuse credentials missing — set LANGFUSE_HOST, LANGFUSE_PUBLIC_KEY, LANGFUSE_SECRET_KEY (env, ~/.config/langfuse/capture.env, or --langfuse-env)\n";

/// A temp dir and its `.agent-captures` store.
struct Context {
    tmp: TempDir,
    store: String,
}

fn context() -> Context {
    let tmp = TempDir::new("lf-e2e-");
    let store = tmp.join(".agent-captures");
    Context { tmp, store }
}

/// Runs the CLI with `HOME` in the temp dir (no real `~/.config/langfuse/capture.env`)
/// and no real `LANGFUSE_*` value (stripped by `run_cli`), plus `langfuse`.
fn run(ctx: &Context, args: &[&str], langfuse: &[(&str, &str)]) -> CliResult {
    let home = ctx.tmp.path();
    let mut env = vec![("HOME", home.as_str())];
    env.extend_from_slice(langfuse);
    run_cli(args, &home, &env)
}

fn test_keys(host: &str) -> [(&str, &str); 3] {
    [
        ("LANGFUSE_HOST", host),
        ("LANGFUSE_PUBLIC_KEY", TEST_PUBLIC_KEY),
        ("LANGFUSE_SECRET_KEY", TEST_SECRET_KEY),
    ]
}

fn s(value: &str) -> Value {
    Value::from(value)
}

fn implement_session() -> Vec<String> {
    let now = date::to_iso_string(date::now_ms()).expect("now");
    [
        trace_root("implement increment 4", &[("id", s("obs-1")), ("startTime", s(&now))]),
        terminal_call(
            &marker_command(CAPTURE_ID, "implement"),
            &[("id", s("obs-91")), ("startTime", s(&now))],
        ),
        generation(
            "I will read the spec first",
            &[("id", s("obs-2")), ("startTime", s(&now))],
        ),
    ]
    .iter()
    .map(Object::to_json)
    .collect()
}

/// A stored specify-label bundle of `source` with one event, at `<dir>/<id>.jsonl`.
fn seed_stored_bundle(dir: &str, source: BundleSource) -> (String, String) {
    let bundle_path = path::join(&[dir, &format!("{CAPTURE_ID}.jsonl")]);
    let mut label_by_cid = OrderedMap::new();
    label_by_cid.insert("c-specify".to_string(), "specify".to_string());
    let header = compute_bundle_header(CAPTURE_ID, source, &label_by_cid, "2026-10-19T08:00:00.000Z");
    let mut event = Object::new();
    event.insert("capture_id", CAPTURE_ID);
    event.insert("label", "specify");
    event.insert("conversation_id", "c-specify");
    event.insert("seq", 1usize);
    event.insert("ts", "2026-10-19T08:00:00.000Z");
    event.insert("role", "user");
    event.insert("kind", "query");
    event.insert("content", "specify it");
    event.insert("meta", Object::new());
    let content = format!("{}\n{}\n", header.to_json(), event.to_json());
    std::fs::create_dir_all(dir).expect("mkdir");
    std::fs::write(&bundle_path, &content).expect("write bundle");
    (bundle_path, content)
}

fn first_line_object(file: &str) -> Object {
    let text = std::fs::read_to_string(file).expect("read file");
    let value = json::parse(text.split('\n').next().expect("first line")).expect("JSON line");
    value.as_object().expect("object").clone()
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .expect("array")
        .iter()
        .map(|v| v.as_str().expect("string").to_string())
        .collect()
}

fn read(file: &str) -> String {
    std::fs::read_to_string(file).expect("read file")
}

// end-to-end --id --source langfuse (CLI -> client -> loopback v2 endpoint -> sink)

#[test]
fn id_capture_writes_the_langfuse_bundle_and_leaves_the_local_bundle_byte_identical() {
    // Given
    let ctx = context();
    let (local_path, local_content) = seed_stored_bundle(&ctx.store, BundleSource::ClaudeCode);
    let server = start_fake_langfuse(FakeServerOptions {
        observations: implement_session(),
        ..Default::default()
    });
    let langfuse_store = path::join(&[&ctx.store, "langfuse"]);

    // When
    let result = run(
        &ctx,
        &["--id", CAPTURE_ID, "--source", "langfuse", "--out", &langfuse_store],
        &test_keys(&server.url),
    );

    // Then
    let out_path = path::join(&[&langfuse_store, &format!("{CAPTURE_ID}.jsonl")]);
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    assert_eq!(result.stdout, format!("wrote {out_path} (3 events, 1 conversations)\n"));
    let header = first_line_object(&out_path);
    assert_eq!(header.get_str("source"), Some("langfuse"));
    assert_eq!(strings(header.get("labels")), vec!["implement"]);
    assert_eq!(read(&local_path), local_content);
}

#[test]
fn id_capture_without_any_credential_exits_2_naming_the_three_variables() {
    // Given
    let ctx = context();

    // When
    let result = run(
        &ctx,
        &["--id", CAPTURE_ID, "--source", "langfuse", "--out", &ctx.store],
        &[],
    );

    // Then
    assert_eq!(
        (result.exit_code, result.stdout.as_str(), result.stderr.as_str()),
        (2, "", MISSING_CREDENTIALS_ERROR)
    );
}

#[test]
fn id_capture_with_only_the_public_key_exits_2_and_never_prints_the_key() {
    // Given
    let ctx = context();

    // When
    let result = run(
        &ctx,
        &["--id", CAPTURE_ID, "--source", "langfuse", "--out", &ctx.store],
        &[("LANGFUSE_PUBLIC_KEY", TEST_PUBLIC_KEY)],
    );

    // Then
    assert_eq!(result.exit_code, 2);
    assert!(!format!("{}{}", result.stdout, result.stderr).contains(TEST_PUBLIC_KEY));
}

#[test]
fn id_capture_with_keys_only_in_the_langfuse_env_file_reads_langfuse_with_them() {
    // Given
    let ctx = context();
    let server = start_fake_langfuse(FakeServerOptions {
        observations: implement_session(),
        ..Default::default()
    });
    let env_file = ctx.tmp.join("capture.env");
    let lines: Vec<String> = test_keys(&server.url)
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect();
    std::fs::write(&env_file, lines.join("\n")).expect("write env file");

    // When
    let result = run(
        &ctx,
        &[
            "--id",
            CAPTURE_ID,
            "--source",
            "langfuse",
            "--out",
            &ctx.store,
            "--langfuse-env",
            &env_file,
        ],
        &[],
    );

    // Then
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    assert_eq!(
        server.requests().iter().map(|r| r.authorized).collect::<Vec<_>>(),
        vec![true]
    );
}

#[test]
fn id_capture_with_an_api_answering_401_exits_1_and_leaves_the_stored_bundle_untouched() {
    // Given
    let ctx = context();
    let server = start_fake_langfuse(FakeServerOptions {
        fail_with: Some(401),
        ..Default::default()
    });
    let (stored_path, stored_content) = seed_stored_bundle(&ctx.store, BundleSource::Langfuse);

    // When
    let result = run(
        &ctx,
        &["--id", CAPTURE_ID, "--source", "langfuse", "--out", &ctx.store],
        &test_keys(&server.url),
    );

    // Then
    let expected_stderr = format!("error: Langfuse API 401 at {}\n", server.url);
    assert_eq!(
        (result.exit_code, result.stdout.as_str(), result.stderr.as_str()),
        (1, "", expected_stderr.as_str())
    );
    assert_eq!(read(&stored_path), stored_content);
}

/// Example 17: session `hermes-0421`, one trace of 2,350 observations (root + 2,349 answers).
fn large_session() -> Vec<String> {
    let start = date::parse("2026-10-18T09:00:00.000Z");
    let at = |i: usize| date::to_iso_string(start + i as f64 * 1000.0).expect("date");
    let mut all = vec![trace_root(
        "answer 0",
        &[
            ("id", s("obs-0")),
            ("traceId", s("tr-0421")),
            ("sessionId", s("hermes-0421")),
            ("startTime", s(&at(0))),
            ("updatedAt", s(&at(0))),
        ],
    )];
    all.extend((0..2349).map(|k| {
        generation(
            &format!("answer {}", k + 1),
            &[
                ("id", s(&format!("obs-{}", k + 1))),
                ("traceId", s("tr-0421")),
                ("sessionId", s("")),
                ("startTime", s(&at(k + 1))),
                ("updatedAt", s(&at(k + 1))),
            ],
        )
    }));
    all.iter().map(Object::to_json).collect()
}

/// A stored Langfuse session file of `cid` with one event, at `<dir>/sessions/<cid>.jsonl`.
fn seed_stored_session(dir: &str, cid: &str) -> (String, String) {
    let session_path = path::join(&[dir, "sessions", &format!("{cid}.jsonl")]);
    let mut header = Object::new();
    header.insert("type", "session_header");
    header.insert("conversation_id", cid);
    header.insert("source", "langfuse");
    header.insert("last_modified", "2026-10-17T14:02:11.000Z");
    header.insert("extracted_at", "2026-10-18T00:00:00.000Z");
    let mut event = Object::new();
    event.insert("conversation_id", cid);
    event.insert("seq", 1usize);
    event.insert("ts", "2026-10-17T14:00:00.000Z");
    event.insert("role", "user");
    event.insert("kind", "query");
    event.insert("content", "run");
    event.insert("meta", Object::new());
    let content = format!("{}\n{}\n", header.to_json(), event.to_json());
    std::fs::create_dir_all(path::dirname(&session_path)).expect("mkdir");
    std::fs::write(&session_path, &content).expect("write session");
    (session_path, content)
}

// end-to-end --sessions --source langfuse (CLI -> client -> loopback v2 endpoint -> sink)

#[test]
fn sessions_export_of_2350_observations_follows_the_cursor_and_writes_every_event_once_ordered_by_ts() {
    // Given
    let ctx = context();
    let server = start_fake_langfuse(FakeServerOptions {
        observations: large_session(),
        ..Default::default()
    });
    let langfuse_store = path::join(&[&ctx.store, "langfuse"]);

    // When
    let result = run(
        &ctx,
        &[
            "--sessions",
            "--source",
            "langfuse",
            "--conversation",
            "hermes-0421",
            "--out",
            &langfuse_store,
        ],
        &test_keys(&server.url),
    );

    // Then
    let out_path = path::join(&[&langfuse_store, "sessions", "hermes-0421.jsonl"]);
    assert_eq!(result.exit_code, 0, "stderr: {}", result.stderr);
    assert_eq!(
        result.stdout,
        format!("wrote {out_path} (2350 events)\n1 sessions: 1 written, 0 unchanged, 0 untouched\n")
    );
    let lines: Vec<Object> = read(&out_path)
        .trim_end()
        .split('\n')
        .map(|line| {
            json::parse(line)
                .expect("JSON line")
                .as_object()
                .expect("object")
                .clone()
        })
        .collect();
    let (header, events) = lines.split_first().expect("header");
    assert_eq!(header.get_str("conversation_id"), Some("hermes-0421"));
    assert_eq!(header.get_str("source"), Some("langfuse"));
    assert_eq!(header.get_str("last_modified"), Some("2026-10-18T09:39:09.000Z"));
    let seq_content: Vec<(f64, String)> = events
        .iter()
        .map(|e| {
            (
                e.get("seq").and_then(Value::as_f64).expect("seq"),
                e.get_str("content").expect("content").to_string(),
            )
        })
        .collect();
    assert_eq!(
        seq_content,
        (0..2350)
            .map(|i| ((i + 1) as f64, format!("answer {i}")))
            .collect::<Vec<_>>()
    );
    let requests: Vec<(Option<String>, Option<String>, Option<String>)> = server
        .requests()
        .iter()
        .map(|r| {
            (
                r.param("fields").map(str::to_string),
                r.param("traceId").map(str::to_string),
                r.param("cursor").map(str::to_string),
            )
        })
        .collect();
    let some = |v: &str| Some(v.to_string());
    let content = "core,basic,io,metadata,trace_context";
    assert_eq!(
        requests,
        vec![
            (some("core,basic,time"), None, None),
            (some("core,basic,time"), None, some("page-1000")),
            (some("core,basic,time"), None, some("page-2000")),
            (some(content), some("tr-0421"), None),
            (some(content), some("tr-0421"), some("page-1000")),
            (some(content), some("tr-0421"), some("page-2000")),
        ]
    );
}

#[test]
fn sessions_without_any_credential_exits_2_naming_the_three_variables_and_writes_nothing() {
    // Given
    let ctx = context();

    // When
    let result = run(&ctx, &["--sessions", "--source", "langfuse", "--out", &ctx.store], &[]);

    // Then
    assert_eq!(
        (result.exit_code, result.stdout.as_str(), result.stderr.as_str()),
        (2, "", MISSING_CREDENTIALS_ERROR)
    );
    assert!(!std::path::Path::new(&ctx.store).exists());
}

#[test]
fn sessions_with_an_api_answering_401_exits_1_and_leaves_the_stored_file_untouched() {
    // Given
    let ctx = context();
    let server = start_fake_langfuse(FakeServerOptions {
        fail_with: Some(401),
        ..Default::default()
    });
    let (stored_path, stored_content) = seed_stored_session(&ctx.store, "hermes-0419");

    // When
    let result = run(
        &ctx,
        &["--sessions", "--source", "langfuse", "--out", &ctx.store],
        &test_keys(&server.url),
    );

    // Then
    let expected_stderr = format!("error: Langfuse API 401 at {}\n", server.url);
    assert_eq!(
        (result.exit_code, result.stdout.as_str(), result.stderr.as_str()),
        (1, "", expected_stderr.as_str())
    );
    assert_eq!(read(&stored_path), stored_content);
}

// capture.sh store for --source langfuse
//
// The TS test replaced `node_modules/.bin/tsx` with a script printing its
// arguments; the Rust capture.sh runs `AGENT_CAPTURE_BIN` when set (no
// build), so that is the argument-printing stand-in here.

/// A copy of capture.sh in a non-git tree, and a binary that prints the arguments it receives.
fn capture_copy(tmp: &TempDir) -> (String, String) {
    let tool_dir = tmp.join("tool");
    std::fs::create_dir_all(&tool_dir).expect("mkdir");
    let script = path::join(&[&tool_dir, "capture.sh"]);
    std::fs::copy(concat!(env!("CARGO_MANIFEST_DIR"), "/capture.sh"), &script).expect("copy capture.sh");
    let bin = path::join(&[&tool_dir, "print-args"]);
    std::fs::write(&bin, "#!/bin/sh\nprintf \"%s\\n\" \"$@\"\n").expect("write fake binary");
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    (script, bin)
}

fn assert_default_store(capture_args: &[&str], expected_store: &str) {
    // Given — run from a directory outside any git checkout, so the store is ./.agent-captures
    let tmp = TempDir::new("lf-e2e-");
    let (script, bin) = capture_copy(&tmp);
    let project = std::fs::canonicalize(&tmp.0)
        .expect("realpath")
        .to_string_lossy()
        .into_owned();

    // When
    let output = Command::new("/bin/bash")
        .arg(&script)
        .args(capture_args)
        .current_dir(&project)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("HOME", tmp.path())
        .env("AGENT_CAPTURE_BIN", &bin)
        .output()
        .expect("run capture.sh");

    // Then
    let stdout = String::from_utf8_lossy(&output.stdout);
    let args: Vec<&str> = stdout.trim().split('\n').collect();
    let expected = path::join(&[&project, expected_store]);
    assert_eq!(args[args.len().saturating_sub(2)..], ["--out", expected.as_str()]);
}

#[test]
fn capture_sh_id_with_source_langfuse_defaults_to_the_langfuse_sub_store() {
    assert_default_store(
        &["--id", CAPTURE_ID, "--source", "langfuse"],
        ".agent-captures/langfuse",
    );
}

#[test]
fn capture_sh_id_with_source_equals_langfuse_defaults_to_the_langfuse_sub_store() {
    assert_default_store(&["--id", CAPTURE_ID, "--source=langfuse"], ".agent-captures/langfuse");
}

#[test]
fn capture_sh_id_with_source_claude_code_defaults_to_the_store() {
    assert_default_store(&["--id", CAPTURE_ID, "--source", "claude-code"], ".agent-captures");
}

#[test]
fn capture_sh_sessions_with_source_langfuse_defaults_to_the_langfuse_sub_store() {
    assert_default_store(&["--sessions", "--source", "langfuse"], ".agent-captures/langfuse");
}

#[test]
fn capture_sh_id_with_source_oz_defaults_to_the_oz_sub_store() {
    assert_default_store(
        &["--id", CAPTURE_ID, "--source", "oz", "--oz-runs", "run-1"],
        ".agent-captures/oz",
    );
}

#[test]
fn capture_sh_sessions_with_source_equals_oz_defaults_to_the_oz_sub_store() {
    assert_default_store(&["--sessions", "--source=oz", "--oz-dir", "runs"], ".agent-captures/oz");
}

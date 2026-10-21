//! Command line: argument parsing (the contract of Node's `util.parseArgs`,
//! strict mode, whose error messages are part of the CLI output), source
//! dispatch and the two run modes.

use crate::adapters::claude_code_transcript_reader::ClaudeCodeTranscriptReader;
use crate::adapters::hermes_conversation_reader::HermesConversationReader;
use crate::adapters::jsonl_bundle_reader::JsonlBundleReader;
use crate::adapters::jsonl_sink::JsonlSink;
use crate::adapters::langfuse_conversation_reader::LangfuseConversationReader;
use crate::adapters::langfuse_observation_client::LangfuseObservationClient;
use crate::adapters::warp_conversation_reader::WarpConversationReader;
use crate::adapters::warp_sqlite_adapter::WarpSqliteAdapter;
use crate::domain::models::{BundleSource, CaptureBundle, js_display};
use crate::domain::ports::Reader;
use crate::domain::stamps::parse_since_utc;
use crate::format_run_report::{RunReport, format_run_report, format_session_report};
use crate::usecases::extract_bundle::{ExtractOptions, extract_bundle};
use crate::usecases::extract_sessions::{ExtractSessionsOptions, extract_sessions};

/// Parsed command line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliArgs {
    pub id: Option<String>,
    pub no_merge: bool,
    pub out: Option<String>,
    pub db_path: Option<String>,
    pub hermes_db_path: Option<String>,
    pub source: Option<String>,
    pub claude_root: Option<String>,
    pub langfuse_env: Option<String>,
    pub sessions: bool,
    pub since: Option<String>,
    pub conversation: Option<String>,
    pub help: bool,
}

/// The outcome of a CLI run: lines to print and the exit code.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    pub stdout: Vec<String>,
    pub stderr: Vec<String>,
    pub exit_code: i32,
}

impl Outcome {
    fn from_report(report: RunReport) -> Self {
        Self {
            stdout: report.stdout,
            stderr: report.stderr,
            exit_code: report.exit_code,
        }
    }

    fn usage_error(lines: Vec<String>) -> Self {
        let mut stderr = lines;
        stderr.push(USAGE.to_string());
        Self {
            stdout: vec![],
            stderr,
            exit_code: 2,
        }
    }

    fn error(lines: Vec<String>, exit_code: i32) -> Self {
        Self {
            stdout: vec![],
            stderr: lines,
            exit_code,
        }
    }
}

const USAGE: &str = "usage: capture.sh --id <capture-id> --source warp|claude-code|hermes|langfuse [--no-merge] [--out dir] [--db-path path] [--claude-root dir] [--hermes-db-path path] [--langfuse-env file]
       capture.sh --sessions --source warp|claude-code|hermes|langfuse [--since <ISO-8601>] [--conversation <cid>] [--no-merge] [--out dir] [--db-path path] [--claude-root dir] [--hermes-db-path path] [--langfuse-env file]";

const SOURCE_REQUIRED: &str =
    "error: --source is required for extraction (\"warp\", \"claude-code\", \"hermes\", or \"langfuse\")";

const STRING_OPTIONS: [&str; 9] = [
    "id",
    "out",
    "db-path",
    "hermes-db-path",
    "source",
    "claude-root",
    "langfuse-env",
    "since",
    "conversation",
];
const BOOLEAN_OPTIONS: [&str; 3] = ["no-merge", "sessions", "help"];

fn option_display(name: &str) -> String {
    if name == "help" {
        "-h, --help".to_string()
    } else {
        format!("--{name}")
    }
}

fn set_string(args: &mut CliArgs, name: &str, value: String) {
    let slot = match name {
        "id" => &mut args.id,
        "out" => &mut args.out,
        "db-path" => &mut args.db_path,
        "hermes-db-path" => &mut args.hermes_db_path,
        "source" => &mut args.source,
        "claude-root" => &mut args.claude_root,
        "langfuse-env" => &mut args.langfuse_env,
        "since" => &mut args.since,
        _ => &mut args.conversation,
    };
    *slot = Some(value);
}

fn set_boolean(args: &mut CliArgs, name: &str) {
    match name {
        "no-merge" => args.no_merge = true,
        "sessions" => args.sessions = true,
        _ => args.help = true,
    }
}

/// `parseArgs({ strict: true })` with this tool's options; the error is
/// Node's message.
pub fn parse_args(argv: &[String]) -> Result<CliArgs, String> {
    let mut args = CliArgs::default();
    let mut i = 0;
    while i < argv.len() {
        let arg = &argv[i];
        if arg == "--" {
            if let Some(next) = argv.get(i + 1) {
                return Err(format!(
                    "Unexpected argument '{next}'. This command does not take positional arguments"
                ));
            }
            break;
        }
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline_value) = match long.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (long, None),
            };
            if BOOLEAN_OPTIONS.contains(&name) {
                if inline_value.is_some() {
                    return Err(format!("Option '{}' does not take an argument", option_display(name)));
                }
                set_boolean(&mut args, name);
            } else if STRING_OPTIONS.contains(&name) {
                let value = match inline_value {
                    Some(v) => v,
                    None => {
                        let Some(next) = argv.get(i + 1) else {
                            return Err(format!("Option '--{name} <value>' argument missing"));
                        };
                        if next.len() > 1 && next.starts_with('-') {
                            return Err(format!(
                                "Option '--{name}' argument is ambiguous.\nDid you forget to specify the option argument for '--{name}'?\nTo specify an option argument starting with a dash use '--{name}=-XYZ'."
                            ));
                        }
                        i += 1;
                        next.clone()
                    }
                };
                set_string(&mut args, name, value);
            } else {
                return Err(format!("Unknown option '--{name}'"));
            }
        } else if arg.len() > 1 && arg.starts_with('-') {
            for c in arg[1..].chars() {
                if c == 'h' {
                    args.help = true;
                } else {
                    return Err(format!("Unknown option '-{c}'"));
                }
            }
        } else {
            return Err(format!(
                "Unexpected argument '{arg}'. This command does not take positional arguments"
            ));
        }
        i += 1;
    }
    Ok(args)
}

/// The id becomes a filename: refuse anything that could escape the output directory.
pub fn is_safe_capture_id(id: &str) -> bool {
    let bytes = id.as_bytes();
    if bytes.is_empty() || crate::js::string::utf16_len(id) > 200 || !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&c| c.is_ascii_alphanumeric() || c == b'.' || c == b'_' || c == b'-')
}

fn reader_for(args: &CliArgs, source: BundleSource) -> Box<dyn Reader> {
    match source {
        BundleSource::ClaudeCode => Box::new(ClaudeCodeTranscriptReader::new(args.claude_root.clone())),
        BundleSource::Hermes => Box::new(HermesConversationReader::new(args.hermes_db_path.clone())),
        BundleSource::Langfuse => Box::new(LangfuseConversationReader::new(LangfuseObservationClient::new(
            args.langfuse_env.clone(),
        ))),
        BundleSource::Warp => Box::new(WarpConversationReader::new(WarpSqliteAdapter::new(
            args.db_path.clone(),
        ))),
    }
}

/// Load a previously-captured bundle for the decay-safe merge. Guards: a
/// corrupt prior file or a capture_id/source mismatch fails loudly instead of
/// silently overwriting (default merge only; --no-merge skips loading).
fn load_existing_bundle(
    capture_id: &str,
    out_dir: &str,
    no_merge: bool,
    source: BundleSource,
) -> Result<(Option<CaptureBundle>, bool), Outcome> {
    let reader = JsonlBundleReader::new(out_dir);
    if !reader.exists(capture_id) {
        return Ok((None, false));
    }
    if no_merge {
        return Ok((None, true));
    }
    let loaded = match reader.load(capture_id) {
        Ok(loaded) => loaded,
        Err(message) => {
            return Err(Outcome::error(
                vec![
                    format!("error: {message}"),
                    format!(
                        "refusing to overwrite \"{}\" — fix or remove the corrupt bundle, or re-run with --no-merge to replace it.",
                        reader.path_for(capture_id)
                    ),
                ],
                1,
            ));
        }
    };
    if let Some(bundle) = &loaded {
        let stored_id = bundle.header.get("capture_id");
        if stored_id.and_then(|v| v.as_str()) != Some(capture_id) {
            return Err(Outcome::error(
                vec![
                    format!(
                        "error: existing bundle capture_id mismatch: file has \"{}\", run expects \"{capture_id}\"",
                        js_display(stored_id)
                    ),
                    format!(
                        "refusing to overwrite \"{}\" — this looks like a collision or manual tampering.",
                        reader.path_for(capture_id)
                    ),
                ],
                1,
            ));
        }
        let stored_source = bundle.header.get("source");
        if stored_source.and_then(|v| v.as_str()) != Some(source.as_str()) {
            return Err(Outcome::error(
                vec![
                    format!(
                        "error: existing bundle source mismatch: file has \"{}\", run requested \"{}\"",
                        js_display(stored_source),
                        source.as_str()
                    ),
                    format!(
                        "refusing to merge \"{}\" across sources — re-run with --no-merge to replace it.",
                        reader.path_for(capture_id)
                    ),
                ],
                1,
            ));
        }
    }
    Ok((loaded, false))
}

fn parse_source(value: &str) -> Result<BundleSource, Outcome> {
    BundleSource::parse(value).ok_or_else(|| {
        Outcome::usage_error(vec![format!(
            "error: --source must be \"warp\", \"claude-code\", \"hermes\", or \"langfuse\" (got \"{value}\")"
        )])
    })
}

/// Run the CLI on `argv` (without the program name).
pub fn run(argv: &[String]) -> Outcome {
    let args = match parse_args(argv) {
        Ok(args) => args,
        Err(message) => return Outcome::usage_error(vec![format!("error: {message}")]),
    };
    if args.help {
        return Outcome {
            stdout: vec![],
            stderr: vec![USAGE.to_string()],
            exit_code: 0,
        };
    }
    let source = match args.source.as_deref().map(parse_source).transpose() {
        Ok(source) => source,
        Err(outcome) => return outcome,
    };

    if !args.sessions && (args.since.is_some() || args.conversation.is_some()) {
        return Outcome::usage_error(vec![
            "error: --since and --conversation are only valid together with --sessions".to_string(),
        ]);
    }
    if args.sessions && args.id.is_some() {
        return Outcome::usage_error(vec![
            "error: --id cannot be combined with --sessions — session mode exports every conversation, no capture id"
                .to_string(),
        ]);
    }

    if args.sessions {
        let Some(source) = source else {
            return Outcome::usage_error(vec![SOURCE_REQUIRED.to_string()]);
        };
        if let Some(since) = &args.since
            && parse_since_utc(since).is_none()
        {
            return Outcome::usage_error(vec![format!(
                "error: --since \"{since}\" is not a parseable ISO-8601 date or datetime"
            )]);
        }
        return run_sessions_mode(&args, source);
    }

    let id = match &args.id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => return Outcome::usage_error(vec!["error: --id <capture-id> is required".to_string()]),
    };
    if !is_safe_capture_id(&id) {
        return Outcome::error(
            vec![format!(
                "error: --id \"{id}\" is not allowed: use letters, digits, \".\", \"_\" or \"-\" (it names the output file), starting with a letter or digit"
            )],
            2,
        );
    }
    let Some(source) = source else {
        return Outcome::usage_error(vec![SOURCE_REQUIRED.to_string()]);
    };

    let out_dir = args.out.clone().unwrap_or_else(|| "out".to_string());
    let (existing_bundle, replaced_existing) = match load_existing_bundle(&id, &out_dir, args.no_merge, source) {
        Ok(loaded) => loaded,
        Err(outcome) => return outcome,
    };

    let reader = reader_for(&args, source);
    let result = match extract_bundle(
        reader.as_ref(),
        &id,
        ExtractOptions {
            no_merge: args.no_merge,
            existing_bundle,
        },
    ) {
        Ok(result) => result,
        Err(error) => return Outcome::error(vec![format!("error: {}", error.message())], error.exit_code()),
    };

    // Never write a header-only file: zero bound conversations means nothing to persist.
    let mut out_path = None;
    if result.summary.conversations > 0 {
        match JsonlSink::new(&out_dir).write(&result.bundle) {
            Ok(path) => out_path = Some(path),
            Err(error) => return Outcome::error(vec![format!("error: {error}")], 1),
        }
    }
    let mut report = format_run_report(&result.summary, out_path.as_deref());
    if replaced_existing && let Some(path) = &out_path {
        report
            .stderr
            .insert(0, format!("replacing existing bundle (--no-merge): {path}"));
    }
    Outcome::from_report(report)
}

/// The `--sessions` run: exit 2 for a source configuration error (missing
/// Langfuse credentials), 1 for a read failure or zero sessions, 0 otherwise.
fn run_sessions_mode(args: &CliArgs, source: BundleSource) -> Outcome {
    let reader = reader_for(args, source);
    let summary = extract_sessions(
        reader.as_ref(),
        ExtractSessionsOptions {
            out_dir: args.out.clone().unwrap_or_else(|| "out".to_string()),
            source,
            since: args.since.clone(),
            conversation: args.conversation.clone(),
            no_merge: args.no_merge,
        },
    );
    match summary {
        Ok(summary) => Outcome::from_report(format_session_report(&summary)),
        Err(error) => Outcome::error(vec![format!("error: {}", error.message())], error.exit_code()),
    }
}

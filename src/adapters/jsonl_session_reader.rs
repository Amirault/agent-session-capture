//! Scans and loads stored `sessions/*.jsonl` files (session mode).

use std::fs;
use std::io::Read;

use super::jsonl_bundle_reader::{content_lines, node_io_message, read_text};
use super::session_file_name::session_file_name;
use crate::domain::models::{BundleSource, SessionFile, SessionHeader, js_display};
use crate::domain::ordered_map::OrderedMap;
use crate::js::string::{js_cmp, js_trim};
use crate::js::{Object, Value, json, path};

/// One stored session file discovered by the scan.
#[derive(Clone, Debug)]
pub struct StoredSessionEntry {
    /// File basename without the `.jsonl` extension.
    pub file_name: String,
    /// Full path of the file.
    pub path: String,
    pub header: SessionHeader,
}

/// Enough bytes for any well-formed session_header line, then some.
const FIRST_LINE_BUDGET: usize = 64 * 1024;

/// Read only a file's first physical line; `None` for an empty file.
fn read_first_line(file: &str) -> Result<Option<String>, String> {
    let mut handle = fs::File::open(file).map_err(|e| node_io_message(&e, "open", file))?;
    let mut chunk = vec![0u8; FIRST_LINE_BUDGET];
    let mut read = 0;
    while read < chunk.len() {
        match handle.read(&mut chunk[read..]) {
            Ok(0) => break,
            Ok(n) => read += n,
            Err(e) => return Err(node_io_message(&e, "read", file)),
        }
    }
    if read == 0 {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&chunk[..read]).into_owned();
    Ok(Some(match text.find('\n') {
        Some(i) => text[..i].to_string(),
        None => text,
    }))
}

fn parse_header_line(line: Option<&str>, file: &str) -> Result<SessionHeader, String> {
    let line = match line {
        Some(l) if !js_trim(l).is_empty() => l,
        _ => return Err(format!("corrupt session file \"{file}\": empty file")),
    };
    let parsed =
        json::parse(line).map_err(|_| format!("corrupt session file \"{file}\": header line is not valid JSON"))?;
    let not_header = || format!("corrupt session file \"{file}\": first line is not a session_header");
    let Value::Object(raw) = parsed else {
        return Err(not_header());
    };
    let conversation_id = match raw.get("conversation_id") {
        Some(Value::String(s)) => s.clone(),
        _ => return Err(not_header()),
    };
    let last_modified = match raw.get("last_modified") {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Null) => None,
        _ => return Err(not_header()),
    };
    if raw.get_str("type") != Some("session_header") {
        return Err(not_header());
    }
    Ok(SessionHeader {
        conversation_id,
        source: raw.get("source").cloned().unwrap_or_default(),
        last_modified,
        raw,
    })
}

fn guard(file_name: &str, file: &str) -> Result<SessionHeader, String> {
    let header = parse_header_line(read_first_line(file)?.as_deref(), file)?;
    if session_file_name(&header.conversation_id) != file_name {
        return Err(format!(
            "session file conversation mismatch: file \"{file_name}.jsonl\" holds conversation \"{}\" — this looks like a collision or manual tampering",
            header.conversation_id
        ));
    }
    Ok(header)
}

/// The result of [`scan_stored_sessions`].
#[derive(Default)]
pub struct StoredSessions {
    pub by_cid: OrderedMap<String, StoredSessionEntry>,
    pub other_sources: OrderedMap<String, StoredSessionEntry>,
    pub warnings: Vec<String>,
}

fn source_matches(header: &SessionHeader, source: BundleSource) -> bool {
    header.source.as_str() == Some(source.as_str())
}

/// Scan `<outDir>/sessions/*.jsonl`, reading each file's FIRST LINE only.
///
/// A corrupt file or a header-vs-filename conversation mismatch fails the run
/// loudly — unless `no_merge`, where each violation becomes a warning and the
/// entry is skipped. A file of another source is set aside in
/// `other_sources`: it only conflicts if this run gathers its conversation.
pub fn scan_stored_sessions(
    sessions_dir: &str,
    source: BundleSource,
    no_merge: bool,
) -> Result<StoredSessions, String> {
    let mut result = StoredSessions::default();
    let Ok(entries) = fs::read_dir(sessions_dir) else {
        return Ok(result);
    };
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".jsonl"))
        .collect();
    names.sort_by(|a, b| js_cmp(a, b));

    for name in names {
        let file = path::join(&[sessions_dir, &name]);
        let file_name = name[..name.len() - ".jsonl".len()].to_string();
        let header = match guard(&file_name, &file) {
            Ok(header) => header,
            Err(error) => {
                if !no_merge {
                    return Err(format!(
                        "{error} — refusing to continue; fix or remove the file, or re-run with --no-merge to replace it"
                    ));
                }
                result
                    .warnings
                    .push(format!("replacing stored session (--no-merge): {error}"));
                continue;
            }
        };
        let cid = header.conversation_id.clone();
        let entry = StoredSessionEntry {
            file_name: file_name.clone(),
            path: file,
            header,
        };
        if !source_matches(&entry.header, source) {
            result.other_sources.insert(cid, entry);
            continue;
        }
        if let Some(previous) = result.by_cid.get(&cid) {
            let error = format!(
                "duplicate session files for conversation \"{cid}\": \"{}.jsonl\" and \"{file_name}.jsonl\"",
                previous.file_name
            );
            if !no_merge {
                return Err(format!(
                    "{error} — refusing to continue; fix or remove one, or re-run with --no-merge"
                ));
            }
            result
                .warnings
                .push(format!("replacing stored session (--no-merge): {error}"));
            continue;
        }
        result.by_cid.insert(cid, entry);
    }
    Ok(result)
}

/// The conflict of a run about to write a conversation whose stored file
/// belongs to another source.
pub fn source_mismatch(entry: &StoredSessionEntry, source: BundleSource) -> String {
    format!(
        "session file source mismatch: file \"{}\" has source \"{}\", run requested \"{}\" — refusing to merge across sources",
        entry.path,
        js_display(Some(&entry.header.source)),
        source.as_str()
    )
}

/// Fully load one stored session file (header + events) for the decay-safe
/// merge. Fails on any unparseable line.
pub fn load_stored_session(file: &str) -> Result<SessionFile, String> {
    let raw = read_text(file)?;
    let lines = content_lines(&raw);
    if lines.is_empty() {
        return Err(format!("corrupt session file \"{file}\": empty file"));
    }
    let header = parse_header_line(Some(lines[0]), file)?;
    let mut events = Vec::new();
    for (i, line) in lines.iter().enumerate().skip(1) {
        if line.is_empty() {
            continue;
        }
        let event = json::parse(line)
            .map_err(|_| format!("corrupt session file \"{file}\": line {} is not valid JSON", i + 1))?;
        events.push(match event {
            Value::Object(o) => o,
            _ => Object::new(),
        });
    }
    Ok(SessionFile { header, events })
}

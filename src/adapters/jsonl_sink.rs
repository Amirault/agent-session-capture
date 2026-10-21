//! Strict NDJSON (JSONL) sink: one compact JSON object per line.

use std::fs;
use std::io;

use super::session_file_name::session_file_name;
use crate::domain::models::{CaptureBundle, EventRecord, SessionHeader};
use crate::js::{date, path};

/// Line 1 is the header; every subsequent line is one time-ordered event.
/// `JSON.stringify` escapes newlines inside values, so each object is one
/// physical line. The file ends with a trailing newline.
pub struct JsonlSink {
    out_dir: String,
}

fn render(header: &crate::js::Object, events: &[EventRecord]) -> String {
    let mut content = header.to_json();
    content.push('\n');
    for event in events {
        content.push_str(&event.to_json());
        content.push('\n');
    }
    content
}

/// Atomic write: stage to a temp file in the same directory, then rename, so
/// a torn write never leaves a half-written file at the target path.
fn write_atomically(out_path: &str, content: &str) -> io::Result<()> {
    let tmp_path = format!("{out_path}.{}.{}.tmp", std::process::id(), date::now_ms());
    fs::write(&tmp_path, content)?;
    fs::rename(&tmp_path, out_path)
}

impl JsonlSink {
    pub fn new(out_dir: &str) -> Self {
        Self {
            out_dir: out_dir.to_string(),
        }
    }

    /// Write a bundle to `<outDir>/<capture_id>.jsonl`; returns the path.
    pub fn write(&self, bundle: &CaptureBundle) -> io::Result<String> {
        fs::create_dir_all(&self.out_dir)?;
        let capture_id = crate::domain::models::js_display(bundle.header.get("capture_id"));
        let out_path = path::join(&[&self.out_dir, &format!("{capture_id}.jsonl")]);
        write_atomically(&out_path, &render(&bundle.header, &bundle.events))?;
        Ok(out_path)
    }

    /// Write one exported session to `<outDir>/sessions/<file>.jsonl`; the
    /// subdirectory keeps marker bundles and session exports disjoint.
    pub fn write_session(&self, header: &SessionHeader, events: &[EventRecord]) -> io::Result<String> {
        let sessions_dir = path::join(&[&self.out_dir, "sessions"]);
        fs::create_dir_all(&sessions_dir)?;
        let out_path = path::join(&[
            &sessions_dir,
            &format!("{}.jsonl", session_file_name(&header.conversation_id)),
        ]);
        write_atomically(&out_path, &render(&header.raw, events))?;
        Ok(out_path)
    }
}

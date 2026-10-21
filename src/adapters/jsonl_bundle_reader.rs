//! Reads a previously-captured JSONL bundle back from disk — the mirror of
//! the sink.

use std::fs;

use crate::domain::models::CaptureBundle;
use crate::js::{Object, Value, json, path};

pub struct JsonlBundleReader {
    out_dir: String,
}

/// Read a file as UTF-8 (invalid sequences replaced), as `readFileSync(p, "utf8")`.
pub fn read_text(file: &str) -> Result<String, String> {
    fs::read(file)
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .map_err(|e| node_io_message(&e, "open", file))
}

/// Node's `ENOENT: no such file or directory, open '<path>'` style message.
pub fn node_io_message(error: &std::io::Error, syscall: &str, file: &str) -> String {
    let (code, text) = match error.raw_os_error() {
        Some(2) => ("ENOENT", "no such file or directory"),
        Some(13) => ("EACCES", "permission denied"),
        Some(21) => ("EISDIR", "illegal operation on a directory"),
        Some(20) => ("ENOTDIR", "not a directory"),
        _ => return error.to_string(),
    };
    if code == "EISDIR" {
        return format!("{code}: {text}, read");
    }
    format!("{code}: {text}, {syscall} '{file}'")
}

/// Split file content into lines, dropping the empty element a trailing
/// newline leaves.
pub fn content_lines(raw: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = raw.split('\n').collect();
    if lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

impl JsonlBundleReader {
    pub fn new(out_dir: &str) -> Self {
        Self {
            out_dir: out_dir.to_string(),
        }
    }

    pub fn path_for(&self, capture_id: &str) -> String {
        path::join(&[&self.out_dir, &format!("{capture_id}.jsonl")])
    }

    pub fn exists(&self, capture_id: &str) -> bool {
        fs::metadata(self.path_for(capture_id)).is_ok()
    }

    /// Load the bundle; `Ok(None)` when no file exists, `Err` when it exists
    /// but is unparseable (corrupt/truncated).
    pub fn load(&self, capture_id: &str) -> Result<Option<CaptureBundle>, String> {
        let out_path = self.path_for(capture_id);
        if fs::metadata(&out_path).is_err() {
            return Ok(None);
        }
        let raw = read_text(&out_path)?;
        let lines = content_lines(&raw);
        if lines.is_empty() {
            return Err(format!("corrupt bundle \"{out_path}\": empty file"));
        }
        let header = json::parse(lines[0])
            .map_err(|_| format!("corrupt bundle \"{out_path}\": header line is not valid JSON"))?;
        let header = match header {
            Value::Null => return Err("Cannot read properties of null (reading 'type')".to_string()),
            Value::Object(h) if h.get_str("type") == Some("bundle_header") => h,
            _ => {
                return Err(format!(
                    "corrupt bundle \"{out_path}\": first line is not a bundle_header"
                ));
            }
        };
        let mut events = Vec::new();
        for (i, line) in lines.iter().enumerate().skip(1) {
            if line.is_empty() {
                continue;
            }
            let event = json::parse(line)
                .map_err(|_| format!("corrupt bundle \"{out_path}\": line {} is not valid JSON", i + 1))?;
            events.push(match event {
                Value::Object(o) => o,
                _ => Object::new(),
            });
        }
        Ok(Some(CaptureBundle { header, events }))
    }
}

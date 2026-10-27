//! Where headless Warp `oz agent run` sessions come from: Warp's server,
//! through the `oz` CLI, or a directory of runs saved from it.
//!
//! A run is the JSON of `oz run get <run id> --output-format json`; its
//! conversation the JSON of `oz run conversation get <conversation id>
//! --output-format json`. A saved run is one `*.json` file holding
//! `{"run": …, "conversation": …}`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Command;

use crate::domain::source_errors::ReadError;
use crate::js::{Object, Value, json};

/// What the oz reader needs: the runs to read, and each run's conversation.
pub trait OzRunStore {
    fn runs(&self) -> Result<Vec<Object>, ReadError>;
    fn conversation(&self, run: &Object) -> Result<Object, ReadError>;
}

/// One `oz` invocation: its standard output, or why it failed.
pub trait OzCommand {
    fn output(&self, args: &[&str]) -> Result<String, ReadError>;
}

/// The `oz` executable on `PATH`.
pub struct OzProcess;

impl OzCommand for OzProcess {
    fn output(&self, args: &[&str]) -> Result<String, ReadError> {
        let action = format!("oz {}", args[..args.len().min(3)].join(" "));
        let output = Command::new("oz")
            .args(args)
            .output()
            .map_err(|e| ReadError::SourceAccess(format!("{action} failed: {e} (is Warp's oz CLI on PATH?)")))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let reason = stderr
                .lines()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no error output");
            return Err(ReadError::SourceAccess(format!(
                "{action} failed ({}): {reason}",
                output.status
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The runs named on the command line (`--oz-runs`) and saved in a directory
/// (`--oz-dir`).
pub struct OzRuns<C: OzCommand> {
    run_ids: Vec<String>,
    dir: Option<String>,
    command: C,
    /// Saved conversations by run id.
    saved: RefCell<HashMap<String, Object>>,
}

impl OzRuns<OzProcess> {
    pub fn new(run_ids: Vec<String>, dir: Option<String>) -> Self {
        Self::with_command(run_ids, dir, OzProcess)
    }
}

impl<C: OzCommand> OzRuns<C> {
    pub fn with_command(run_ids: Vec<String>, dir: Option<String>, command: C) -> Self {
        Self {
            run_ids,
            dir,
            command,
            saved: RefCell::new(HashMap::new()),
        }
    }

    fn get_json(&self, args: &[&str]) -> Result<Object, ReadError> {
        let action = format!("oz {}", args[..3].join(" "));
        let text = self.command.output(args)?;
        match json::parse(&text) {
            Ok(Value::Object(object)) => Ok(object),
            Ok(_) => Err(ReadError::SourceAccess(format!("{action}: expected a JSON object"))),
            Err(e) => Err(ReadError::SourceAccess(format!("{action}: {e}"))),
        }
    }

    fn saved_runs(&self, dir: &str) -> Result<Vec<Object>, ReadError> {
        let entries = std::fs::read_dir(dir).map_err(|e| ReadError::Other(format!("--oz-dir \"{dir}\": {e}")))?;
        let mut paths: Vec<_> = entries
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        let mut runs = Vec::new();
        for path in paths {
            let shown = path.display();
            let text = std::fs::read_to_string(&path).map_err(|e| ReadError::Other(format!("{shown}: {e}")))?;
            let saved = match json::parse(&text) {
                Ok(Value::Object(object)) => object,
                Ok(_) => return Err(ReadError::Other(format!("{shown}: expected a JSON object"))),
                Err(e) => return Err(ReadError::Other(format!("{shown}: {e}"))),
            };
            let (Some(Value::Object(run)), Some(Value::Object(conversation))) =
                (saved.get("run"), saved.get("conversation"))
            else {
                return Err(ReadError::Other(format!(
                    "{shown}: expected {{\"run\": <oz run get>, \"conversation\": <oz run conversation get>}}"
                )));
            };
            let run_id = run.get_str("run_id").unwrap_or_default().to_string();
            self.saved.borrow_mut().insert(run_id, conversation.clone());
            runs.push(run.clone());
        }
        Ok(runs)
    }
}

/// An id passed to `oz` must not read as one of its flags.
fn checked_id<'a>(what: &str, id: &'a str) -> Result<&'a str, ReadError> {
    if id.is_empty() || id.starts_with('-') {
        return Err(ReadError::SourceConfiguration(format!("invalid oz {what} \"{id}\"")));
    }
    Ok(id)
}

impl<C: OzCommand> OzRunStore for OzRuns<C> {
    fn runs(&self) -> Result<Vec<Object>, ReadError> {
        if self.run_ids.is_empty() && self.dir.is_none() {
            return Err(ReadError::SourceConfiguration(
                "--source oz needs --oz-runs <run-id,…> or --oz-dir <dir>".to_string(),
            ));
        }
        let mut runs = match &self.dir {
            Some(dir) => self.saved_runs(dir)?,
            None => Vec::new(),
        };
        for run_id in &self.run_ids {
            let run_id = checked_id("run id", run_id)?;
            runs.push(self.get_json(&["run", "get", run_id, "--output-format", "json"])?);
        }
        Ok(runs)
    }

    fn conversation(&self, run: &Object) -> Result<Object, ReadError> {
        if let Some(saved) = run
            .get_str("run_id")
            .and_then(|id| self.saved.borrow().get(id).cloned())
        {
            return Ok(saved);
        }
        let cid = checked_id("conversation id", run.get_str("conversation_id").unwrap_or_default())?;
        self.get_json(&["run", "conversation", "get", cid, "--output-format", "json"])
    }
}

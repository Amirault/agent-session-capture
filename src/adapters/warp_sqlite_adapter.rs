//! Warp's local SQLite DB: live DB discovery and a read-only `VACUUM INTO`
//! snapshot.

use std::fs;
use std::time::Duration;

use super::sqlite_readable_db::{ReadableDb, sqlite_message};
use crate::domain::ports::ConversationSource;
use crate::domain::source_errors::ReadError;
use crate::js::{date, path};

/// Known relative locations of the Warp SQLite DB (macOS).
const PROBE_REL_PATHS: [&str; 1] =
    ["Library/Group Containers/2BBY89MBSN.dev.warp/Library/Application Support/dev.warp.Warp-Stable/warp.sqlite"];

fn not_found(probed: &[String]) -> ReadError {
    let lines: Vec<String> = probed.iter().map(|p| format!("  - {p}")).collect();
    ReadError::Other(format!("warp.sqlite not found. Probed:\n{}", lines.join("\n")))
}

/// Safety: the live DB is opened read-only and never written. Work runs
/// against a `VACUUM INTO` snapshot (committed WAL pages included) that is
/// itself opened read-only and deleted once the run finishes.
pub struct WarpSqliteAdapter {
    live_db_path: Option<String>,
}

impl WarpSqliteAdapter {
    pub fn new(live_db_path: Option<String>) -> Self {
        Self { live_db_path }
    }

    fn create_snapshot(live_path: &str, snap_path: &str) -> Result<(), ReadError> {
        let live = ReadableDb::open_read_only(live_path).map_err(ReadError::Other)?;
        live.connection().busy_timeout(Duration::from_millis(5000))?;
        let escaped = snap_path.replace('\'', "''");
        live.connection()
            .execute_batch(&format!("VACUUM INTO '{escaped}'"))
            .map_err(|e| ReadError::Other(sqlite_message(&e)))
    }
}

/// A fresh private directory under the system temp dir (`mkdtemp`).
fn make_temp_dir(prefix: &str) -> Result<String, ReadError> {
    let base = std::env::temp_dir();
    for attempt in 0..100u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos();
        let dir = base.join(format!("{prefix}{:x}{:x}{attempt}", std::process::id(), nanos));
        if fs::create_dir(&dir).is_ok() {
            return Ok(dir.to_string_lossy().into_owned());
        }
    }
    Err(ReadError::Other("could not create a snapshot directory".to_string()))
}

impl ConversationSource for WarpSqliteAdapter {
    fn resolve_live_db_path(&self) -> Result<String, ReadError> {
        if let Some(override_path) = self.live_db_path.as_deref().filter(|p| !p.is_empty()) {
            if fs::metadata(override_path).is_err() {
                return Err(not_found(&[override_path.to_string()]));
            }
            return Ok(override_path.to_string());
        }
        let home = path::home_dir();
        let probed: Vec<String> = PROBE_REL_PATHS.iter().map(|p| path::join(&[&home, p])).collect();
        probed
            .iter()
            .find(|p| fs::metadata(p).is_ok())
            .cloned()
            .ok_or_else(|| not_found(&probed))
    }

    fn with_snapshot<T>(&self, f: impl FnOnce(&ReadableDb) -> Result<T, ReadError>) -> Result<T, ReadError> {
        let live_path = self.resolve_live_db_path()?;
        let snap_dir = make_temp_dir("warp-snap-")?;
        let snap_path = path::join(&[&snap_dir, &format!("snap-{}-{}.db", std::process::id(), date::now_ms())]);
        let result = (|| {
            Self::create_snapshot(&live_path, &snap_path)?;
            if fs::metadata(&snap_path).is_err() {
                return Err(ReadError::Other(format!(
                    "VACUUM INTO did not produce a snapshot at {snap_path}"
                )));
            }
            let snap = ReadableDb::open_read_only(&snap_path).map_err(ReadError::Other)?;
            f(&snap)
        })();
        let _ = fs::remove_file(&snap_path);
        let _ = fs::remove_dir_all(&snap_dir);
        result
    }
}

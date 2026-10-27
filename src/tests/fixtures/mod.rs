//! Test fixtures.

#![allow(dead_code)]

pub mod fake_langfuse_server;
pub mod fake_source;
pub mod fixture_db;
pub mod langfuse_observations;
pub mod oz_runs;
pub mod protobuf;
pub mod test_proto_schema;

/// A fresh temporary directory, removed when dropped.
pub struct TempDir(pub std::path::PathBuf);

impl TempDir {
    pub fn new(prefix: &str) -> Self {
        let base = std::env::temp_dir();
        for attempt in 0..1000u32 {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos();
            let dir = base.join(format!("{prefix}{}-{nanos}-{attempt}", std::process::id()));
            if std::fs::create_dir(&dir).is_ok() {
                return Self(dir);
            }
        }
        panic!("could not create a temp dir");
    }

    pub fn path(&self) -> String {
        self.0.to_string_lossy().into_owned()
    }

    pub fn join(&self, rel: &str) -> String {
        self.0.join(rel).to_string_lossy().into_owned()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

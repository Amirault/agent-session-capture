//! A ConversationSource that runs directly against an already-open fixture DB
//! (no VACUUM INTO copy).

use crate::adapters::sqlite_readable_db::ReadableDb;
use crate::domain::ports::ConversationSource;
use crate::domain::source_errors::ReadError;

pub struct FakeSource {
    db: ReadableDb,
}

/// Wrap a fixture DB file (opened read-write so tests can keep seeding).
pub fn fake_source(path: &str) -> FakeSource {
    FakeSource { db: ReadableDb::new(rusqlite::Connection::open(path).expect("open fixture")) }
}

impl ConversationSource for FakeSource {
    fn with_snapshot<T>(&self, f: impl FnOnce(&ReadableDb) -> Result<T, ReadError>) -> Result<T, ReadError> {
        f(&self.db)
    }

    fn resolve_live_db_path(&self) -> Result<String, ReadError> {
        Ok("/fake/warp.sqlite".to_string())
    }
}

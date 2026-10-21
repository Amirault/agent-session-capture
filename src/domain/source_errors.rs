//! Errors a reader raises, with the exit-code semantics of the CLI.

use std::fmt;

/// Why a source read failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadError {
    /// The source could not be read at all (access refused, server error,
    /// unreachable). A stored bundle must not mask it: the use cases rethrow it
    /// instead of degrading to stored-only, so nothing is rewritten and the CLI
    /// exits 1. The message never carries a secret.
    SourceAccess(String),
    /// The source is not configured (e.g. credentials missing): a usage error,
    /// the CLI exits 2.
    SourceConfiguration(String),
    /// Any other failure (missing store, unreadable file, schema mismatch).
    Other(String),
}

impl ReadError {
    pub fn message(&self) -> &str {
        match self {
            ReadError::SourceAccess(m) | ReadError::SourceConfiguration(m) | ReadError::Other(m) => m,
        }
    }

    /// `instanceof SourceAccessError` (configuration errors included).
    pub fn is_source_access(&self) -> bool {
        matches!(self, ReadError::SourceAccess(_) | ReadError::SourceConfiguration(_))
    }

    /// The CLI exit code for this failure.
    pub fn exit_code(&self) -> i32 {
        if matches!(self, ReadError::SourceConfiguration(_)) {
            2
        } else {
            1
        }
    }
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for ReadError {}

impl From<rusqlite::Error> for ReadError {
    fn from(error: rusqlite::Error) -> Self {
        ReadError::Other(crate::adapters::sqlite_readable_db::sqlite_message(&error))
    }
}

//! Ports: what the use cases need from a source, a sink and a SQLite snapshot.

use super::models::{BundleSource, EventDraft, Label, SeedMatch, SessionHeader, SessionInfo, SkippedRow};
use super::ordered_map::OrderedMap;
use super::source_errors::ReadError;
use crate::adapters::sqlite_readable_db::ReadableDb;

/// Source port: owns the live Warp DB and hands out a read-only snapshot to run
/// work against. The snapshot is ephemeral and deleted once `f` returns.
pub trait ConversationSource {
    fn with_snapshot<T>(&self, f: impl FnOnce(&ReadableDb) -> Result<T, ReadError>) -> Result<T, ReadError>;
    /// Resolve the live DB path or fail with a clear error listing probed paths.
    fn resolve_live_db_path(&self) -> Result<String, ReadError>;
}

/// What a [`ConversationReader`] returns for one capture id.
///
/// `label_by_cid` is the source-agnostic binding (conversation -> its label);
/// the use case classifies, time-orders, and tallies labels from it. The
/// `unbindable`/`collisions` anomaly fields are marker-specific (Warp); other
/// sources return empty lists.
#[derive(Clone, Debug)]
pub struct CaptureRead {
    pub source: BundleSource,
    pub label_by_cid: OrderedMap<String, Label>,
    pub drafts: Vec<EventDraft>,
    pub skipped: Vec<SkippedRow>,
    pub unbindable: Vec<SeedMatch>,
    pub collisions: Vec<SeedMatch>,
    /// Seeds bound via the ai_queries temporal-proximity fallback.
    pub heuristic_bindings: Vec<SeedMatch>,
}

/// Reader port: resolves one capture id's conversations and their events.
pub trait ConversationReader {
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError>;
}

/// Options for [`SessionReader::read_sessions`].
#[derive(Clone, Debug, Default)]
pub struct ReadSessionsOptions {
    /// ISO-8601 date or datetime cutoff (validated by the CLI): only
    /// conversations whose last activity is at/after it are enumerated.
    pub since: Option<String>,
    /// Debug filter: enumerate exactly this one conversation (always gathered).
    pub conversation: Option<String>,
    /// Stored session headers by conversation id: a session whose stored
    /// `last_modified` equals the fresh stamp (raw string equality) is not gathered.
    pub stored_by_cid: OrderedMap<String, SessionHeader>,
}

/// What the reader gathered for one conversation.
#[derive(Clone, Debug)]
pub struct SessionGather {
    pub drafts: Vec<EventDraft>,
    pub skipped: Vec<SkippedRow>,
    /// The conversation's fresh source-native stamp (recorded in the header).
    pub last_modified: Option<String>,
}

/// What [`SessionReader::read_sessions`] returns for the whole run.
#[derive(Clone, Debug)]
pub struct SessionsRead {
    pub source: BundleSource,
    /// Sessions enumerated in this run, after the since/conversation filters.
    pub sessions: Vec<SessionInfo>,
    /// Gathered events per conversation — only conversations that changed (or new).
    pub reads: OrderedMap<String, SessionGather>,
}

/// Reader port for the non-marker session mode (`--sessions`): enumerates
/// every conversation in the source and gathers events for the ones that
/// changed, inside one source snapshot.
pub trait SessionReader {
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError>;
}

/// Every source implements both reader ports.
pub trait Reader: ConversationReader + SessionReader {}
impl<T: ConversationReader + SessionReader> Reader for T {}

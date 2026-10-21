//! ConversationReader + SessionReader backed by Warp's local SQLite DB.
//!
//! Binds a capture id to its conversations (CAPTURE_MARKER emissions) and
//! reads every event for them (queries, blocks, agent_tasks), all inside the
//! read-only snapshot the ConversationSource provides.

use super::readers::block_reader::read_blocks;
use super::readers::marker_reader::find_seeds;
use super::readers::query_reader::read_queries;
use super::readers::task_reader::read_tasks;
use super::sqlite_readable_db::ReadableDb;
use crate::domain::models::{BundleSource, SeedConfidence, SeedStatus, SessionInfo};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{
    CaptureRead, ConversationReader, ConversationSource, ReadSessionsOptions, SessionGather, SessionReader,
    SessionsRead,
};
use crate::domain::skip_unchanged::should_gather;
use crate::domain::source_errors::ReadError;
use crate::domain::stamps::{parse_since_utc, parse_stamp_utc};
use crate::js::spread_push;

pub struct WarpConversationReader<S: ConversationSource> {
    source: S,
}

impl<S: ConversationSource> WarpConversationReader<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }
}

impl<S: ConversationSource> ConversationReader for WarpConversationReader<S> {
    fn read_capture(&self, capture_id: &str) -> Result<CaptureRead, ReadError> {
        self.source.with_snapshot(|db| {
            let seeds = find_seeds(db, capture_id)?;
            let unbindable = seeds
                .iter()
                .filter(|s| s.status == SeedStatus::Unbindable)
                .cloned()
                .collect();
            let collisions = seeds
                .iter()
                .filter(|s| s.status == SeedStatus::Collision)
                .cloned()
                .collect();
            let heuristic_bindings = seeds
                .iter()
                .filter(|s| s.confidence == Some(SeedConfidence::Heuristic))
                .cloned()
                .collect();

            let mut label_by_cid = OrderedMap::new();
            for s in &seeds {
                if s.status == SeedStatus::Bound
                    && let Some(cid) = s.conversation_id.as_ref().filter(|c| !c.is_empty())
                {
                    label_by_cid.insert(cid.clone(), s.label.clone());
                }
            }
            let all_cids: Vec<String> = label_by_cid.keys().cloned().collect();

            // Fixed reader order (queries, blocks, tasks); the use case's stable
            // sort preserves it for equal timestamps.
            let mut drafts = Vec::new();
            let mut skipped = Vec::new();
            for result in [
                read_queries(db, &all_cids)?,
                read_blocks(db, &all_cids)?,
                read_tasks(db, &all_cids)?,
            ] {
                spread_push(&mut drafts, result.drafts).map_err(ReadError::Other)?;
                spread_push(&mut skipped, result.skipped).map_err(ReadError::Other)?;
            }
            Ok(CaptureRead {
                source: BundleSource::Warp,
                label_by_cid,
                drafts,
                skipped,
                unbindable,
                collisions,
                heuristic_bindings,
            })
        })
    }
}

impl<S: ConversationSource> SessionReader for WarpConversationReader<S> {
    /// Enumerate every conversation (durable tables only) and gather events
    /// for the ones that changed — all inside ONE snapshot.
    fn read_sessions(&self, options: &ReadSessionsOptions) -> Result<SessionsRead, ReadError> {
        self.source.with_snapshot(|db| {
            let mut selected = enumerate_sessions(db)?;
            if let Some(conversation) = &options.conversation {
                selected.retain(|s| &s.conversation_id == conversation);
            }
            if let Some(since) = &options.since
                && let Some(cutoff) = parse_since_utc(since)
            {
                // A stamp that cannot be parsed fails toward inclusion.
                selected.retain(|s| match &s.last_modified {
                    None => true,
                    Some(stamp) => parse_stamp_utc(stamp).is_none_or(|activity| activity >= cutoff),
                });
            }

            let mut reads = OrderedMap::new();
            for info in &selected {
                if !should_gather(info, options) {
                    continue;
                }
                let cid = vec![info.conversation_id.clone()];
                let (queries, blocks, tasks) = (read_queries(db, &cid)?, read_blocks(db, &cid)?, read_tasks(db, &cid)?);
                let mut drafts = queries.drafts;
                drafts.extend(blocks.drafts);
                drafts.extend(tasks.drafts);
                let mut skipped = queries.skipped;
                skipped.extend(blocks.skipped);
                skipped.extend(tasks.skipped);
                reads.insert(
                    info.conversation_id.clone(),
                    SessionGather {
                        drafts,
                        skipped,
                        last_modified: info.last_modified.clone(),
                    },
                );
            }
            Ok(SessionsRead {
                source: BundleSource::Warp,
                sessions: selected,
                reads,
            })
        })
    }
}

/// Every conversation with its last-activity stamp: the MAX over the durable
/// `agent_conversations`/`agent_tasks` tables. Stamped sessions by stamp
/// (tiebreak on id), then unstamped ones.
fn enumerate_sessions(db: &ReadableDb) -> Result<Vec<SessionInfo>, ReadError> {
    let rows = db.all(
        "SELECT conversation_id, MAX(last_modified_at) AS last_modified
       FROM (
         SELECT conversation_id, last_modified_at FROM agent_conversations
         UNION ALL
         SELECT conversation_id, last_modified_at FROM agent_tasks
       )
      GROUP BY conversation_id
      ORDER BY (last_modified IS NULL), last_modified, conversation_id",
        &[],
    )?;
    Ok(rows
        .iter()
        .map(|row| SessionInfo {
            conversation_id: row.get("conversation_id").text().unwrap_or_else(|| "null".to_string()),
            last_modified: row.get("last_modified").text(),
        })
        .collect())
}

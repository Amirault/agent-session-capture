//! Session mode (`--sessions`): every conversation, one file each.

use std::collections::HashSet;

use crate::adapters::jsonl_session_reader::{load_stored_session, scan_stored_sessions, source_mismatch};
use crate::adapters::jsonl_sink::JsonlSink;
use crate::domain::causal_sort::causal_order;
use crate::domain::merge_bundle::merge_event_lists;
use crate::domain::models::{
    BundleSource, EventRecord, SessionHeader, SessionRunSummary, WrittenSession, session_event,
};
use crate::domain::ordered_map::OrderedMap;
use crate::domain::ports::{ReadSessionsOptions, SessionReader};
use crate::domain::source_errors::ReadError;
use crate::js::{date, path};

pub struct ExtractSessionsOptions {
    /// Output directory (session files land in `<outDir>/sessions/`).
    pub out_dir: String,
    /// The run's source (drives the stored-file guards).
    pub source: BundleSource,
    pub since: Option<String>,
    pub conversation: Option<String>,
    /// When true, replace stored session files instead of merging with them.
    pub no_merge: bool,
}

/// Orchestrate the non-marker session export against a SessionReader:
///
/// 1. Scan `<outDir>/sessions/*.jsonl` headers (first line only), guarded.
/// 2. Read the source once; only changed conversations are gathered.
/// 3. Per gathered conversation: causal order + seq 1..N, then decay-safe merge
///    with the stored file's events when present and not `no_merge`.
/// 4. Write every file atomically, after every load and merge succeeded, so a
///    mid-run failure leaves the disk untouched. Stored files outside this
///    run's enumeration are kept untouched — never GC'd.
pub fn extract_sessions(
    reader: &dyn SessionReader,
    options: ExtractSessionsOptions,
) -> Result<SessionRunSummary, ReadError> {
    let ExtractSessionsOptions {
        out_dir,
        source,
        since,
        conversation,
        no_merge,
    } = options;
    let sessions_dir = path::join(&[&out_dir, "sessions"]);

    let stored = scan_stored_sessions(&sessions_dir, source, no_merge).map_err(ReadError::Other)?;
    let mut warnings = stored.warnings;
    let stored_by_cid: OrderedMap<String, SessionHeader> = stored
        .by_cid
        .iter()
        .map(|(cid, entry)| (cid.clone(), entry.header.clone()))
        .collect();

    let read = reader.read_sessions(&ReadSessionsOptions {
        since: since.clone(),
        conversation: conversation.clone(),
        stored_by_cid,
    })?;
    for cid in read.reads.keys() {
        let Some(other) = stored.other_sources.get(cid) else {
            continue;
        };
        if !no_merge {
            return Err(ReadError::Other(format!(
                "{} — refusing to continue; fix or remove the file, or re-run with --no-merge to replace it",
                source_mismatch(other, source)
            )));
        }
        warnings.push(format!(
            "replacing stored session (--no-merge): {}",
            source_mismatch(other, source)
        ));
    }

    let mut built: Vec<(SessionHeader, Vec<EventRecord>)> = Vec::new();
    let mut skipped_rows = Vec::new();
    for (cid, gather) in read.reads.iter() {
        let mut events: Vec<EventRecord> = causal_order(gather.drafts.clone())
            .iter()
            .enumerate()
            .map(|(i, d)| session_event(d, i + 1))
            .collect();
        if !no_merge && let Some(entry) = stored.by_cid.get(cid) {
            let stored_file = load_stored_session(&entry.path).map_err(ReadError::Other)?;
            events = merge_event_lists(&stored_file.events, &events);
        }
        let extracted_at = date::to_iso_string(date::now_ms()).expect("valid date");
        built.push((
            SessionHeader::fresh(cid, source, gather.last_modified.as_deref(), &extracted_at),
            events,
        ));
        skipped_rows.extend(gather.skipped.iter().cloned());
    }

    let sink = JsonlSink::new(&out_dir);
    let mut written = Vec::new();
    for (header, events) in &built {
        let out_path = sink
            .write_session(header, events)
            .map_err(|e| ReadError::Other(e.to_string()))?;
        written.push(WrittenSession {
            conversation_id: header.conversation_id.clone(),
            path: out_path,
            events: events.len(),
        });
    }

    let gathered: HashSet<&String> = read.reads.keys().collect();
    let unchanged: Vec<String> = read
        .sessions
        .iter()
        .filter(|s| !gathered.contains(&s.conversation_id))
        .map(|s| s.conversation_id.clone())
        .collect();
    let mut touched: HashSet<String> = written.iter().map(|w| w.path.clone()).collect();
    for cid in &unchanged {
        if let Some(entry) = stored.by_cid.get(cid) {
            touched.insert(entry.path.clone());
        }
    }
    let untouched = stored
        .by_cid
        .values()
        .filter(|e| !touched.contains(&e.path))
        .map(|e| e.path.clone())
        .collect();

    Ok(SessionRunSummary {
        since,
        conversation,
        enumerated: read.sessions.len(),
        written,
        unchanged,
        untouched,
        skipped_rows,
        warnings,
    })
}

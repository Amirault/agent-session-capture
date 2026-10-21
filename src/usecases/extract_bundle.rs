//! Marker mode: one capture id → one chronological bundle.

use crate::domain::bundle_header::compute_bundle_header;
use crate::domain::causal_sort::causal_order;
use crate::domain::merge_bundle::merge_bundles;
use crate::domain::models::{CaptureBundle, RunSummary, conversation_event};
use crate::domain::ports::{CaptureRead, ConversationReader};
use crate::domain::source_errors::ReadError;
use crate::js::date;

/// Options for [`extract_bundle`].
#[derive(Default)]
pub struct ExtractOptions {
    /// When true, replace the existing bundle instead of merging with it.
    pub no_merge: bool,
    /// A previously-captured bundle to merge with (decay-safe).
    pub existing_bundle: Option<CaptureBundle>,
}

pub struct ExtractResult {
    pub bundle: CaptureBundle,
    pub summary: RunSummary,
}

fn now_iso() -> String {
    date::to_iso_string(date::now_ms()).expect("current time is a valid date")
}

/// Orchestrate the full extraction for one capture id against a reader:
/// read → classify each event by its conversation's label → causally order
/// the whole bundle and assign a monotonic seq → compute the bundle header.
///
/// With an `existing_bundle` and no `no_merge`, the fresh bundle is merged
/// with it decay-safe. If the fresh read errors and a stored bundle exists,
/// the result degrades to stored-only (with a `fresh_read_error` warning);
/// with no stored bundle it fails. A source access error always propagates.
pub fn extract_bundle(
    reader: &dyn ConversationReader,
    capture_id: &str,
    options: ExtractOptions,
) -> Result<ExtractResult, ReadError> {
    let ExtractOptions {
        no_merge,
        existing_bundle,
    } = options;

    let capture_read: CaptureRead = match reader.read_capture(capture_id) {
        Ok(read) => read,
        Err(error) if error.is_source_access() => return Err(error),
        Err(error) => {
            if let Some(existing) = existing_bundle {
                return Ok(stored_only_result(capture_id, existing, error.message().to_string()));
            }
            return Err(ReadError::Other(format!(
                "fresh read failed for capture id \"{capture_id}\" and no stored bundle exists to fall back on: {}",
                error.message()
            )));
        }
    };

    let CaptureRead {
        source,
        label_by_cid,
        drafts,
        skipped,
        unbindable,
        collisions,
        heuristic_bindings,
    } = capture_read;

    let bound: Vec<_> = drafts
        .into_iter()
        .filter(|d| label_by_cid.contains_key(&d.conversation_id))
        .collect();
    let events = causal_order(bound)
        .iter()
        .enumerate()
        .map(|(i, d)| {
            conversation_event(
                capture_id,
                label_by_cid.get(&d.conversation_id).expect("bound"),
                d,
                i + 1,
            )
        })
        .collect();

    let fresh = CaptureBundle {
        header: compute_bundle_header(capture_id, source, &label_by_cid, &now_iso()),
        events,
    };
    let bundle = match existing_bundle {
        Some(existing) if !no_merge => merge_bundles(&existing, &fresh, &now_iso()),
        _ => fresh,
    };

    let summary = RunSummary {
        capture_id: capture_id.to_string(),
        conversations: bundle.conversation_count(),
        events: bundle.events.len(),
        unbindable,
        collisions,
        heuristic_bindings,
        skipped_rows: skipped,
        fresh_read_error: None,
    };
    Ok(ExtractResult { bundle, summary })
}

/// The stored bundle, re-stamped, when the fresh read failed: events are kept
/// intact and `fresh_read_error` surfaces the degradation.
fn stored_only_result(capture_id: &str, existing: CaptureBundle, fresh_read_error: String) -> ExtractResult {
    let mut header = existing.header;
    header.insert("extracted_at", now_iso());
    let bundle = CaptureBundle {
        header,
        events: existing.events,
    };
    let summary = RunSummary {
        capture_id: capture_id.to_string(),
        conversations: bundle.conversation_count(),
        events: bundle.events.len(),
        fresh_read_error: Some(fresh_read_error),
        ..RunSummary::default()
    };
    ExtractResult { bundle, summary }
}

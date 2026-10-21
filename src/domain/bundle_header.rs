//! The bundle header fields computed from the conversation→label binding.

use super::models::BundleSource;
use super::ordered_map::OrderedMap;
use crate::js::{Object, Value};

/// Compute the bundle header (`type, capture_id, labels,
/// conversations_per_label, conversation_ids, source, extracted_at`).
///
/// Shared by `extract_bundle` (live read) and `merge_bundles` (decay-safe
/// merge) so how conversations are tallied lives in exactly one place.
/// `label_by_cid` preserves insertion order, which becomes the
/// `conversation_ids` order; labels are listed in JavaScript property order of
/// the per-label tally (first appearance, array-index-like labels first).
pub fn compute_bundle_header(
    capture_id: &str,
    source: BundleSource,
    label_by_cid: &OrderedMap<String, String>,
    extracted_at: &str,
) -> Object {
    let mut per_label = Object::new();
    for label in label_by_cid.values() {
        let count = per_label.get(label).and_then(Value::as_f64).unwrap_or(0.0);
        per_label.insert(label.as_str(), count + 1.0);
    }
    let labels: Vec<Value> = per_label.keys().map(|k| Value::from(k.as_str())).collect();
    let conversation_ids: Vec<Value> = label_by_cid.keys().map(|k| Value::from(k.as_str())).collect();
    let mut header = Object::new();
    header.insert("type", "bundle_header");
    header.insert("capture_id", capture_id);
    header.insert("labels", labels);
    header.insert("conversations_per_label", per_label);
    header.insert("conversation_ids", conversation_ids);
    header.insert("source", source.as_str());
    header.insert("extracted_at", extracted_at);
    header
}

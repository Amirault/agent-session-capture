//! Decay-safe merge of a stored bundle (or session file) with a fresh read.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use super::models::{CaptureBundle, EventRecord, js_display};
use super::ordered_map::OrderedMap;
use crate::js::string::{js_cmp, locale_compare};
use crate::js::{Object, Value};

/// Merge-only aggregation metadata can change when the same task is re-read.
const VOLATILE_META_KEYS: [&str; 2] = ["merged_count", "repeat"];
const SOURCE_SURROGATE_META_KEYS: [&str; 4] = ["block_id", "exchange_id", "task_event_index", "task_id"];

#[derive(Clone, Copy, PartialEq)]
enum KeyMode {
    Strong,
    Compatibility,
}

fn meta_of(event: &EventRecord) -> Option<&Object> {
    event.get("meta").and_then(Value::as_object)
}

/// Stable content identity used across captures. `seq` is always per-run. Only
/// Warp agent_tasks timestamps are mutable; stable timestamps remain part of all
/// other event identities so separate same-content queries/commands cannot merge.
/// Compatibility mode strips newly-added source IDs solely for matching legacy
/// stored bundles that predate those IDs.
fn event_key(event: &EventRecord, mode: KeyMode) -> String {
    let mut entries: Vec<(&String, &Value)> = meta_of(event)
        .map(|meta| {
            meta.iter()
                .filter(|(key, _)| {
                    !VOLATILE_META_KEYS.contains(&key.as_str())
                        && (mode == KeyMode::Strong || !SOURCE_SURROGATE_META_KEYS.contains(&key.as_str()))
                })
                .collect()
        })
        .unwrap_or_default();
    stable_sort_by(&mut entries, |a, b| locale_compare(a.0, b.0));
    let meta: Object = entries.into_iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    let mut key = Object::new();
    for field in ["conversation_id", "label", "role", "kind", "content"] {
        if let Some(value) = event.get(field) {
            key.insert(field, value.clone());
        }
    }
    key.insert("meta", meta);
    if !has_mutable_task_timestamp(event)
        && let Some(ts) = event.get("ts")
    {
        key.insert("ts", ts.clone());
    }
    key.to_json()
}

fn has_mutable_task_timestamp(event: &EventRecord) -> bool {
    if event.get_str("kind") != Some("agent_message") {
        return false;
    }
    let Some(meta) = meta_of(event) else { return false };
    matches!(meta.get("task_id"), Some(Value::String(_)))
        || matches!(meta.get("task_event_index"), Some(Value::Number(_)))
        || matches!(meta.get("field_path"), Some(Value::String(_)))
}

fn has_source_surrogate(event: &EventRecord) -> bool {
    meta_of(event).is_some_and(|meta| meta.keys().any(|k| SOURCE_SURROGATE_META_KEYS.contains(&k.as_str())))
}

/// `a < b` for two JSON values, as JavaScript compares them.
pub fn js_less(a: Option<&Value>, b: Option<&Value>) -> bool {
    match (a, b) {
        (Some(Value::String(x)), Some(Value::String(y))) => js_cmp(x, y) == Ordering::Less,
        _ => {
            let number = |v: Option<&Value>| -> f64 {
                match v {
                    Some(Value::Number(n)) => *n,
                    Some(Value::String(s)) => {
                        let t = crate::js::string::js_trim(s);
                        if t.is_empty() {
                            0.0
                        } else {
                            t.parse().unwrap_or(f64::NAN)
                        }
                    }
                    Some(Value::Bool(b)) => f64::from(u8::from(*b)),
                    Some(Value::Null) => 0.0,
                    _ => f64::NAN,
                }
            };
            number(a) < number(b)
        }
    }
}

/// The comparator `(a, b) => a.ts < b.ts ? -1 : a.ts > b.ts ? 1 : 0`.
fn by_ts(a: &EventRecord, b: &EventRecord) -> Ordering {
    let (x, y) = (a.get("ts"), b.get("ts"));
    if js_less(x, y) {
        Ordering::Less
    } else if js_less(y, x) {
        Ordering::Greater
    } else {
        Ordering::Equal
    }
}

/// A stable merge sort that, unlike `slice::sort_by`, never panics on an
/// inconsistent comparator (JavaScript's `Array.prototype.sort` does not either).
pub fn stable_sort_by<T>(items: &mut Vec<T>, mut compare: impl FnMut(&T, &T) -> Ordering) {
    fn merge_sort<T>(
        order: &mut [usize],
        buffer: &mut Vec<usize>,
        items: &[T],
        compare: &mut impl FnMut(&T, &T) -> Ordering,
    ) {
        let n = order.len();
        if n <= 1 {
            return;
        }
        if n <= 16 {
            for i in 1..n {
                let mut j = i;
                while j > 0 && compare(&items[order[j - 1]], &items[order[j]]) == Ordering::Greater {
                    order.swap(j - 1, j);
                    j -= 1;
                }
            }
            return;
        }
        let mid = n / 2;
        merge_sort(&mut order[..mid], buffer, items, compare);
        merge_sort(&mut order[mid..], buffer, items, compare);
        if compare(&items[order[mid - 1]], &items[order[mid]]) != Ordering::Greater {
            return;
        }
        buffer.clear();
        let (mut i, mut j) = (0, mid);
        while i < mid && j < n {
            if compare(&items[order[j]], &items[order[i]]) == Ordering::Less {
                buffer.push(order[j]);
                j += 1;
            } else {
                buffer.push(order[i]);
                i += 1;
            }
        }
        buffer.extend_from_slice(&order[i..mid]);
        buffer.extend_from_slice(&order[j..n]);
        order.copy_from_slice(buffer);
    }
    let mut order: Vec<usize> = (0..items.len()).collect();
    let mut buffer = Vec::with_capacity(items.len());
    merge_sort(&mut order, &mut buffer, items, &mut compare);
    let mut slots: Vec<Option<T>> = items.drain(..).map(Some).collect();
    items.extend(order.into_iter().map(|i| slots[i].take().expect("each item once")));
}

fn claim_fresh_index(
    index: &HashMap<String, Vec<usize>>,
    cursors: &mut HashMap<String, usize>,
    consumed: &HashSet<usize>,
    key: &str,
) -> Option<usize> {
    let candidates = index.get(key)?;
    let mut cursor = cursors.get(key).copied().unwrap_or(0);
    while cursor < candidates.len() && consumed.contains(&candidates[cursor]) {
        cursor += 1;
    }
    cursors.insert(key.to_string(), cursor + 1);
    candidates.get(cursor).copied()
}

/// Decay-safe merge of two event lists — the pairing machinery both extraction
/// modes share.
///
/// Fresh events are primary; stored events only fill gaps fresh can no longer
/// bind (full marker decay) or has evicted (partial ring-buffer eviction).
/// Overlapping re-read events dedup by stable source identity. Only Warp
/// agent_tasks ignore their mutable row timestamp, and legacy bundles use a
/// compatibility key that omits newly-added source IDs. Fresh content and meta
/// remain primary; only the earliest observed task timestamp is reconciled from
/// the stored copy.
///
/// The result is stably sorted by ts with a monotonic `seq` reassigned 1..N.
pub fn merge_event_lists(stored: &[EventRecord], fresh: &[EventRecord]) -> Vec<EventRecord> {
    let mut unioned: Vec<EventRecord> = fresh.to_vec();
    let mut by_strong: HashMap<String, Vec<usize>> = HashMap::new();
    let mut by_compat: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, event) in unioned.iter().enumerate() {
        by_strong.entry(event_key(event, KeyMode::Strong)).or_default().push(i);
        by_compat
            .entry(event_key(event, KeyMode::Compatibility))
            .or_default()
            .push(i);
    }

    let mut consumed: HashSet<usize> = HashSet::new();
    let mut strong_cursors = HashMap::new();
    let mut compat_cursors = HashMap::new();
    for stored_event in stored {
        let mut fresh_index = claim_fresh_index(
            &by_strong,
            &mut strong_cursors,
            &consumed,
            &event_key(stored_event, KeyMode::Strong),
        );
        if fresh_index.is_none() && !has_source_surrogate(stored_event) {
            fresh_index = claim_fresh_index(
                &by_compat,
                &mut compat_cursors,
                &consumed,
                &event_key(stored_event, KeyMode::Compatibility),
            );
        }
        let Some(fresh_index) = fresh_index else {
            unioned.push(stored_event.clone());
            continue;
        };
        consumed.insert(fresh_index);
        let fresh_event = &mut unioned[fresh_index];
        if has_mutable_task_timestamp(fresh_event) && js_less(stored_event.get("ts"), fresh_event.get("ts")) {
            let ts = stored_event.get("ts").cloned().unwrap_or_default();
            fresh_event.insert("ts", ts);
        }
    }

    stable_sort_by(&mut unioned, by_ts);
    for (i, event) in unioned.iter_mut().enumerate() {
        event.insert("seq", i + 1);
    }
    unioned
}

/// Merge a stored bundle with a fresh one, decay-safe — a thin wrapper over
/// [`merge_event_lists`] that re-stamps the capture id and recomputes the
/// bundle header (labels, per-label tallies, …) from the merged event set.
pub fn merge_bundles(existing: &CaptureBundle, fresh: &CaptureBundle, extracted_at: &str) -> CaptureBundle {
    let capture_id = fresh.header.get("capture_id").cloned().unwrap_or_default();
    let mut events = merge_event_lists(&existing.events, &fresh.events);
    for event in &mut events {
        event.insert("capture_id", capture_id.clone());
    }

    // conversation -> label, first seen wins (keys keep their JSON identity).
    let mut label_by_cid: OrderedMap<String, (Option<Value>, Option<Value>)> = OrderedMap::new();
    for event in &events {
        let cid = event.get("conversation_id");
        let key = cid.map_or_else(|| "undefined".to_string(), Value::to_json);
        if !label_by_cid.contains_key(&key) {
            label_by_cid.insert(key, (cid.cloned(), event.get("label").cloned()));
        }
    }

    let mut per_label = Object::new();
    for (_, label) in label_by_cid.values() {
        let name = js_display(label.as_ref());
        let count = per_label.get(&name).and_then(Value::as_f64).unwrap_or(0.0);
        per_label.insert(name, count + 1.0);
    }
    let labels: Vec<Value> = per_label.keys().map(|k| Value::from(k.as_str())).collect();
    let conversation_ids: Vec<Value> = label_by_cid
        .values()
        .map(|(cid, _)| cid.clone().unwrap_or(Value::Null))
        .collect();

    let mut header = Object::new();
    header.insert("type", "bundle_header");
    header.insert("capture_id", capture_id);
    header.insert("labels", labels);
    header.insert("conversations_per_label", per_label);
    header.insert("conversation_ids", conversation_ids);
    header.insert("source", fresh.header.get("source").cloned().unwrap_or_default());
    header.insert("extracted_at", extracted_at);
    CaptureBundle { header, events }
}

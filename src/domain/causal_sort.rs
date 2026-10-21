//! Causal sort key for drafts whose source persists true insertion order.

use std::collections::HashMap;

use super::models::EventDraft;
use crate::js::Value;
use crate::js::string::js_cmp;

/// Causal sort timestamp for one draft.
///
/// Hermes (and any source) can persist true insertion order in `message_id`
/// because clocks can move backwards. When a draft carries a numeric
/// `message_id`, raise only the internal sort key to the previous timestamp in
/// the same conversation; this preserves causality while keeping the public `ts`
/// intact. The caller keeps one high-water map per run.
pub fn causal_sort_timestamp(draft: &EventDraft, high_water_by_conversation: &mut HashMap<String, String>) -> String {
    if !matches!(draft.meta.get("message_id"), Some(Value::Number(_))) {
        return draft.ts.clone();
    }
    let sort_timestamp = match high_water_by_conversation.get(&draft.conversation_id) {
        Some(previous) if js_cmp(previous, &draft.ts).is_gt() => previous.clone(),
        _ => draft.ts.clone(),
    };
    high_water_by_conversation.insert(draft.conversation_id.clone(), sort_timestamp.clone());
    sort_timestamp
}

/// Order drafts causally (stable on the original index) and return them.
pub fn causal_order(drafts: Vec<EventDraft>) -> Vec<EventDraft> {
    let mut high_water = HashMap::new();
    let mut keyed: Vec<(String, usize, EventDraft)> = drafts
        .into_iter()
        .enumerate()
        .map(|(index, draft)| (causal_sort_timestamp(&draft, &mut high_water), index, draft))
        .collect();
    keyed.sort_by(|a, b| js_cmp(&a.0, &b.0).then(a.1.cmp(&b.1)));
    keyed.into_iter().map(|(_, _, draft)| draft).collect()
}

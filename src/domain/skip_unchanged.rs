//! Skip-unchanged, shared by every SessionReader.

use super::models::SessionInfo;
use super::ports::ReadSessionsOptions;

/// Gather unless the stored header's stamp equals the fresh stamp (raw string
/// equality — the stamps never change representation). New conversations,
/// `None` stamps (no durable stamp — always re-extract) and the
/// `--conversation` debug filter are always gathered.
pub fn should_gather(info: &SessionInfo, options: &ReadSessionsOptions) -> bool {
    if options.conversation.as_deref() == Some(info.conversation_id.as_str()) {
        return true;
    }
    let Some(stamp) = &info.last_modified else { return true };
    match options.stored_by_cid.get(&info.conversation_id) {
        None => true,
        Some(stored) => stored.last_modified.as_deref() != Some(stamp.as_str()),
    }
}

//! Ported test suite (Given/When/Then), run against the hand-trimmed test schema.

pub mod fixtures;
pub mod hermes_fixture;

mod claude_code_session_reader;
mod claude_code_transcript_reader;
mod collapse_deltas;
mod compact;
mod extract_bundle;
mod extract_sessions;
mod format_run_report;
mod format_session_report;
mod hermes_conversation_reader;
mod hermes_session_reader;
mod jsonl_sink;
mod langfuse_conversation_reader;
mod langfuse_observation_client;
mod langfuse_session_reader;
mod marker_docs_contract;
mod marker_reader;
mod merge_bundle;
mod merge_event_lists;
mod oz_conversation_reader;
mod protobuf_walk;
mod query_reader_block_reader;
mod schema_overlay;
mod task_reader;
mod warp_conversation_reader;
mod warp_session_reader;
mod warp_sqlite_adapter;

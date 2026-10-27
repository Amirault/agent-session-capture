//! Adapters: one reader per source, the Warp protobuf pipeline, and the JSONL
//! sink and stored-file readers.

pub mod ansi;
pub mod claude_code_transcript_reader;
pub mod collapse_deltas;
pub mod compact;
pub mod envelope_dedupe;
pub mod hermes_conversation_reader;
pub mod jsonl_bundle_reader;
pub mod jsonl_session_reader;
pub mod jsonl_sink;
pub mod langfuse_conversation_reader;
pub mod langfuse_observation_client;
pub mod langfuse_observation_lister;
pub mod oz_conversation_reader;
pub mod oz_run_store;
pub mod parallel;
pub mod proto_schema;
pub mod protobuf_walk;
pub mod readers;
pub mod schema_overlay;
pub mod session_file_name;
pub mod sqlite_readable_db;
pub mod warp_conversation_reader;
pub mod warp_sqlite_adapter;

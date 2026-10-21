//! agent-session-capture: export every coding-agent conversation tagged with
//! a `CAPTURE_MARKER` id — from Claude Code, Hermes, Warp or Langfuse — as one
//! chronological JSONL, or every conversation of a store with `--sessions`.
//!
//! Ports & adapters: [`domain`] holds the pure models and rules, [`usecases`]
//! the two run modes, [`adapters`] every source and the JSONL sink, [`cli`]
//! the command line.

pub mod adapters;
pub mod cli;
pub mod domain;
pub mod format_run_report;
pub mod js;
pub mod usecases;

#[cfg(test)]
mod tests;

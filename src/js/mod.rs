//! JavaScript runtime semantics the CLI contract depends on.
//!
//! The tool was first written in TypeScript; its output (JSON bytes, number
//! and date formatting, V8 error messages, path normalization, string order)
//! is the contract. These modules reproduce exactly those behaviours.

pub mod buffer;
pub mod date;
pub mod json;
pub mod number;
pub mod path;
pub mod string;

pub use json::{Object, Value};

/// V8 rejects a call spread (`array.push(...items)`) past about this many
/// arguments; the exact bound depends slightly on stack depth.
pub const MAX_SPREAD_ARGUMENTS: usize = 124_000;

/// `target.push(...items)`, failing as V8 does for a huge `items`.
pub fn spread_push<T>(target: &mut Vec<T>, items: Vec<T>) -> Result<(), String> {
    if items.len() > MAX_SPREAD_ARGUMENTS {
        return Err("Maximum call stack size exceeded".to_string());
    }
    target.extend(items);
    Ok(())
}

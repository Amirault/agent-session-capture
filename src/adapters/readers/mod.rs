//! Warp sub-readers: markers, prompts, commands and agent tasks.

pub mod block_reader;
pub mod marker_reader;
pub mod query_reader;
pub mod task_reader;

use rusqlite::ToSql;

use super::sqlite_readable_db::Cell;
use crate::js::Value;

/// A driver value as JavaScript saw it in a JSON-serialized `meta`.
pub fn cell_to_value(cell: &Cell) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::Integer(i) => Value::Number(*i as f64),
        Cell::Real(f) => Value::Number(*f),
        Cell::Text(s) => Value::String(s.clone()),
        Cell::Blob(b) => {
            let mut buffer = crate::js::Object::new();
            buffer.insert("type", "Buffer");
            buffer.insert(
                "data",
                b.iter().map(|x| Value::Number(f64::from(*x))).collect::<Vec<_>>(),
            );
            Value::Object(buffer)
        }
    }
}

/// Template-literal display of a driver value (`null` for NULL).
pub fn cell_display(cell: &Cell) -> String {
    cell.text().unwrap_or_else(|| "null".to_string())
}

/// `?` placeholders and bound values for an `IN (...)` list.
pub fn in_list(ids: &[String]) -> (String, Vec<&dyn ToSql>) {
    let placeholders = vec!["?"; ids.len()].join(",");
    let params = ids.iter().map(|id| id as &dyn ToSql).collect();
    (placeholders, params)
}

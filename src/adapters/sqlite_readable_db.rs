//! Read-only SQLite access shared by the Warp and Hermes readers.

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, ToSql};

use crate::domain::source_errors::ReadError;
use crate::js::number::number_to_string;

/// One SQLite value, as the original driver handed it to JavaScript.
#[derive(Clone, Debug, PartialEq)]
pub enum Cell {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Cell {
    fn from_ref(value: ValueRef<'_>) -> Self {
        match value {
            ValueRef::Null => Cell::Null,
            ValueRef::Integer(i) => Cell::Integer(i),
            ValueRef::Real(f) => Cell::Real(f),
            ValueRef::Text(t) => Cell::Text(String::from_utf8_lossy(t).into_owned()),
            ValueRef::Blob(b) => Cell::Blob(b.to_vec()),
        }
    }

    /// The value as a string column (`null` for NULL; numbers printed as JS would).
    pub fn text(&self) -> Option<String> {
        match self {
            Cell::Null => None,
            Cell::Integer(i) => Some(i.to_string()),
            Cell::Real(f) => Some(number_to_string(*f)),
            Cell::Text(s) => Some(s.clone()),
            Cell::Blob(b) => Some(String::from_utf8_lossy(b).into_owned()),
        }
    }

    /// The value as a JS number (`None` for NULL or non-numeric).
    pub fn number(&self) -> Option<f64> {
        match self {
            Cell::Integer(i) => Some(*i as f64),
            Cell::Real(f) => Some(*f),
            _ => None,
        }
    }

    /// The value as bytes (BLOB or TEXT); `None` for NULL.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        match self {
            Cell::Blob(b) => Some(b.clone()),
            Cell::Text(s) => Some(s.as_bytes().to_vec()),
            Cell::Null => None,
            other => other.text().map(String::into_bytes),
        }
    }

    /// JavaScript truthiness of the driver value.
    pub fn truthy(&self) -> bool {
        match self {
            Cell::Null => false,
            Cell::Integer(i) => *i != 0,
            Cell::Real(f) => *f != 0.0 && !f.is_nan(),
            Cell::Text(s) => !s.is_empty(),
            Cell::Blob(_) => true,
        }
    }
}

/// One result row: cells in column order, addressable by column name.
pub struct Row {
    names: std::sync::Arc<Vec<String>>,
    cells: Vec<Cell>,
}

impl Row {
    pub fn get(&self, name: &str) -> &Cell {
        static NULL: Cell = Cell::Null;
        self.names
            .iter()
            .position(|n| n == name)
            .map_or(&NULL, |i| &self.cells[i])
    }
}

/// The message better-sqlite3 surfaced for a SQLite error.
pub fn sqlite_message(error: &rusqlite::Error) -> String {
    match error {
        rusqlite::Error::SqliteFailure(_, Some(message)) => message.clone(),
        rusqlite::Error::SqliteFailure(e, None) => rusqlite::ffi::code_to_str(e.extended_code).to_string(),
        other => other.to_string(),
    }
}

/// Minimal read-only query interface over a SQLite connection.
pub struct ReadableDb {
    conn: Connection,
}

impl ReadableDb {
    pub fn new(conn: Connection) -> Self {
        Self { conn }
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Open `path` read-only; the file must exist.
    pub fn open_read_only(path: &str) -> Result<Self, String> {
        let parent = crate::js::path::dirname(path);
        if std::fs::metadata(parent).is_err() {
            return Err("Cannot open database because the directory does not exist".to_string());
        }
        Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
            .map(Self::new)
            .map_err(|e| sqlite_message(&e))
    }

    /// `db.prepare(sql).all(...params)`.
    pub fn all(&self, sql: &str, params: &[&dyn ToSql]) -> Result<Vec<Row>, ReadError> {
        let mut statement = self.conn.prepare_cached(sql)?;
        let names: std::sync::Arc<Vec<String>> =
            std::sync::Arc::new(statement.column_names().iter().map(|n| n.to_string()).collect());
        let width = names.len();
        let mut rows = statement.query(params)?;
        let mut out = Vec::new();
        while let Some(row) = rows.next()? {
            let mut cells = Vec::with_capacity(width);
            for i in 0..width {
                cells.push(Cell::from_ref(row.get_ref(i)?));
            }
            out.push(Row {
                names: names.clone(),
                cells,
            });
        }
        Ok(out)
    }
}

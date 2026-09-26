use crate::{SheetInfo, Table};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Number, Value};
use std::path::Path;

fn connect(path: &Path) -> Result<Connection, String> {
    Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| e.to_string())
}

/// Table and view names from `sqlite_master`, in creation order, excluding
/// SQLite's own internal `sqlite_%` tables.
pub fn names(path: &Path) -> Result<Vec<String>, String> {
    let conn = connect(path)?;
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY rowid",
        )
        .map_err(|e| e.to_string())?;
    let names = stmt
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    Ok(names)
}

pub fn info(path: &Path) -> Result<Vec<SheetInfo>, String> {
    let conn = connect(path)?;
    names(path)?
        .into_iter()
        .map(|name| {
            let count: u64 = conn
                .query_row(
                    &format!("SELECT COUNT(*) FROM {}", quote(&name)),
                    [],
                    |row| row.get(0),
                )
                .map_err(|e| e.to_string())?;
            let columns = conn
                .prepare(&format!("PRAGMA table_info({})", quote(&name)))
                .map_err(|e| e.to_string())?
                .query_map([], |row| row.get::<_, String>(1))
                .map_err(|e| e.to_string())?
                .count();
            Ok(SheetInfo {
                sheet: name,
                rows: count,
                columns,
            })
        })
        .collect()
}

/// First rows read at once; each later read doubles, so a preview reads
/// little and a deep `--rows` range still takes few queries.
const FIRST_CHUNK: u64 = 256;

/// A table's or view's rows and `PRAGMA table_info` column types. Rows are
/// read lazily in growing `LIMIT`/`OFFSET` chunks, since a `Statement`'s row
/// iterator borrows the `Connection` and `Table::rows` must own its source.
pub fn open(path: &Path, sheet: String) -> Result<Table, String> {
    let conn = connect(path)?;
    let quoted = quote(&sheet);
    let columns: Vec<(String, String)> = conn
        .prepare(&format!("PRAGMA table_info({quoted})"))
        .map_err(|e| e.to_string())?
        .query_map([], |row| {
            Ok((row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })
        .map_err(|e| e.to_string())?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    let header: Vec<String> = columns.iter().map(|(name, _)| name.clone()).collect();
    let types: Vec<String> = columns
        .iter()
        .map(|(_, declared)| affinity(declared).to_string())
        .collect();
    let total_rows: u64 = conn
        .query_row(&format!("SELECT COUNT(*) FROM {quoted}"), [], |row| {
            row.get(0)
        })
        .map_err(|e| e.to_string())?;
    let width = header.len();
    let mut offset = 0;
    let mut chunk = FIRST_CHUNK;
    let mut buffer = std::collections::VecDeque::new();
    let rows = std::iter::from_fn(move || {
        if buffer.is_empty() && offset < total_rows {
            let read = conn
                .prepare(&format!(
                    "SELECT * FROM {quoted} LIMIT {chunk} OFFSET {offset}"
                ))
                .and_then(|mut stmt| {
                    stmt.query_map([], |row| {
                        (0..width)
                            .map(|i| row.get_ref(i).map(from_value_ref))
                            .collect()
                    })?
                    .collect::<Result<Vec<Vec<Value>>, _>>()
                });
            match read {
                Ok(rows) if !rows.is_empty() => {
                    offset += rows.len() as u64;
                    chunk *= 2;
                    buffer.extend(rows);
                }
                Ok(_) => offset = total_rows,
                Err(e) => {
                    offset = total_rows;
                    return Some(Err(e.to_string()));
                }
            }
        }
        buffer.pop_front().map(Ok)
    });
    Ok(Table {
        sheet,
        header,
        total_rows,
        types: Some(types),
        rows: Box::new(rows),
        warnings: vec![],
    })
}

fn from_value_ref(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(n) => Value::Number(n.into()),
        ValueRef::Real(f) => Number::from_f64(f).map_or(Value::Null, Value::Number),
        ValueRef::Text(s) => Value::String(String::from_utf8_lossy(s).into_owned()),
        ValueRef::Blob(b) => Value::String(format!("<blob {} bytes>", b.len())),
    }
}

/// SQLite's column type affinity rules (declared type -> storage class),
/// mapped to the crate's schema type vocabulary. REAL and NUMERIC affinity
/// (everything left over, such as DECIMAL) both report as "float".
fn affinity(declared: &str) -> &'static str {
    let declared = declared.to_uppercase();
    if declared.contains("INT") {
        "int"
    } else if declared.contains("CHAR") || declared.contains("CLOB") || declared.contains("TEXT") {
        "string"
    } else if declared.contains("BLOB") || declared.is_empty() {
        "binary"
    } else {
        "float"
    }
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

use crate::{Table, cells};
use csv::ReaderBuilder;
use serde_json::Value;
use std::path::Path;

fn reader(delimiter: u8) -> ReaderBuilder {
    let mut builder = ReaderBuilder::new();
    builder
        .delimiter(delimiter)
        .flexible(true)
        .has_headers(true);
    builder
}

pub fn open(path: &Path, delimiter: u8, sheet: String) -> Result<Table, String> {
    let mut counter = reader(delimiter)
        .from_path(path)
        .map_err(|e| e.to_string())?;
    let width = counter.headers().map_err(|e| e.to_string())?.len();
    let mut total = 0;
    let mut wide = 0;
    for record in counter.records() {
        let record = record.map_err(|e| e.to_string())?;
        total += 1;
        if record.len() > width {
            wide += 1;
        }
    }
    let mut warnings = vec![];
    if wide > 0 {
        warnings.push(format!(
            "{wide} rows have more fields than the header; the extra fields are dropped."
        ));
    }
    let mut rows = reader(delimiter)
        .from_path(path)
        .map_err(|e| e.to_string())?;
    let header = cells::header(
        rows.headers()
            .map_err(|e| e.to_string())?
            .iter()
            .map(String::from)
            .collect(),
    );
    let rows = rows.into_records().map(move |record| {
        let record = record.map_err(|e| e.to_string())?;
        let mut row: Vec<Value> = record.iter().take(width).map(cells::parse).collect();
        row.resize(width, Value::Null);
        Ok(row)
    });
    Ok(Table {
        sheet,
        header,
        total_rows: total,
        types: None,
        rows: Box::new(rows),
        warnings,
    })
}

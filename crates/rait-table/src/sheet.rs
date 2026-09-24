use crate::{SheetInfo, Table, cells};
use calamine::{Data, Range, Reader, open_workbook_auto};
use serde_json::Value;
use std::path::Path;

pub fn names(path: &Path) -> Result<Vec<String>, String> {
    Ok(open_workbook_auto(path)
        .map_err(|e| e.to_string())?
        .sheet_names())
}

pub fn info(path: &Path) -> Result<Vec<SheetInfo>, String> {
    let mut book = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let mut sheets = vec![];
    for name in book.sheet_names() {
        let range = book.worksheet_range(&name).map_err(|e| e.to_string())?;
        let (height, width) = range.get_size();
        sheets.push(SheetInfo {
            sheet: name,
            rows: height.saturating_sub(1) as u64,
            columns: width,
        });
    }
    Ok(sheets)
}

/// The first row of the sheet's used range is the header.
pub fn open(path: &Path, sheet: String) -> Result<Table, String> {
    let mut book = open_workbook_auto(path).map_err(|e| e.to_string())?;
    let range: Range<Data> = book.worksheet_range(&sheet).map_err(|e| e.to_string())?;
    let mut rows = range.rows();
    let header = cells::header(
        rows.next()
            .map(|row| {
                row.iter()
                    .map(|cell| match cells::from_data(cell) {
                        Value::String(s) => s,
                        Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
    );
    let data: Vec<Vec<Value>> = rows
        .map(|row| row.iter().map(cells::from_data).collect())
        .collect();
    Ok(Table {
        sheet,
        header,
        total_rows: data.len() as u64,
        types: None,
        rows: Box::new(data.into_iter().map(Ok)),
        warnings: vec![],
    })
}

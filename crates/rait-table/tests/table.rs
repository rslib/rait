use rait_core::{ErrorCode, PageRange};
use rait_table::{Format, RowsRequest, info, rows, schema};
use serde_json::{Map, Value, json};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures")).join(name)
}

fn request(name: &str) -> RowsRequest {
    RowsRequest {
        path: fixture(name),
        sheet: None,
        rows: None,
        columns: vec![],
        max_chars: None,
    }
}

/// Spreadsheets store whole floats as integers; compare numbers by value.
fn normalized(row: &Map<String, Value>) -> Vec<Value> {
    row.values()
        .map(|v| match v.as_f64() {
            Some(f) => json!(f),
            None => v.clone(),
        })
        .collect()
}

#[test]
fn every_format_reads_the_same_data() {
    let expected = rows(&request("sample.csv")).unwrap();
    assert_eq!(expected.rows.len(), 20);
    assert_eq!(expected.total_rows, 25);
    assert_eq!(
        Value::Object(expected.rows[0].clone()),
        json!({"region": "EU", "quarter": "Q1", "revenue": 10.5, "units": 3, "active": false,
               "zip": "01001", "day": "2026-01-01", "note": "row 1"})
    );
    let csv_schema = schema(&fixture("sample.csv"), None).unwrap();
    let types: Vec<(String, String, u64)> = csv_schema
        .columns
        .iter()
        .map(|c| (c.column.clone(), c.kind.clone(), c.empty))
        .collect();
    for name in [
        "sample.tsv",
        "sample.xlsx",
        "sample.parquet",
        "sample-zstd.parquet",
    ] {
        let got = rows(&request(name)).unwrap();
        assert_eq!(got.total_rows, 25, "{name}");
        for (a, b) in got.rows.iter().zip(&expected.rows) {
            assert_eq!(normalized(a), normalized(b), "{name}");
        }
        let got: Vec<(String, String, u64)> = schema(&fixture(name), None)
            .unwrap()
            .columns
            .into_iter()
            .map(|c| (c.column, c.kind, c.empty))
            .collect();
        assert_eq!(got, types, "{name}");
    }
}

#[test]
fn workbooks_list_and_select_sheets() {
    let book = info(&fixture("sample.xlsx")).unwrap();
    assert_eq!(book.format, Format::Xlsx);
    let sheets: Vec<(&str, u64, usize)> = book
        .sheets
        .iter()
        .map(|s| (s.sheet.as_str(), s.rows, s.columns))
        .collect();
    assert_eq!(sheets, [("Sales", 25, 8), ("Notes", 2, 2)]);
    for sheet in ["Notes", "2", "notes"] {
        let got = rows(&RowsRequest {
            sheet: Some(sheet.into()),
            ..request("sample.xlsx")
        })
        .unwrap();
        assert_eq!(got.sheet, "Notes");
        assert_eq!(
            Value::Object(got.rows[1].clone()),
            json!({"key": "version", "value": 3})
        );
    }
    let err = rows(&RowsRequest {
        sheet: Some("Nope".into()),
        ..request("sample.xlsx")
    })
    .unwrap_err();
    assert!(err.hint.unwrap().contains("Sales, Notes"));
    let err = rows(&RowsRequest {
        sheet: Some("other".into()),
        ..request("sample.csv")
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn csv_edge_cases() {
    let got = rows(&request("edge.csv")).unwrap();
    let header: Vec<&String> = got.rows[0].keys().collect();
    assert_eq!(header, ["name", "name_2", "column_3", "amount, usd"]);
    assert_eq!(got.rows[1]["name_2"], json!("line one\nline two"));
    assert_eq!(got.rows[2]["name_2"], json!("She said \"hi\""));
    assert_eq!(
        Value::Object(got.rows[3].clone()),
        json!({"name": "dave", "name_2": "short", "column_3": null, "amount, usd": null})
    );
}

#[test]
fn rows_and_columns_are_selectable() {
    let got = rows(&RowsRequest {
        rows: Some(PageRange::Range(24, 25)),
        columns: vec!["units".into(), "1".into()],
        ..request("sample.parquet")
    })
    .unwrap();
    assert_eq!(got.range, "24-25");
    assert_eq!(
        Value::Object(got.rows[0].clone()),
        json!({"units": 72, "region": "APAC"})
    );
    let err = rows(&RowsRequest {
        rows: Some(PageRange::Single(26)),
        ..request("sample.parquet")
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidPageRange);
    assert!(err.hint.unwrap().contains("--rows"));
    let err = rows(&RowsRequest {
        columns: vec!["nope".into()],
        ..request("sample.csv")
    })
    .unwrap_err();
    assert!(err.hint.unwrap().starts_with("Columns: region, quarter"));
}

#[test]
fn max_chars_keeps_contiguous_rows_and_at_least_one() {
    let got = rows(&RowsRequest {
        max_chars: Some(300),
        ..request("sample.csv")
    })
    .unwrap();
    assert!(!got.rows.is_empty() && got.rows.len() < 20);
    assert_eq!(got.last_requested, 20);
    assert!(got.requested_chars > 300);
    let got = rows(&RowsRequest {
        max_chars: Some(1),
        ..request("sample.csv")
    })
    .unwrap();
    assert_eq!(got.rows.len(), 1);
    assert_eq!(got.range, "1");
}

#[test]
fn bad_inputs_are_typed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("broken.parquet");
    std::fs::write(&broken, "not parquet").unwrap();
    let err = info(&broken).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(err.detail.is_some());
    let other = dir.path().join("data.json");
    std::fs::write(&other, "{}").unwrap();
    assert!(info(&other).unwrap_err().message.contains("`.json`"));
    let empty = dir.path().join("empty.csv");
    std::fs::write(&empty, "a,b\n").unwrap();
    let got = rows(&RowsRequest {
        path: empty,
        ..request("sample.csv")
    })
    .unwrap();
    assert_eq!((got.total_rows, got.range.as_str()), (0, "none"));
    let broken_db = dir.path().join("broken.db");
    std::fs::write(&broken_db, "not a database").unwrap();
    let err = info(&broken_db).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
}

#[test]
fn sqlite_lists_tables_and_views_with_counts() {
    let book = info(&fixture("sample.sqlite")).unwrap();
    assert_eq!(book.format, Format::Sqlite);
    let sheets: Vec<(&str, u64, usize)> = book
        .sheets
        .iter()
        .map(|s| (s.sheet.as_str(), s.rows, s.columns))
        .collect();
    assert_eq!(
        sheets,
        [
            ("sales", 3, 4),
            ("notes", 2, 2),
            ("big_sales", 2, 4),
            ("weird \"quoted\" table", 1, 2),
        ]
    );
}

#[test]
fn sqlite_reports_declared_column_types() {
    let schema = schema(&fixture("sample.sqlite"), Some("sales")).unwrap();
    let types: Vec<(&str, &str)> = schema
        .columns
        .iter()
        .map(|c| (c.column.as_str(), c.kind.as_str()))
        .collect();
    assert_eq!(
        types,
        [
            ("id", "int"),
            ("amount, usd", "float"),
            ("region", "string"),
            ("note text", "string"),
        ]
    );
    assert_eq!(schema.columns[3].empty, 1);
}

#[test]
fn sqlite_selects_rows_range_and_columns() {
    let got = rows(&RowsRequest {
        sheet: Some("sales".into()),
        rows: Some(PageRange::Range(2, 3)),
        columns: vec!["region".into(), "note text".into()],
        ..request("sample.sqlite")
    })
    .unwrap();
    assert_eq!(got.range, "2-3");
    assert_eq!(
        Value::Object(got.rows[0].clone()),
        json!({"region": "US", "note text": null})
    );
    assert_eq!(
        Value::Object(got.rows[1].clone()),
        json!({"region": "APAC", "note text": "row 3"})
    );
}

#[test]
fn sqlite_blob_becomes_a_placeholder() {
    let got = rows(&RowsRequest {
        sheet: Some("notes".into()),
        ..request("sample.sqlite")
    })
    .unwrap();
    assert_eq!(got.rows[0]["payload"], json!("<blob 9 bytes>"));
    assert_eq!(got.rows[1]["payload"], Value::Null);
}

#[test]
fn sqlite_quotes_table_identifiers_needing_it() {
    let got = rows(&RowsRequest {
        sheet: Some("weird \"quoted\" table".into()),
        ..request("sample.sqlite")
    })
    .unwrap();
    assert_eq!(got.sheet, "weird \"quoted\" table");
    assert_eq!(
        Value::Object(got.rows[0].clone()),
        json!({"id": 1, "label": "hi"})
    );
}

#[test]
fn sqlite_view_is_selectable_like_a_table() {
    let got = rows(&RowsRequest {
        sheet: Some("big_sales".into()),
        ..request("sample.sqlite")
    })
    .unwrap();
    assert_eq!(got.total_rows, 2);
}

use rait_core::ErrorCode;
use rait_pdf::{DetectorChoice, PageRange, normalize_crlf};
use std::path::Path;

fn fixture() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/simple.pdf"
    ))
}

#[test]
fn reads_fixture_info() {
    let info = rait_pdf::info(fixture()).unwrap();
    assert_eq!(info.pages, 1);
    assert_eq!(info.page_size_points.unwrap().width, 612.0);
}

#[test]
fn parses_page_ranges() {
    assert_eq!(PageRange::parse(Some("4")).unwrap(), PageRange::Single(4));
    assert_eq!(
        PageRange::parse(Some("2-7")).unwrap(),
        PageRange::Range(2, 7)
    );
    assert_eq!(
        PageRange::parse(Some("2,")).unwrap_err().code,
        ErrorCode::InvalidPageRange
    );
    assert_eq!(
        PageRange::parse(Some("2-")).unwrap_err().code,
        ErrorCode::InvalidPageRange
    );
}

#[test]
fn normalizes_crlf_only() {
    assert_eq!(normalize_crlf("a\r\nb\n  c"), "a\nb\n  c");
}

#[test]
fn missing_file_is_file_not_found() {
    let err = rait_pdf::info(Path::new("missing.pdf")).unwrap_err();
    assert_eq!(err.code, ErrorCode::FileNotFound);
}

#[test]
fn extracts_fixture_text() {
    let req = rait_pdf::TextRequest {
        path: fixture().into(),
        pages: PageRange::Single(1),
        layout: false,
        tables: true,
        detector: DetectorChoice::Heuristic,
    };
    let text = rait_pdf::text(&req).unwrap();
    assert_eq!(text.pages.len(), 1);
    assert_eq!(text.pages[0].page, 1);
    assert!(text.pages[0].text.contains("RAIT simple PDF fixture"));
    assert!(!text.pages[0].text.contains('\u{c}'));
}

#[test]
fn parses_pdfinfo_report() {
    let report = "Title:           Caches: a survey\nAuthor:          \nPages:           12\nEncrypted:       yes (print:yes copy:no)\nPage size:       595.276 x 841.89 pts (A4)\n";
    let info = rait_pdf::parse_info(report).unwrap();
    assert_eq!(info.title.as_deref(), Some("Caches: a survey"));
    assert_eq!(info.author, None);
    assert_eq!(info.pages, 12);
    assert!(info.encrypted);
    assert_eq!(info.page_size_points.unwrap().height, 841.89);
}

#[test]
fn pdfinfo_report_without_pages_is_dependency_failed() {
    let err = rait_pdf::parse_info("Title: x\n").unwrap_err();
    assert_eq!(err.code, ErrorCode::DependencyFailed);
}

#[test]
fn page_range_outside_document_is_rejected() {
    assert_eq!(PageRange::Range(2, 3).bounds(5, "--pages").unwrap(), (2, 3));
    assert_eq!(PageRange::All.bounds(5, "--pages").unwrap(), (1, 5));
    assert_eq!(
        PageRange::Single(6).bounds(5, "--pages").unwrap_err().code,
        ErrorCode::InvalidPageRange
    );
}

#[test]
fn searches_fixture_by_page() {
    let req = rait_pdf::SearchRequest {
        path: fixture().into(),
        query: "FIXTURE".into(),
        pages: PageRange::All,
        ignore_case: true,
        max_chars: None,
    };
    let result = rait_pdf::search(&req).unwrap();
    assert_eq!(result.matches, 1);
    assert_eq!(result.hits[0].page, 1);
    let strict = rait_pdf::search(&rait_pdf::SearchRequest {
        ignore_case: false,
        ..req
    })
    .unwrap();
    assert_eq!(strict.matches, 0);
}

fn tables_fixture() -> std::path::PathBuf {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/tables.pdf"
    ))
    .into()
}

fn table_rows(page: u32) -> Vec<Vec<serde_json::Value>> {
    let found = rait_pdf::tables(&rait_pdf::TablesRequest {
        path: tables_fixture(),
        pages: PageRange::Single(page),
        detector: DetectorChoice::Heuristic,
    })
    .unwrap();
    found
        .tables
        .iter()
        .flat_map(|t| {
            let header: Vec<serde_json::Value> =
                t.rows[0].keys().map(|k| k.as_str().into()).collect();
            std::iter::once(header).chain(t.rows.iter().map(|r| r.values().cloned().collect()))
        })
        .collect()
}

#[test]
fn detects_borderless_table() {
    let rows = table_rows(1);
    assert_eq!(rows[0], ["Region", "Q1", "Q2", "Growth"]);
    assert_eq!(
        rows[2],
        [serde_json::json!("US"), 7.into(), 9.into(), "28.6%".into()]
    );
    assert_eq!(rows.len(), 5);
}

#[test]
fn joins_wrapped_cells_in_ruled_table() {
    let rows = table_rows(2);
    assert_eq!(rows[0], ["System", "Description", "Latency (ms)"]);
    assert_eq!(
        rows[1][1],
        "An embeddable persistent key-value store built for fast flash storage and heavy write loads"
    );
    assert_eq!(
        rows[3][1],
        "A storage engine with document-level concurrency and compression"
    );
}

#[test]
fn two_column_prose_is_not_a_table() {
    assert!(table_rows(3).is_empty());
}

#[test]
fn spanning_header_labels_each_column() {
    let found = rait_pdf::tables(&rait_pdf::TablesRequest {
        path: tables_fixture(),
        pages: PageRange::Single(4),
        detector: DetectorChoice::Heuristic,
    })
    .unwrap();
    assert_eq!(found.tables.len(), 2);
    let header: Vec<&String> = found.tables[0].rows[0].keys().collect();
    assert_eq!(
        header,
        ["Workload", "Latency (us) p50", "Latency (us) p99", "Ops"]
    );
    assert_eq!(found.tables[1].rows[2]["Value"], "LZ4");
}

#[test]
fn text_inlines_tables_unless_disabled() {
    let req = |tables| rait_pdf::TextRequest {
        path: tables_fixture(),
        pages: PageRange::Single(1),
        layout: false,
        tables,
        detector: DetectorChoice::Heuristic,
    };
    let with = rait_pdf::text(&req(true)).unwrap();
    assert_eq!(with.tables, 1);
    let text = &with.pages[0].text;
    assert!(text.contains(
        "| Region | Q1 | Q2 | Growth |\n| --- | --- | --- | --- |\n| EU | 10 | 12 | 20% |"
    ));
    assert!(text.starts_with("Quarterly Results\n\nThis report summarizes"));
    assert!(text.contains("Overall bookings grew"));
    let without = rait_pdf::text(&req(false)).unwrap();
    assert_eq!(without.tables, 0);
    assert!(!without.pages[0].text.contains("| Region"));
}

/// Runs only when the table model is installed (`rait models fetch`).
#[test]
fn model_finds_the_same_tables() {
    if rait_vision::models::installed(&rait_vision::models::TATR).is_none() {
        eprintln!("skipped: table model not installed");
        return;
    }
    let found = rait_pdf::tables(&rait_pdf::TablesRequest {
        path: tables_fixture(),
        pages: PageRange::parse(None).unwrap(),
        detector: DetectorChoice::Model,
    })
    .unwrap();
    assert_eq!(found.detector, rait_pdf::Detector::Tatr);
    let pages: Vec<u32> = found.tables.iter().map(|t| t.page).collect();
    assert_eq!(pages, [1, 2, 4, 4]);
    let header: Vec<&String> = found.tables[0].rows[0].keys().collect();
    assert_eq!(header, ["Region", "Q1", "Q2", "Growth"]);
}

#[test]
fn model_detector_without_model_says_how_to_install() {
    if rait_vision::models::installed(&rait_vision::models::TATR).is_some() {
        return;
    }
    let err = rait_pdf::tables(&rait_pdf::TablesRequest {
        path: tables_fixture(),
        pages: PageRange::Single(1),
        detector: DetectorChoice::Model,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::DependencyMissing);
    assert!(err.hint.as_deref().unwrap().contains("rait models fetch"));
}

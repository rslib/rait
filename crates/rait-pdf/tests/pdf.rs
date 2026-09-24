use rait_core::ErrorCode;
use rait_pdf::{PageRange, normalize_crlf};
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
    assert_eq!(PageRange::Range(2, 3).bounds(5).unwrap(), (2, 3));
    assert_eq!(PageRange::All.bounds(5).unwrap(), (1, 5));
    assert_eq!(
        PageRange::Single(6).bounds(5).unwrap_err().code,
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

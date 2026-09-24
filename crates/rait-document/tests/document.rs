use rait_core::{ErrorCode, PageRange};
use rait_document::{DocText, Format, TextRequest, text};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures")).join(name)
}

fn read(path: PathBuf, parts: PageRange) -> DocText {
    text(&TextRequest { path, parts }).unwrap()
}

fn all(name: &str) -> Vec<String> {
    read(fixture(name), PageRange::All)
        .parts
        .into_iter()
        .map(|part| part.text)
        .collect()
}

#[test]
fn docx_epub_and_html_from_one_source_agree() {
    let docx = all("sample.docx");
    assert_eq!(docx.len(), 2);
    assert_eq!(all("sample.epub"), docx);
    assert_eq!(all("sample.html"), docx);
    assert_eq!(
        docx[0],
        "# Introduction\n\nThis report has two sections.\n\n- First point\n  - Sub point\n- Second point\n\n1. Step one\n1. Step two"
    );
    assert!(docx[1].contains("| Region | Q1 | Q2 |\n| --- | --- | --- |\n| EU | 10 | 12 |"));
    assert!(docx[1].contains("## Details"));
}

#[test]
fn docx_edge_cases() {
    let out = read(fixture("edge.docx"), PageRange::All);
    assert_eq!(out.format, Format::Docx);
    assert_eq!(out.parts[0].text, "# Edge Cases");
    let body = &out.parts[1].text;
    assert!(
        body.starts_with("# Merged Cells\n\nRun joined text\n\n"),
        "{body}"
    );
    assert!(body.contains(
        "| APAC | 5 | 6 |\n|  | 7 | 8 |\n| Total |  | 26 |\n| Line one<br>Line two | a\\|b | in1<br>in2 |"
    ));
    assert!(body.contains("- Bullet item\n  - Nested bullet\n\n1. Numbered item"));
    assert_eq!(body.matches("Boxed text").count(), 1);
    assert!(body.contains("inserted words"));
    assert!(!body.contains("deleted words"));
}

#[test]
fn pptx_parts_are_slides_in_presentation_order() {
    let slides = all("sample.pptx");
    assert_eq!(slides.len(), 3);
    assert!(
        slides[0].starts_with("# Introduction\n\nThis report has two sections.\n\n- First point")
    );
    assert!(slides[1].contains("| EU | 10 | 12 |"));
    assert_eq!(slides[2], "Details\n\nClosing notes.");
}

#[test]
fn parts_select_a_range() {
    let out = read(fixture("sample.docx"), PageRange::Single(2));
    assert_eq!((out.total_parts, out.range.as_str()), (2, "2"));
    assert_eq!(out.parts.len(), 1);
    assert_eq!(out.parts[0].part, 2);
    let err = text(&TextRequest {
        path: fixture("sample.docx"),
        parts: PageRange::Single(3),
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidPageRange);
    assert!(err.hint.unwrap().contains("--parts"));
}

#[test]
fn html_spans_keep_columns_aligned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.html");
    std::fs::write(
        &path,
        "<table><tr><th rowspan=2>A</th><th colspan=2>B</th></tr><tr><td>1</td><td>2</td></tr>\
         <tr><td>x</td><td>y</td><td>z</td></tr></table><script>ignored()</script>",
    )
    .unwrap();
    let out = read(path, PageRange::All);
    assert_eq!(
        out.parts[0].text,
        "| A | B |  |\n| --- | --- | --- |\n|  | 1 | 2 |\n| x | y | z |"
    );
}

#[test]
fn bad_inputs_are_typed_errors() {
    let dir = tempfile::tempdir().unwrap();
    let corrupt = dir.path().join("broken.docx");
    std::fs::write(&corrupt, "not a zip").unwrap();
    let err = text(&TextRequest {
        path: corrupt,
        parts: PageRange::All,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(err.detail.is_some());
    let other = dir.path().join("notes.txt");
    std::fs::write(&other, "x").unwrap();
    let err = text(&TextRequest {
        path: other,
        parts: PageRange::All,
    })
    .unwrap_err();
    assert!(err.message.contains("`.txt`"));
    let err = text(&TextRequest {
        path: dir.path().join("missing.docx"),
        parts: PageRange::All,
    })
    .unwrap_err();
    assert_eq!(err.code, ErrorCode::FileNotFound);
}

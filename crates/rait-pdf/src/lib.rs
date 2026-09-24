mod fallback;
mod files;
mod layout;
mod tables;

pub use files::{
    ImageFormat, MergeRequest, Merged, OcrRequest, Ocred, OutputFiles, RenderRequest, SplitRequest,
    merge, ocr, output_stem, render, split,
};
pub use rait_core::PageRange;
pub use rait_core::exec::{Dependency, ExecError};

use rait_core::exec;
use rait_core::{ErrorCode, ErrorInfo, check_input};
use rayon::prelude::*;
use serde::Serialize;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// Fewest pages per `pdftotext` run when a range is split across threads;
/// smaller chunks spend more on process start-up than they save.
const MIN_CHUNK_PAGES: u32 = 8;

/// `pdftotext` ends every page with a form feed.
const PAGE_BREAK: char = '\u{c}';

const PDFINFO: &str = "pdfinfo";
const PDFTOTEXT: &str = "pdftotext";
const PDFTOPPM: &str = "pdftoppm";
const QPDF: &str = "qpdf";
const OCRMYPDF: &str = "ocrmypdf";

const POPPLER_INSTALL: &str = "macOS: brew install poppler; Debian/Ubuntu: sudo apt install poppler-utils; Fedora: sudo dnf install poppler-utils";

pub const DEPENDENCIES: &[Dependency] = &[
    Dependency {
        program: PDFINFO,
        capabilities: &["pdf.info", "pdf.text", "pdf.search", "pdf.render"],
        fallback: Some("lopdf"),
        install: POPPLER_INSTALL,
        version_flag: "-v",
    },
    Dependency {
        program: PDFTOTEXT,
        capabilities: &["pdf.text", "pdf.search"],
        fallback: Some("lopdf"),
        install: POPPLER_INSTALL,
        version_flag: "-v",
    },
    Dependency {
        program: PDFTOPPM,
        capabilities: &["pdf.render"],
        fallback: None,
        install: POPPLER_INSTALL,
        version_flag: "-v",
    },
    Dependency {
        program: QPDF,
        capabilities: &["pdf.split", "pdf.merge"],
        fallback: None,
        install: "macOS: brew install qpdf; Debian/Ubuntu: sudo apt install qpdf; Fedora: sudo dnf install qpdf",
        version_flag: "--version",
    },
    Dependency {
        program: OCRMYPDF,
        capabilities: &["pdf.ocr"],
        fallback: None,
        install: "macOS: brew install ocrmypdf; Debian/Ubuntu: sudo apt install ocrmypdf; Fedora: sudo dnf install ocrmypdf",
        version_flag: "--version",
    },
];

#[derive(Clone, Debug)]
pub struct TextRequest {
    pub path: PathBuf,
    pub pages: PageRange,
    pub layout: bool,
    /// Replace detected tables with Markdown tables. Ignored with `layout`.
    pub tables: bool,
}

#[derive(Clone, Debug)]
pub struct TablesRequest {
    pub path: PathBuf,
    pub pages: PageRange,
}

/// A table detected on a page, with rows keyed by its first row.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FoundTable {
    pub page: u32,
    /// 1-based position among the tables found in the selected pages.
    pub table: u32,
    pub rows: Vec<serde_json::Map<String, serde_json::Value>>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PdfTables {
    /// The requested `--pages` selector, `all` by default.
    pub range: String,
    pub total_pages: u32,
    pub tables: Vec<FoundTable>,
}

#[derive(Clone, Debug)]
pub struct SearchRequest {
    pub path: PathBuf,
    pub query: String,
    pub pages: PageRange,
    pub ignore_case: bool,
    /// Stop collecting hit lines once their total length would pass this many chars.
    pub max_chars: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageSizePoints {
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PdfInfo {
    pub pages: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    pub encrypted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub page_size_points: Option<PageSizePoints>,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PageText {
    pub page: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PdfText {
    /// The requested `--pages` selector, `all` by default.
    pub range: String,
    pub total_pages: u32,
    /// Tables detected and rendered as Markdown inside the page text.
    pub tables: u32,
    pub pages: Vec<PageText>,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    pub page: u32,
    pub line: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SearchResult {
    pub matches: usize,
    pub hits: Vec<Hit>,
    /// Chars in all matching lines, including hits left out by `max_chars`.
    #[serde(skip)]
    pub matched_chars: usize,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

/// Uses `pdfinfo`, or the built-in lopdf backend with a warning when it is missing.
pub fn info(path: &Path) -> Result<PdfInfo, ErrorInfo> {
    check_input(path)?;
    match exec::run(
        PDFINFO,
        [OsStr::new("-enc"), OsStr::new("UTF-8"), path.as_os_str()],
    ) {
        Ok(out) => parse_info(&String::from_utf8_lossy(&out.stdout)),
        Err(ExecError::Missing(_)) => {
            let mut info = fallback::info(path)?;
            info.warnings.push(FALLBACK_WARNING.into());
            Ok(info)
        }
        Err(err) => Err(backend_error(err)),
    }
}

/// Parse the default `pdfinfo` report.
pub fn parse_info(report: &str) -> Result<PdfInfo, ErrorInfo> {
    let mut info = PdfInfo {
        pages: 0,
        title: None,
        author: None,
        encrypted: false,
        page_size_points: None,
        warnings: vec![],
    };
    let mut pages = None;
    for line in report.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key {
            "Title" if !value.is_empty() => info.title = Some(value.into()),
            "Author" if !value.is_empty() => info.author = Some(value.into()),
            "Pages" => pages = value.parse().ok(),
            "Encrypted" => info.encrypted = value.starts_with("yes"),
            "Page size" => info.page_size_points = parse_page_size(value),
            _ => {}
        }
    }
    info.pages = pages.ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::DependencyFailed,
            "`pdfinfo` did not report a page count.",
            "Run `rait doctor` to check the Poppler install.".to_string(),
        )
    })?;
    Ok(info)
}

fn parse_page_size(value: &str) -> Option<PageSizePoints> {
    let mut words = value.split_whitespace();
    let width = words.next()?.parse().ok()?;
    let height = words.nth(1)?.parse().ok()?;
    Some(PageSizePoints { width, height })
}

pub fn text(req: &TextRequest) -> Result<PdfText, ErrorInfo> {
    let info = info(&req.path)?;
    let (first, last) = req.pages.bounds(info.pages, "--pages")?;
    let mut warnings = info.warnings;
    let text = extract(&req.path, first, last, req.layout, &mut warnings)?;
    let mut pages: Vec<PageText> = (first..=last)
        .zip(text.split_terminator(PAGE_BREAK))
        .map(|(page, text)| PageText {
            page,
            text: text.into(),
        })
        .collect();
    let mut table_count = 0;
    if req.tables && !req.layout && !warnings.iter().any(|w| w == FALLBACK_WARNING) {
        let layouts = bbox(&req.path, first, last)?;
        let detected: Vec<_> = layouts.par_iter().map(tables::detect).collect();
        for ((page, layout), found) in pages.iter_mut().zip(&layouts).zip(detected) {
            if !found.is_empty() {
                table_count += found.len() as u32;
                page.text = tables::render_page(layout, &found);
            }
        }
    }
    if pages.iter().all(|page| page.text.trim().is_empty()) {
        warnings.push(
            "No text found. The pages may be scanned images; `rait pdf ocr` adds a text layer."
                .into(),
        );
    }
    Ok(PdfText {
        range: req.pages.to_string(),
        total_pages: info.pages,
        tables: table_count,
        pages,
        warnings,
    })
}

/// Tables detected from word positions. Needs `pdftotext`; there is no fallback.
pub fn tables(req: &TablesRequest) -> Result<PdfTables, ErrorInfo> {
    let info = info(&req.path)?;
    let (first, last) = req.pages.bounds(info.pages, "--pages")?;
    let layouts = bbox(&req.path, first, last)?;
    let detected: Vec<_> = layouts.par_iter().map(tables::detect).collect();
    let mut found = vec![];
    for (page, tables) in (first..).zip(detected) {
        for table in tables {
            found.push(FoundTable {
                page,
                table: found.len() as u32 + 1,
                rows: rait_core::cells::keyed(&table.rows),
            });
        }
    }
    Ok(PdfTables {
        range: req.pages.to_string(),
        total_pages: info.pages,
        tables: found,
    })
}

fn bbox(path: &Path, first: u32, last: u32) -> Result<Vec<layout::Page>, ErrorInfo> {
    let mut pages = vec![];
    for out in pdftotext(path, first, last, &["-bbox-layout"]).map_err(backend_error)? {
        let parsed = layout::parse(&String::from_utf8_lossy(&out)).map_err(|detail| {
            ErrorInfo::new(
                ErrorCode::DependencyFailed,
                "`pdftotext` returned unreadable word positions.",
                "Run `rait doctor` to check the Poppler install.".to_string(),
            )
            .with_detail(detail)
        })?;
        pages.extend(parsed);
    }
    Ok(pages)
}

/// Run `pdftotext` over `first..=last`, split into page chunks that run in
/// parallel on the current thread pool. Outputs are in page order.
fn pdftotext(
    path: &Path,
    first: u32,
    last: u32,
    flags: &[&str],
) -> Result<Vec<Vec<u8>>, ExecError> {
    chunks(first, last)
        .into_par_iter()
        .map(|(from, to)| {
            let mut args: Vec<OsString> = flags.iter().map(OsString::from).collect();
            args.extend([
                "-enc".into(),
                "UTF-8".into(),
                "-f".into(),
                from.to_string().into(),
                "-l".into(),
                to.to_string().into(),
                path.into(),
                "-".into(),
            ]);
            exec::run(PDFTOTEXT, args).map(|out| out.stdout)
        })
        .collect()
}

/// Split a page range into at most one chunk per thread, each at least
/// [`MIN_CHUNK_PAGES`] long.
fn chunks(first: u32, last: u32) -> Vec<(u32, u32)> {
    let pages = last - first + 1;
    let threads = rayon::current_num_threads().max(1) as u32;
    let count = (pages / MIN_CHUNK_PAGES).clamp(1, threads);
    let size = pages.div_ceil(count);
    (0..count)
        .map(|i| (first + i * size, (first + (i + 1) * size - 1).min(last)))
        .filter(|(from, to)| from <= to)
        .collect()
}

pub fn search(req: &SearchRequest) -> Result<SearchResult, ErrorInfo> {
    if req.query.is_empty() {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "Search query is empty.",
            "Pass a non-empty <QUERY>.".to_string(),
        ));
    }
    let info = info(&req.path)?;
    let (first, last) = req.pages.bounds(info.pages, "--pages")?;
    let mut warnings = info.warnings;
    let text = extract(&req.path, first, last, false, &mut warnings)?;
    let fold = |s: &str| {
        if req.ignore_case {
            s.to_lowercase()
        } else {
            s.to_owned()
        }
    };
    let query = fold(&req.query);
    let mut result = SearchResult {
        matches: 0,
        hits: vec![],
        matched_chars: 0,
        warnings,
    };
    let mut budget = req.max_chars.unwrap_or(usize::MAX);
    for (page, body) in (first..).zip(text.split(PAGE_BREAK)) {
        for line in body.lines().map(str::trim) {
            if !fold(line).contains(&query) {
                continue;
            }
            result.matches += 1;
            let chars = line.chars().count();
            result.matched_chars += chars;
            if chars <= budget && result.hits.len() + 1 == result.matches {
                budget -= chars;
                result.hits.push(Hit {
                    page,
                    line: line.into(),
                });
            }
        }
    }
    Ok(result)
}

fn extract(
    path: &Path,
    first: u32,
    last: u32,
    layout: bool,
    warnings: &mut Vec<String>,
) -> Result<String, ErrorInfo> {
    let flags: &[&str] = if layout { &["-layout"] } else { &[] };
    let out = match pdftotext(path, first, last, flags) {
        Ok(chunks) => chunks.concat(),
        Err(ExecError::Missing(_)) => {
            if !warnings.iter().any(|w| w == FALLBACK_WARNING) {
                warnings.push(FALLBACK_WARNING.into());
            }
            if layout {
                warnings.push("--layout needs Poppler, so the text has no layout.".into());
            }
            fallback::extract(path, first, last)?.into_bytes()
        }
        Err(err) => return Err(backend_error(err)),
    };
    Ok(normalize_crlf(&String::from_utf8_lossy(&out)))
}

const FALLBACK_WARNING: &str = "Poppler is not installed, so the built-in PDF reader was used and line breaks may be lost. Run `rait doctor` for install commands.";

fn not_a_pdf() -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        "The file is not a valid PDF or is damaged.",
        "Check the file, or run `rait pdf info <file>` on a known good copy.".to_string(),
    )
}

fn encrypted() -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::EncryptedPdf,
        "The PDF is encrypted.",
        "Provide an unencrypted copy.".to_string(),
    )
}

pub fn normalize_crlf(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn backend_error(err: ExecError) -> ErrorInfo {
    let stderr = err.stderr().to_lowercase();
    if stderr.contains("password")
        || matches!(
            err,
            ExecError::Failed {
                program: OCRMYPDF,
                status: 8,
                ..
            }
        )
    {
        return encrypted();
    }
    if let ExecError::Failed {
        program, status, ..
    } = &err
        && matches!(
            (*program, *status),
            (PDFINFO | PDFTOTEXT | PDFTOPPM, 1) | (QPDF | OCRMYPDF, 2)
        )
    {
        return not_a_pdf().with_detail(err.to_string());
    }
    err.into()
}

#[cfg(test)]
mod tests {
    use super::chunks;

    #[test]
    fn chunks_cover_the_range_in_order() {
        for (first, last) in [(1, 1), (1, 7), (3, 240), (1, 1000)] {
            let parts = chunks(first, last);
            assert_eq!(parts[0].0, first);
            assert_eq!(parts[parts.len() - 1].1, last);
            for pair in parts.windows(2) {
                assert_eq!(pair[0].1 + 1, pair[1].0);
            }
            assert!(
                parts.len() == 1
                    || parts
                        .iter()
                        .all(|(a, b)| b - a + 1 >= super::MIN_CHUNK_PAGES - 1)
            );
        }
        assert_eq!(chunks(1, 7), [(1, 7)]);
    }
}

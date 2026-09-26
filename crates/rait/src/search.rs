//! `rait search`: literal search in any supported file, with each hit located
//! by page, document part or table row.

use rait_core::{ErrorCode, ErrorInfo, PageRange};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hit {
    /// `page 3`, `part 2`, `row 14` or `sales row 14`.
    pub at: String,
    pub line: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Found {
    pub format: String,
    pub matches: usize,
    pub hits: Vec<Hit>,
    /// Chars in all matching lines, including hits left out by the limit.
    #[serde(skip)]
    pub matched_chars: usize,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

/// Collects matching lines up to a character budget, counting the rest.
struct Collector<'a> {
    query: String,
    ignore_case: bool,
    budget: Option<usize>,
    found: &'a mut Found,
}

impl Collector<'_> {
    fn offer(&mut self, at: impl FnOnce() -> String, line: &str) {
        let hit = if self.ignore_case {
            line.to_lowercase().contains(&self.query)
        } else {
            line.contains(&self.query)
        };
        if !hit {
            return;
        }
        let chars = line.chars().count();
        self.found.matches += 1;
        self.found.matched_chars += chars;
        let used: usize = self.found.hits.iter().map(|h| h.line.chars().count()).sum();
        if self
            .budget
            .is_none_or(|max| self.found.hits.is_empty() || used + chars <= max)
        {
            self.found.hits.push(Hit {
                at: at(),
                line: line.trim().to_string(),
            });
        }
    }
}

pub fn search(
    path: &Path,
    query: &str,
    ignore_case: bool,
    max_chars: Option<usize>,
) -> Result<Found, ErrorInfo> {
    let is_pdf = path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        let result = rait_pdf::search(&rait_pdf::SearchRequest {
            path: path.into(),
            query: query.into(),
            pages: PageRange::parse(None)?,
            ignore_case,
            max_chars,
        })?;
        return Ok(Found {
            format: "pdf".into(),
            matches: result.matches,
            hits: result
                .hits
                .into_iter()
                .map(|h| Hit {
                    at: format!("page {}", h.page),
                    line: h.line,
                })
                .collect(),
            matched_chars: result.matched_chars,
            warnings: result.warnings,
        });
    }
    let mut found = Found {
        format: String::new(),
        matches: 0,
        hits: vec![],
        matched_chars: 0,
        warnings: vec![],
    };
    let mut collector = Collector {
        query: if ignore_case {
            query.to_lowercase()
        } else {
            query.into()
        },
        ignore_case,
        budget: max_chars,
        found: &mut found,
    };
    if let Some(format) = rait_document::Format::from_path(path) {
        let doc = rait_document::text(&rait_document::TextRequest {
            path: path.into(),
            parts: PageRange::parse(None)?,
        })?;
        for part in &doc.parts {
            for line in part.text.lines() {
                collector.offer(|| format!("part {}", part.part), line);
            }
        }
        found.format = serde_json::to_value(format)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        found.warnings = doc.warnings;
        return Ok(found);
    }
    if rait_table::Format::from_path(path).is_some() {
        let info = rait_table::info(path)?;
        let named = info.sheets.len() > 1;
        for sheet in &info.sheets {
            rait_table::scan(path, Some(&sheet.sheet), |row, header, cells| {
                let line = cells
                    .iter()
                    .zip(header)
                    .filter(|(v, _)| !v.is_null())
                    .map(|(v, h)| match v {
                        Value::String(s) => format!("{h}: {s}"),
                        v => format!("{h}: {v}"),
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                collector.offer(
                    || match named {
                        true => format!("{} row {row}", sheet.sheet),
                        false => format!("row {row}"),
                    },
                    &line,
                );
                true
            })?;
        }
        found.format = serde_json::to_value(info.format)
            .ok()
            .and_then(|v| v.as_str().map(String::from))
            .unwrap_or_default();
        return Ok(found);
    }
    Err(ErrorInfo::new(
        ErrorCode::InvalidInput,
        "Search reads PDF, document, notebook and table files only.",
        "Use a .pdf, .docx, .pptx, .epub, .html, .ipynb, .csv, .xlsx, .parquet or .sqlite file."
            .to_string(),
    ))
}

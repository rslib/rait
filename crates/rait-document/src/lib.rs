mod docx;
mod epub;
mod html;
mod markdown;
mod pptx;
mod xml;

use markdown::{Block, render, split_at_h1};
use rait_core::{ErrorCode, ErrorInfo, PageRange, check_input};
use serde::Serialize;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use zip::ZipArchive;

/// Largest decompressed file read from a document, against zip bombs.
const MAX_ENTRY_BYTES: u64 = 256 * 1024 * 1024;

pub const EXTENSIONS: &[&str] = &["docx", "pptx", "epub", "html", "htm", "xhtml"];

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Docx,
    Pptx,
    Epub,
    Html,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "docx" => Some(Self::Docx),
            "pptx" => Some(Self::Pptx),
            "epub" => Some(Self::Epub),
            "html" | "htm" | "xhtml" => Some(Self::Html),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Docx => "DOCX",
            Self::Pptx => "PPTX",
            Self::Epub => "EPUB",
            Self::Html => "HTML",
        }
    }
}

#[derive(Clone, Debug)]
pub struct TextRequest {
    pub path: PathBuf,
    pub parts: PageRange,
}

/// One unit of a document: a section starting at a level-1 heading (DOCX, HTML),
/// a slide (PPTX) or a chapter (EPUB). Text is GitHub-flavoured Markdown.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Part {
    pub part: u32,
    pub text: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DocText {
    pub format: Format,
    /// The requested `--parts` selector, `all` by default.
    pub range: String,
    pub total_parts: u32,
    pub parts: Vec<Part>,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

pub fn text(req: &TextRequest) -> Result<DocText, ErrorInfo> {
    check_input(&req.path)?;
    let format = Format::from_path(&req.path).ok_or_else(|| unsupported(&req.path))?;
    let all = parts(&req.path, format).map_err(|detail| {
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            format!(
                "The file is not a valid {}, or it is encrypted or damaged.",
                format.name()
            ),
            "Check the file, or open and re-save it.".to_string(),
        )
        .with_detail(detail)
    })?;
    let total = all.len() as u32;
    let (first, last) = req.parts.bounds(total, "--parts")?;
    let parts: Vec<Part> = (first..=last)
        .zip(all.into_iter().skip(first as usize - 1))
        .map(|(part, text)| Part { part, text })
        .collect();
    let mut warnings = vec![];
    if parts.iter().all(|part| part.text.trim().is_empty()) {
        warnings.push("No text found in the selected parts.".to_string());
    }
    Ok(DocText {
        format,
        range: req.parts.to_string(),
        total_parts: total,
        parts,
        warnings,
    })
}

fn parts(path: &Path, format: Format) -> Result<Vec<String>, String> {
    let rendered = |parts: Vec<Vec<Block>>| parts.iter().map(|part| render(part)).collect();
    match format {
        Format::Html => {
            let html = read_limited(File::open(path).map_err(|err| err.to_string())?)?;
            Ok(rendered(split_at_h1(html::blocks(&html))))
        }
        Format::Docx => {
            let mut zip = open_zip(path)?;
            let document = entry(&mut zip, "word/document.xml")?;
            let styles = entry(&mut zip, "word/styles.xml").ok();
            let numbering = entry(&mut zip, "word/numbering.xml").ok();
            let blocks = docx::blocks(&document, styles.as_deref(), numbering.as_deref())?;
            Ok(rendered(split_at_h1(blocks)))
        }
        Format::Pptx => {
            let mut zip = open_zip(path)?;
            let presentation = entry(&mut zip, "ppt/presentation.xml")?;
            let rels = entry(&mut zip, "ppt/_rels/presentation.xml.rels")?;
            let mut slides = vec![];
            for slide in pptx::slide_paths(&presentation, &rels)? {
                slides.push(render(&pptx::slide_blocks(&entry(&mut zip, &slide)?)?));
            }
            Ok(slides)
        }
        Format::Epub => {
            let mut zip = open_zip(path)?;
            let opf_path = epub::package_path(&entry(&mut zip, "META-INF/container.xml")?)?;
            let opf = entry(&mut zip, &opf_path)?;
            let mut chapters = vec![];
            for doc in epub::spine(&opf, &opf_path)? {
                let text = render(&html::blocks(&entry(&mut zip, &doc)?));
                if !text.is_empty() {
                    chapters.push(text);
                }
            }
            Ok(chapters)
        }
    }
}

fn open_zip(path: &Path) -> Result<ZipArchive<File>, String> {
    let file = File::open(path).map_err(|err| err.to_string())?;
    ZipArchive::new(file).map_err(|err| err.to_string())
}

fn entry(zip: &mut ZipArchive<File>, name: &str) -> Result<String, String> {
    let file = zip.by_name(name).map_err(|err| format!("{name}: {err}"))?;
    read_limited(file)
}

fn read_limited(reader: impl Read) -> Result<String, String> {
    let mut bytes = vec![];
    reader
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| err.to_string())?;
    if bytes.len() as u64 > MAX_ENTRY_BYTES {
        return Err(format!("a part is larger than {MAX_ENTRY_BYTES} bytes"));
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// Resolve `target` relative to the directory of `base`, both zip paths.
fn resolve(base: &str, target: &str) -> String {
    if let Some(absolute) = target.strip_prefix('/') {
        return absolute.to_string();
    }
    let mut parts: Vec<&str> = base.split('/').collect();
    parts.pop();
    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            _ => parts.push(segment),
        }
    }
    parts.join("/")
}

fn unsupported(path: &Path) -> ErrorInfo {
    let ext = path
        .extension()
        .map(|ext| format!("`.{}`", ext.to_string_lossy()))
        .unwrap_or_else(|| "without an extension".into());
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        format!("Files {ext} are not supported."),
        "Use .docx, .pptx, .epub or .html, or `rait pdf text` for PDFs.".to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::resolve;

    #[test]
    fn resolves_zip_paths() {
        assert_eq!(
            resolve("ppt/presentation.xml", "slides/slide1.xml"),
            "ppt/slides/slide1.xml"
        );
        assert_eq!(
            resolve("OEBPS/content.opf", "../text/a.xhtml"),
            "text/a.xhtml"
        );
        assert_eq!(
            resolve("ppt/presentation.xml", "/ppt/slides/s.xml"),
            "ppt/slides/s.xml"
        );
    }
}

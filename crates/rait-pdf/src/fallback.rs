use crate::{PAGE_BREAK, PageSizePoints, PdfInfo, encrypted};
use lopdf::{Document, Object, decode_text_string};
use rait_core::ErrorInfo;
use std::path::Path;

const TEXT_LIMIT_BYTES: usize = 64 * 1024 * 1024;

pub fn info(path: &Path) -> Result<PdfInfo, ErrorInfo> {
    let doc = load(path)?;
    Ok(PdfInfo {
        pages: doc.get_pages().len() as u32,
        title: metadata_text(&doc, b"Title"),
        author: metadata_text(&doc, b"Author"),
        encrypted: false,
        page_size_points: first_page_size(&doc),
        warnings: vec![],
    })
}

/// Text of pages `first..=last`, each followed by a form feed like `pdftotext`.
pub fn extract(path: &Path, first: u32, last: u32) -> Result<String, ErrorInfo> {
    let doc = load(path)?;
    let mut text = String::new();
    for page in first..=last {
        text.push_str(
            &doc.extract_text_with_limit(&[page], TEXT_LIMIT_BYTES)
                .map_err(pdf_error)?,
        );
        text.push(PAGE_BREAK);
    }
    Ok(text)
}

fn load(path: &Path) -> Result<Document, ErrorInfo> {
    let doc = Document::load(path).map_err(pdf_error)?;
    if doc.is_encrypted() {
        return Err(encrypted());
    }
    Ok(doc)
}

fn metadata_text(doc: &Document, key: &[u8]) -> Option<String> {
    let object = doc.trailer.get(b"Info").ok()?;
    let (_, object) = doc.dereference(object).ok()?;
    let value = object.as_dict().ok()?.get(key).ok()?;
    decode_text_string(value)
        .ok()
        .filter(|text| !text.is_empty())
}

fn first_page_size(doc: &Document) -> Option<PageSizePoints> {
    let page_id = *doc.get_pages().values().next()?;
    let page = doc.get_object(page_id).ok()?.as_dict().ok()?;
    let media_box = page.get(b"MediaBox").ok()?.as_array().ok()?;
    let [x0, y0, x1, y1] = media_box.as_slice() else {
        return None;
    };
    Some(PageSizePoints {
        width: (number(x1)? - number(x0)?).abs(),
        height: (number(y1)? - number(y0)?).abs(),
    })
}

fn number(object: &Object) -> Option<f64> {
    match object {
        Object::Integer(n) => Some(*n as f64),
        Object::Real(n) => Some((*n).into()),
        _ => None,
    }
}

fn pdf_error(err: lopdf::Error) -> ErrorInfo {
    let message = err.to_string();
    let lower = message.to_lowercase();
    if lower.contains("encrypt") || lower.contains("password") {
        return encrypted();
    }
    crate::not_a_pdf().with_detail(message)
}

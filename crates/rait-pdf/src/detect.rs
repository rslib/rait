//! Choosing the table detector: Table Transformer regions when the model is
//! installed, the geometry heuristic otherwise. Inside each region the
//! structure model gives rows and columns when it is installed; otherwise the
//! gaps between words do.

use crate::layout::Page;
use crate::structure;
use crate::tables::{self, Table};
use crate::{PDFTOPPM, backend_error};
use rait_core::exec::{self, ExecError};
use rait_core::{ErrorCode, ErrorInfo};
use rait_vision::{INPUT_SIZE, STRUCTURE_SIZE, TableDetector, TableStructure, models};
use rayon::prelude::*;
use serde::Serialize;
use std::ffi::OsString;
use std::path::Path;
use std::sync::OnceLock;

/// Which table detector a caller wants.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum DetectorChoice {
    /// The model when installed, else the heuristic.
    #[default]
    Auto,
    Model,
    Heuristic,
}

/// Which table detector produced a result.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Detector {
    Tatr,
    Heuristic,
}

static TATR: OnceLock<Result<TableDetector, ErrorInfo>> = OnceLock::new();

fn tatr() -> Option<&'static Result<TableDetector, ErrorInfo>> {
    let path = models::installed(&models::TATR)?;
    Some(TATR.get_or_init(|| TableDetector::load(&path)))
}

static STRUCTURE: OnceLock<Result<TableStructure, ErrorInfo>> = OnceLock::new();

/// Points added around a table before cropping it for the structure model.
const CROP_PAD: f64 = 8.0;
/// Highest resolution for a table crop. Small tables stay small instead of
/// being blown up to [`STRUCTURE_SIZE`]; the structure model reads text at
/// the size it was trained on better.
const MAX_CROP_DPI: f64 = 200.0;

// ponytail: a structure model that fails to load falls back to word gaps
// silently; warn if that ever happens in practice.
fn structure() -> Option<&'static TableStructure> {
    let path = models::installed(&models::TATR_STRUCTURE)?;
    STRUCTURE
        .get_or_init(|| TableStructure::load(&path))
        .as_ref()
        .ok()
}

/// Tables for each page in `layouts`, which start at page `first`.
pub fn find(
    path: &Path,
    first: u32,
    layouts: &[Page],
    choice: DetectorChoice,
    warnings: &mut Vec<String>,
) -> Result<(Vec<Vec<Table>>, Detector), ErrorInfo> {
    let heuristic = || {
        (
            layouts.par_iter().map(tables::detect).collect(),
            Detector::Heuristic,
        )
    };
    if choice == DetectorChoice::Heuristic {
        return Ok(heuristic());
    }
    let model = match tatr() {
        Some(Ok(model)) => model,
        Some(Err(err)) if choice == DetectorChoice::Auto => {
            warnings.push(format!(
                "The table model could not load ({}), so the heuristic was used.",
                err.message
            ));
            return Ok(heuristic());
        }
        Some(Err(err)) => return Err(err.clone()),
        None if choice == DetectorChoice::Auto => return Ok(heuristic()),
        None => {
            return Err(ErrorInfo::new(
                ErrorCode::DependencyMissing,
                "The table detection model is not installed.",
                "Run `rait models fetch` to install it.".to_string(),
            ));
        }
    };
    let found: Result<Vec<Vec<Table>>, ErrorInfo> = layouts
        .par_iter()
        .enumerate()
        .map(|(i, page)| {
            let number = first + i as u32;
            let (width, height, rgb) = render(path, number)?;
            let regions = model.detect(&rgb, width, height)?;
            let boxes: Vec<[f64; 4]> = regions
                .iter()
                .map(|r| {
                    [
                        r.x0 as f64 * page.width,
                        r.y0 as f64 * page.height,
                        r.x1 as f64 * page.width,
                        r.y1 as f64 * page.height,
                    ]
                })
                .collect();
            let mut found = vec![];
            for region in tables::distinct(&boxes) {
                let table = match structure() {
                    Some(model) => structured(path, number, page, region, model)?,
                    None => None,
                };
                match table {
                    Some(table) => found.push(table),
                    None => found.extend(tables::detect_in(page, &[region])),
                }
            }
            Ok(found)
        })
        .collect();
    match found {
        Ok(found) => Ok((found, Detector::Tatr)),
        Err(err) if choice == DetectorChoice::Auto && err.code == ErrorCode::DependencyMissing => {
            warnings.push("`pdftoppm` is missing, so tables were found with the heuristic.".into());
            Ok(heuristic())
        }
        Err(err) => Err(err),
    }
}

/// The table in `region` (page points) from its rows and columns.
fn structured(
    path: &Path,
    number: u32,
    page: &Page,
    region: [f64; 4],
    model: &TableStructure,
) -> Result<Option<Table>, ErrorInfo> {
    let [x0, y0] = [
        (region[0] - CROP_PAD).max(0.0),
        (region[1] - CROP_PAD).max(0.0),
    ];
    let [x1, y1] = [
        (region[2] + CROP_PAD).min(page.width),
        (region[3] + CROP_PAD).min(page.height),
    ];
    let side = (x1 - x0).max(y1 - y0).max(1.0);
    let dpi = (f64::from(STRUCTURE_SIZE) * 72.0 / side).clamp(72.0, MAX_CROP_DPI);
    let scale = dpi / 72.0;
    let (px, py) = ((x0 * scale).floor(), (y0 * scale).floor());
    let args: Vec<OsString> = vec![
        "-f".into(),
        number.to_string().into(),
        "-l".into(),
        number.to_string().into(),
        "-r".into(),
        format!("{dpi:.2}").into(),
        "-x".into(),
        px.to_string().into(),
        "-y".into(),
        py.to_string().into(),
        "-W".into(),
        ((x1 - x0) * scale).round().to_string().into(),
        "-H".into(),
        ((y1 - y0) * scale).round().to_string().into(),
        path.into(),
    ];
    let (width, height, rgb) = ppm(args)?;
    let (left, top) = (px / scale, py / scale);
    let (w, h) = (width as f64 / scale, height as f64 / scale);
    let elements: Vec<structure::Element> = model
        .recognize(&rgb, width, height)?
        .into_iter()
        .map(|e| {
            let r = e.region;
            let b = [
                left + f64::from(r.x0) * w,
                top + f64::from(r.y0) * h,
                left + f64::from(r.x1) * w,
                top + f64::from(r.y1) * h,
            ];
            (e.label, b, r.score)
        })
        .collect();
    Ok(structure::table(page, region, &elements))
}

/// One page as RGB, longest side [`INPUT_SIZE`] pixels.
fn render(path: &Path, page: u32) -> Result<(usize, usize, Vec<u8>), ErrorInfo> {
    ppm(vec![
        "-f".into(),
        page.to_string().into(),
        "-l".into(),
        page.to_string().into(),
        "-scale-to".into(),
        INPUT_SIZE.to_string().into(),
        path.into(),
    ])
}

fn ppm(args: Vec<OsString>) -> Result<(usize, usize, Vec<u8>), ErrorInfo> {
    let out = exec::run(PDFTOPPM, args).map_err(|err| match err {
        ExecError::Missing(_) => err.into(),
        err => backend_error(err),
    })?;
    parse_ppm(out.stdout).ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::DependencyFailed,
            "`pdftoppm` returned an unreadable page image.",
            "Run `rait doctor` to check the Poppler install.".to_string(),
        )
    })
}

/// A binary PPM (P6, 8-bit): width, height and the RGB bytes.
pub(crate) fn parse_ppm(mut bytes: Vec<u8>) -> Option<(usize, usize, Vec<u8>)> {
    let mut fields = vec![];
    let mut pos = 0;
    while fields.len() < 4 {
        while bytes.get(pos)?.is_ascii_whitespace() {
            pos += 1;
        }
        if bytes[pos] == b'#' {
            while *bytes.get(pos)? != b'\n' {
                pos += 1;
            }
            continue;
        }
        let start = pos;
        while !bytes.get(pos)?.is_ascii_whitespace() {
            pos += 1;
        }
        fields.push(String::from_utf8_lossy(&bytes[start..pos]).into_owned());
    }
    let (w, h): (usize, usize) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    if fields[0] != "P6" || fields[3] != "255" {
        return None;
    }
    let data = bytes.split_off(pos + 1);
    (data.len() == w * h * 3).then_some((w, h, data))
}

#[cfg(test)]
mod tests {
    use super::parse_ppm;

    #[test]
    fn parses_binary_ppm_with_comments() {
        let mut ppm = b"P6\n# made by a test\n2 1\n255\n".to_vec();
        ppm.extend([1, 2, 3, 4, 5, 6]);
        assert_eq!(parse_ppm(ppm), Some((2, 1, vec![1, 2, 3, 4, 5, 6])));
        assert_eq!(parse_ppm(b"P6\n2 1\n255\n\x01".to_vec()), None);
        assert_eq!(parse_ppm(b"P5\n1 1\n255\n\x01".to_vec()), None);
    }
}

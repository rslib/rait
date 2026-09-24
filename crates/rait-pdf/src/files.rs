use crate::{OCRMYPDF, PDFTOPPM, PageRange, QPDF, backend_error, check_file, exec, info};
use rait_core::{ErrorCode, ErrorInfo, path_to_protocol};
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ImageFormat {
    Png,
    Jpeg,
}

impl ImageFormat {
    fn flag(self) -> &'static str {
        match self {
            Self::Png => "-png",
            Self::Jpeg => "-jpeg",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }
}

#[derive(Clone, Debug)]
pub struct RenderRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    pub pages: PageRange,
    pub dpi: u32,
    pub format: ImageFormat,
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct SplitRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    pub pages: PageRange,
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct MergeRequest {
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
    pub force: bool,
}

#[derive(Clone, Debug)]
pub struct OcrRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutputFiles {
    pub files: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Merged {
    pub output: String,
    pub inputs: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ocred {
    pub output: String,
}

/// File name stem used to name outputs written for `path`.
pub fn output_stem(path: &Path) -> Result<String, ErrorInfo> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorCode::InvalidInput,
                "Input path has no UTF-8 file name.",
                "Use a UTF-8 file name.".to_string(),
            )
        })
}

pub fn render(req: &RenderRequest) -> Result<OutputFiles, ErrorInfo> {
    let page_count = info(&req.path)?.pages;
    let (first, last) = req.pages.bounds(page_count)?;
    let stem = output_stem(&req.path)?;
    // pdftoppm zero-pads page numbers to the digit count of the document page count.
    let width = page_count.to_string().len();
    let ext = req.format.extension();
    let targets: Vec<PathBuf> = (first..=last)
        .map(|page| req.output_dir.join(format!("{stem}-{page:0width$}.{ext}")))
        .collect();
    prepare_outputs(&req.output_dir, &targets, req.force)?;
    let args: Vec<OsString> = vec![
        "-r".into(),
        req.dpi.to_string().into(),
        req.format.flag().into(),
        "-f".into(),
        first.to_string().into(),
        "-l".into(),
        last.to_string().into(),
        req.path.clone().into(),
        req.output_dir.join(&stem).into(),
    ];
    exec::run(PDFTOPPM, args).map_err(backend_error)?;
    collect_outputs(&targets)
}

pub fn split(req: &SplitRequest) -> Result<OutputFiles, ErrorInfo> {
    check_file(&req.path)?;
    let out = exec::run(
        QPDF,
        [OsString::from("--show-npages"), req.path.clone().into()],
    )
    .map_err(backend_error)?;
    let page_count: u32 = String::from_utf8_lossy(&out.stdout)
        .trim()
        .parse()
        .map_err(|_| {
            ErrorInfo::new(
                ErrorCode::DependencyFailed,
                "`qpdf` did not report a page count.",
                "Run `rait doctor` to check the qpdf install.".to_string(),
            )
        })?;
    let (first, last) = req.pages.bounds(page_count)?;
    let stem = output_stem(&req.path)?;
    let width = page_count.to_string().len();
    let pages: Vec<(u32, PathBuf)> = (first..=last)
        .map(|page| {
            let target = req.output_dir.join(format!("{stem}-{page:0width$}.pdf"));
            (page, target)
        })
        .collect();
    let targets: Vec<PathBuf> = pages.iter().map(|(_, target)| target.clone()).collect();
    prepare_outputs(&req.output_dir, &targets, req.force)?;
    for (page, target) in &pages {
        let args: Vec<OsString> = vec![
            "--warning-exit-0".into(),
            req.path.clone().into(),
            "--pages".into(),
            ".".into(),
            page.to_string().into(),
            "--".into(),
            target.clone().into(),
        ];
        exec::run(QPDF, args).map_err(backend_error)?;
    }
    collect_outputs(&targets)
}

pub fn merge(req: &MergeRequest) -> Result<Merged, ErrorInfo> {
    if req.inputs.len() < 2 {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "Merge needs at least two input PDFs.",
            "Pass two or more <FILE> arguments.".to_string(),
        ));
    }
    for input in &req.inputs {
        check_file(input)?;
        if same_file(input, &req.output) {
            return Err(ErrorInfo::new(
                ErrorCode::InvalidInput,
                "The output file is also an input.",
                "Choose a different --output path.".to_string(),
            ));
        }
    }
    let output = path_to_protocol(&req.output)?;
    if req.output.exists() && !req.force {
        return Err(output_exists(&output));
    }
    let name = req
        .output
        .file_name()
        .ok_or_else(|| ErrorInfo::new(ErrorCode::InvalidInput, "--output must name a file.", None))?
        .to_string_lossy();
    let tmp = req.output.with_file_name(format!(".{name}.rait-tmp"));
    let mut args: Vec<OsString> = vec![
        "--warning-exit-0".into(),
        "--empty".into(),
        "--pages".into(),
    ];
    args.extend(req.inputs.iter().map(|input| input.clone().into()));
    args.extend(["--".into(), tmp.clone().into()]);
    if let Err(err) = exec::run(QPDF, args) {
        let _ = fs::remove_file(&tmp);
        return Err(backend_error(err));
    }
    fs::rename(&tmp, &req.output).map_err(|err| {
        let _ = fs::remove_file(&tmp);
        io_error("Failed to move the merged PDF into place", err)
    })?;
    Ok(Merged {
        output,
        inputs: req.inputs.len(),
    })
}

pub fn ocr(req: &OcrRequest) -> Result<Ocred, ErrorInfo> {
    check_file(&req.path)?;
    let target = req
        .output_dir
        .join(format!("{}.pdf", output_stem(&req.path)?));
    if same_file(&req.path, &target) {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The OCR output would replace the input.",
            "Choose a different --output directory.".to_string(),
        ));
    }
    prepare_outputs(&req.output_dir, std::slice::from_ref(&target), req.force)?;
    let args: Vec<OsString> = vec![
        "--quiet".into(),
        "--skip-text".into(),
        req.path.clone().into(),
        target.clone().into(),
    ];
    exec::run(OCRMYPDF, args).map_err(backend_error)?;
    Ok(Ocred {
        output: collect_outputs(&[target])?.files.remove(0),
    })
}

fn prepare_outputs(dir: &Path, targets: &[PathBuf], force: bool) -> Result<(), ErrorInfo> {
    if !force && let Some(existing) = targets.iter().find(|target| target.exists()) {
        return Err(output_exists(&path_to_protocol(existing)?));
    }
    fs::create_dir_all(dir).map_err(|err| io_error("Failed to create the output directory", err))
}

fn collect_outputs(targets: &[PathBuf]) -> Result<OutputFiles, ErrorInfo> {
    let mut files = Vec::with_capacity(targets.len());
    for target in targets {
        if !target.exists() {
            return Err(ErrorInfo::new(
                ErrorCode::DependencyFailed,
                format!("Expected output {} was not written.", target.display()),
                "Run `rait doctor` to check the backend install.".to_string(),
            ));
        }
        files.push(path_to_protocol(target)?);
    }
    Ok(OutputFiles { files })
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn output_exists(path: &str) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::OutputExists,
        format!("Output {path} already exists."),
        "Use --force to replace existing files.".to_string(),
    )
}

fn io_error(context: &str, err: std::io::Error) -> ErrorInfo {
    let code = if err.kind() == std::io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::InternalError
    };
    ErrorInfo::new(code, format!("{context}: {err}"), None)
}

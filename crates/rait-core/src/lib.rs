pub mod cells;
pub mod exec;
mod range;

pub use range::PageRange;

use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::ErrorKind;
use std::path::Path;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidInput,
    DependencyMissing,
    DependencyFailed,
    FileNotFound,
    PermissionDenied,
    EncryptedPdf,
    InvalidPageRange,
    OutputExists,
    OutputLimitExceeded,
    InternalError,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Raw backend diagnostics. Never serialized; the CLI prints it to stderr.
    #[serde(skip)]
    pub detail: Option<String>,
}

impl ErrorInfo {
    pub fn new(
        code: ErrorCode,
        message: impl Into<String>,
        hint: impl Into<Option<String>>,
    ) -> Self {
        Self {
            code,
            message: message.into(),
            hint: hint.into(),
            detail: None,
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Truncation {
    pub limit_chars: usize,
    pub returned_chars: usize,
    pub total_chars: usize,
    pub hint: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Record {
    pub schema: u8,
    pub id: usize,
    pub command: String,
    pub input: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorInfo>,
    pub warnings: Vec<String>,
    pub truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation: Option<Truncation>,
}

impl Record {
    pub fn success(id: usize, command: &str, input: String, data: Value) -> Self {
        Self {
            schema: 1,
            id,
            command: command.into(),
            input,
            ok: true,
            data: Some(data),
            error: None,
            warnings: vec![],
            truncated: false,
            truncation: None,
        }
    }

    pub fn failure(id: usize, command: &str, input: String, error: ErrorInfo) -> Self {
        Self {
            schema: 1,
            id,
            command: command.into(),
            input,
            ok: false,
            data: None,
            error: Some(error),
            warnings: vec![],
            truncated: false,
            truncation: None,
        }
    }
}

pub fn path_to_protocol(path: &Path) -> Result<String, ErrorInfo> {
    path.to_str().map(ToOwned::to_owned).ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            "Path is not valid UTF-8.",
            "Use a UTF-8 path.".to_string(),
        )
    })
}

/// Limit `text` to `max_chars`, cutting on a char boundary.
pub fn limit_text(text: String, max_chars: usize) -> (String, Option<Truncation>) {
    let Some((cut, _)) = text.char_indices().nth(max_chars) else {
        return (text, None);
    };
    let total_chars = text.chars().count();
    let limited = text[..cut].to_owned();
    (limited, Some(truncation(max_chars, max_chars, total_chars)))
}

/// Keep the leading chunks (pages) whose combined length fits in `max_chars`.
/// If not even the first chunk fits, return it cut on a char boundary.
pub fn limit_chunks(chunks: Vec<String>, max_chars: usize) -> (Vec<String>, Option<Truncation>) {
    let total_chars: usize = chunks.iter().map(|chunk| chunk.chars().count()).sum();
    if total_chars <= max_chars {
        return (chunks, None);
    }
    let mut kept = vec![];
    let mut returned_chars = 0;
    for chunk in chunks {
        let chars = chunk.chars().count();
        if returned_chars + chars > max_chars {
            if kept.is_empty() {
                let (cut, _) = limit_text(chunk, max_chars);
                kept.push(cut);
                returned_chars = max_chars;
            }
            break;
        }
        returned_chars += chars;
        kept.push(chunk);
    }
    (
        kept,
        Some(truncation(max_chars, returned_chars, total_chars)),
    )
}

fn truncation(limit_chars: usize, returned_chars: usize, total_chars: usize) -> Truncation {
    Truncation {
        limit_chars,
        returned_chars,
        total_chars,
        hint: "Request a smaller page range or increase --max-chars.".into(),
    }
}

pub fn default_jobs() -> usize {
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .max(1)
}

pub fn run_ordered<I, R, F>(jobs: usize, inputs: Vec<I>, f: F) -> Vec<R>
where
    I: Send,
    R: Send,
    F: Fn(usize, I) -> R + Sync + Send,
{
    let jobs = if jobs == 0 {
        default_jobs()
    } else {
        jobs.max(1)
    };
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .expect("rayon pool");
    let mut results = pool.install(|| {
        inputs
            .into_par_iter()
            .enumerate()
            .map(|(i, input)| (i, f(i, input)))
            .collect::<Vec<_>>()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

/// Checks that an input path exists and can be read.
pub fn check_input(path: &Path) -> Result<(), ErrorInfo> {
    match path.try_exists() {
        Ok(true) => Ok(()),
        Ok(false) => Err(ErrorInfo::new(
            ErrorCode::FileNotFound,
            "Input file was not found.",
            "Check the path.".to_string(),
        )),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => Err(ErrorInfo::new(
            ErrorCode::PermissionDenied,
            "Input file cannot be accessed.",
            "Check file permissions.".to_string(),
        )),
        Err(err) => Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            format!("Input file cannot be checked: {err}"),
            None,
        )),
    }
}

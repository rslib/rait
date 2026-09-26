//! Model files: a pinned registry, a cache directory and a verified download.

use rait_core::exec::{self, Dependency};
use rait_core::{ErrorCode, ErrorInfo, path_to_protocol};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

const CURL: &str = "curl";

pub const DEPENDENCIES: &[Dependency] = &[Dependency {
    program: CURL,
    capabilities: &["models.fetch"],
    fallback: None,
    install: "macOS: included; Debian/Ubuntu: sudo apt install curl; Fedora: sudo dnf install curl",
    version_flag: "--version",
}];

/// A downloadable model pinned to one upstream revision and checksum.
#[derive(Clone, Copy, Debug)]
pub struct ModelSpec {
    pub name: &'static str,
    pub purpose: &'static str,
    pub license: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
    /// File name in the model cache.
    pub file: &'static str,
}

/// Table Transformer detection weights (microsoft/table-transformer-detection).
pub const TATR: ModelSpec = ModelSpec {
    name: "tatr",
    purpose: "table detection for pdf tables and pdf text",
    license: "MIT",
    url: "https://huggingface.co/microsoft/table-transformer-detection/resolve/2357cbe2b5a5d1c03e54f32764f06058933b65ab/model.safetensors",
    sha256: "8f1aa73170102c038d40155e2734b343bf07e0fe12594228a8590943b01dccf7",
    bytes: 115_317_516,
    file: "tatr-2357cbe.safetensors",
};

/// Table Transformer structure weights v1.1
/// (microsoft/table-transformer-structure-recognition-v1.1-all).
pub const TATR_STRUCTURE: ModelSpec = ModelSpec {
    name: "tatr-structure",
    purpose: "rows, columns and header cells inside detected tables",
    license: "MIT",
    url: "https://huggingface.co/microsoft/table-transformer-structure-recognition-v1.1-all/resolve/7587a7ef111d9dcbf8ac695f1376ab7014340a0c/model.safetensors",
    sha256: "9df416575a3a36ebd0129342d4f597f14d6e5170268f3d52d28584ab4466a501",
    bytes: 115_437_156,
    file: "tatr-structure-7587a7e.safetensors",
};

pub const MODELS: &[ModelSpec] = &[TATR, TATR_STRUCTURE];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ModelStatus {
    pub installed: bool,
    pub bytes: u64,
    pub license: String,
    pub purpose: String,
    pub path: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Fetched {
    pub path: String,
    pub status: &'static str,
}

/// `$RAIT_MODELS_DIR`, else `$XDG_CACHE_HOME/rait/models`, else
/// `~/.cache/rait/models`.
pub fn cache_dir() -> Option<PathBuf> {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    var("RAIT_MODELS_DIR")
        .or_else(|| var("XDG_CACHE_HOME").map(|d| d.join("rait/models")))
        .or_else(|| var("HOME").map(|d| d.join(".cache/rait/models")))
}

/// The model file when it is present with the expected size. The checksum is
/// verified at download time, not on every load.
pub fn installed(spec: &ModelSpec) -> Option<PathBuf> {
    let path = cache_dir()?.join(spec.file);
    (fs::metadata(&path).ok()?.len() == spec.bytes).then_some(path)
}

pub fn status(spec: &ModelSpec) -> Result<ModelStatus, ErrorInfo> {
    let dir = cache_dir().ok_or_else(no_cache_dir)?;
    Ok(ModelStatus {
        installed: installed(spec).is_some(),
        bytes: spec.bytes,
        license: spec.license.into(),
        purpose: spec.purpose.into(),
        path: path_to_protocol(&dir.join(spec.file))?,
    })
}

/// Download and verify `spec` into the cache. A present, verified file is a
/// no-op unless `force`.
pub fn fetch(spec: &ModelSpec, force: bool) -> Result<Fetched, ErrorInfo> {
    fetch_into(spec, &cache_dir().ok_or_else(no_cache_dir)?, force)
}

pub fn fetch_into(spec: &ModelSpec, dir: &Path, force: bool) -> Result<Fetched, ErrorInfo> {
    let path = dir.join(spec.file);
    let shown = path_to_protocol(&path)?;
    if !force && path.exists() && sha256(&path).is_ok_and(|sum| sum == spec.sha256) {
        return Ok(Fetched {
            path: shown,
            status: "already installed (no-op)",
        });
    }
    fs::create_dir_all(dir).map_err(|e| io_error("create the model directory", e))?;
    let part = dir.join(format!("{}.part", spec.file));
    let args: Vec<OsString> = vec![
        "--fail".into(),
        "--location".into(),
        "--silent".into(),
        "--show-error".into(),
        "--retry".into(),
        "2".into(),
        "--output".into(),
        part.clone().into(),
        spec.url.into(),
    ];
    if let Err(err) = exec::run(CURL, args) {
        let _ = fs::remove_file(&part);
        return Err(match err {
            exec::ExecError::Missing(_) => err.into(),
            _ => ErrorInfo::new(
                ErrorCode::DependencyFailed,
                format!("Downloading the {} model failed.", spec.name),
                "Check the network connection and retry `rait models fetch`.".to_string(),
            )
            .with_detail(err.to_string()),
        });
    }
    let sum = sha256(&part).map_err(|e| io_error("read the downloaded model", e))?;
    if sum != spec.sha256 {
        let _ = fs::remove_file(&part);
        return Err(ErrorInfo::new(
            ErrorCode::DependencyFailed,
            format!(
                "The downloaded {} model does not match its checksum.",
                spec.name
            ),
            "Retry `rait models fetch`; if it keeps failing, the upstream file changed."
                .to_string(),
        )
        .with_detail(format!("expected {}, got {sum}", spec.sha256)));
    }
    fs::rename(&part, &path).map_err(|e| io_error("move the model into place", e))?;
    Ok(Fetched {
        path: shown,
        status: "downloaded",
    })
}

fn sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn no_cache_dir() -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        "No model cache directory: HOME is not set.",
        "Set RAIT_MODELS_DIR to a writable directory.".to_string(),
    )
}

fn io_error(action: &str, err: io::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::PermissionDenied,
        format!("Failed to {action}: {err}"),
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(source: &Path, sha256: &'static str) -> ModelSpec {
        let url = format!("file://{}", source.display());
        ModelSpec {
            name: "test",
            purpose: "test",
            license: "MIT",
            url: Box::leak(url.into_boxed_str()),
            sha256,
            bytes: 5,
            file: "test.bin",
        }
    }

    #[test]
    fn fetch_verifies_then_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        fs::write(&source, b"hello").unwrap();
        let cache = dir.path().join("cache");
        // sha256("hello")
        let good = spec(
            &source,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824",
        );
        assert_eq!(
            fetch_into(&good, &cache, false).unwrap().status,
            "downloaded"
        );
        assert_eq!(fs::read(cache.join("test.bin")).unwrap(), b"hello");
        assert_eq!(
            fetch_into(&good, &cache, false).unwrap().status,
            "already installed (no-op)"
        );
        assert_eq!(
            fetch_into(&good, &cache, true).unwrap().status,
            "downloaded"
        );
    }

    #[test]
    fn fetch_rejects_a_checksum_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.bin");
        fs::write(&source, b"hello").unwrap();
        let cache = dir.path().join("cache");
        let bad = spec(
            &source,
            "0000000000000000000000000000000000000000000000000000000000000000",
        );
        let err = fetch_into(&bad, &cache, false).unwrap_err();
        assert_eq!(err.code, ErrorCode::DependencyFailed);
        assert!(!cache.join("test.bin").exists());
        assert!(!cache.join("test.bin.part").exists());
    }
}

mod formats;

use formats::{Format, Kind, Meta};
use rait_core::{ErrorCode, ErrorInfo, PageRange, check_input, path_to_protocol};
use serde::Serialize;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Component, Path, PathBuf};

/// Entries listed when `--entries` is not given.
pub const DEFAULT_ENTRIES: u64 = 200;
/// Largest entry `read` returns.
const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

pub const EXTENSIONS: &[&str] = &[
    "zip", "7z", "a", "ar", "deb", "tar", "tgz", "tbz2", "txz", "tzst", "gz", "zst", "zstd", "xz",
    "bz2",
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    pub path: String,
    #[serde(rename = "type")]
    pub kind: &'static str,
    /// Uncompressed bytes; directories and links are 0.
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Listing {
    pub format: &'static str,
    pub total_entries: u64,
    pub files: u64,
    pub dirs: u64,
    /// Uncompressed bytes of all files.
    pub bytes: u64,
    /// The entries returned, such as `1-200`.
    pub range: String,
    pub entries: Vec<Entry>,
}

#[derive(Clone, Debug)]
pub struct ListRequest {
    pub path: PathBuf,
    /// Entries to return, 1-based; the first [`DEFAULT_ENTRIES`] by default.
    pub entries: Option<PageRange>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EntryText {
    pub entry: String,
    pub size: u64,
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct ExtractRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    /// Entry paths, or directories of entries, to extract; all by default.
    pub only: Vec<String>,
    /// Refuse archives whose files add up to more than this.
    pub max_bytes: u64,
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Skipped {
    pub path: String,
    pub reason: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Extracted {
    pub output: String,
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub skipped: Vec<Skipped>,
}

pub fn list(req: &ListRequest) -> Result<Listing, ErrorInfo> {
    let format = open(&req.path)?;
    let mut all = vec![];
    formats::walk(&req.path, format, &mut |meta, data| {
        let size = match (meta.kind, meta.size) {
            (Kind::File, Some(size)) => size,
            (Kind::File, None) => io::copy(data, &mut io::sink()).map_err(|e| e.to_string())?,
            _ => 0,
        };
        all.push(Entry {
            path: meta.path.clone(),
            kind: meta.kind.name(),
            size,
        });
        Ok(true)
    })
    .map_err(|detail| unreadable(format, detail))?;
    let total = all.len() as u64;
    let (first, last) = match req.entries {
        Some(range) => {
            let (first, last) =
                range.bounds(u32::try_from(total).unwrap_or(u32::MAX), "--entries")?;
            (u64::from(first), u64::from(last))
        }
        None => (1, total.min(DEFAULT_ENTRIES)),
    };
    let files = all.iter().filter(|e| e.kind == "file").count() as u64;
    let dirs = all.iter().filter(|e| e.kind == "dir").count() as u64;
    let bytes = all.iter().map(|e| e.size).sum();
    let entries: Vec<Entry> = all
        .into_iter()
        .skip(first.saturating_sub(1) as usize)
        .take((last + 1).saturating_sub(first) as usize)
        .collect();
    Ok(Listing {
        format: format.name(),
        total_entries: total,
        files,
        dirs,
        bytes,
        range: match entries.len() {
            0 => "none".into(),
            1 => first.to_string(),
            n => format!("{first}-{}", first + n as u64 - 1),
        },
        entries,
    })
}

/// The text of one file entry, without extracting anything.
pub fn read(path: &Path, entry: &str) -> Result<EntryText, ErrorInfo> {
    let format = open(path)?;
    let want = entry.trim_start_matches("./");
    let mut found: Option<Result<EntryText, ErrorInfo>> = None;
    formats::walk(path, format, &mut |meta, data| {
        if meta.path.trim_start_matches("./") != want {
            return Ok(true);
        }
        found = Some(match meta.kind {
            Kind::File => {
                let mut bytes = vec![];
                data.take(MAX_READ_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|e| e.to_string())?;
                text_of(entry, bytes)
            }
            kind => Err(ErrorInfo::new(
                ErrorCode::InvalidInput,
                format!("`{entry}` is a {}, not a file.", kind.name()),
                "Run `rait archive list <file>` to see the files.".to_string(),
            )),
        });
        Ok(false)
    })
    .map_err(|detail| unreadable(format, detail))?;
    found.unwrap_or_else(|| {
        Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            format!("No entry `{entry}` in the archive."),
            format!(
                "Run `rait archive list {}` to see the entries.",
                path.display()
            ),
        ))
    })
}

fn text_of(entry: &str, bytes: Vec<u8>) -> Result<EntryText, ErrorInfo> {
    if bytes.len() as u64 > MAX_READ_BYTES {
        return Err(ErrorInfo::new(
            ErrorCode::OutputLimitExceeded,
            format!("`{entry}` is larger than {} MiB.", MAX_READ_BYTES >> 20),
            "Extract it with `rait archive extract <file> --output <dir> --only <entry>`."
                .to_string(),
        ));
    }
    let binary = bytes.iter().take(8192).any(|&b| b == 0);
    match (binary, String::from_utf8(bytes)) {
        (false, Ok(text)) => Ok(EntryText {
            entry: entry.to_string(),
            size: text.len() as u64,
            text,
        }),
        _ => Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            format!("`{entry}` is binary, not UTF-8 text."),
            "Extract it with `rait archive extract <file> --output <dir> --only <entry>`."
                .to_string(),
        )),
    }
}

/// Extract with path checks. Entries that would land outside `output_dir`, links
/// and special files are skipped and reported. Nothing is written when an
/// existing file would be replaced without `force` or when the files' declared
/// sizes pass `max_bytes`; actual sizes are counted too, since archives can lie.
pub fn extract(req: &ExtractRequest) -> Result<Extracted, ErrorInfo> {
    let format = open(&req.path)?;
    let out = &req.output_dir;
    let mut plan: Vec<(String, Option<PathBuf>)> = vec![];
    let mut skipped = vec![];
    let mut declared = 0u64;
    let mut conflict = None;
    formats::walk(&req.path, format, &mut |meta, _| {
        let relative = match plan_entry(meta, &req.only) {
            Ok(Some(relative)) => relative,
            Ok(None) => return Ok(true),
            Err(reason) => {
                skipped.push(Skipped {
                    path: meta.path.clone(),
                    reason,
                });
                plan.push((meta.path.clone(), None));
                return Ok(true);
            }
        };
        if meta.kind == Kind::File {
            declared = declared.saturating_add(meta.size.unwrap_or(0));
            let target = out.join(&relative);
            if conflict.is_none() && !req.force && target.exists() {
                conflict = Some(target);
            }
        }
        plan.push((meta.path.clone(), Some(relative)));
        Ok(true)
    })
    .map_err(|detail| unreadable(format, detail))?;
    if let Some(existing) = conflict {
        return Err(ErrorInfo::new(
            ErrorCode::OutputExists,
            format!("Output {} already exists.", existing.display()),
            "Use --force to replace existing files, or pick another --output.".to_string(),
        ));
    }
    if declared > req.max_bytes {
        return Err(too_big(declared, req.max_bytes));
    }
    fs::create_dir_all(out).map_err(|e| write_error("create the output directory", e))?;
    let root = out
        .canonicalize()
        .map_err(|e| write_error("resolve the output directory", e))?;
    let mut result = Extracted {
        output: path_to_protocol(out)?,
        files: 0,
        dirs: 0,
        bytes: 0,
        skipped,
    };
    let mut index = 0;
    let mut failure: Option<ErrorInfo> = None;
    formats::walk(&req.path, format, &mut |meta, data| {
        let Some((path, target)) = plan.get(index).cloned() else {
            return Ok(true);
        };
        if path != meta.path {
            return Ok(true);
        }
        index += 1;
        let Some(relative) = target else {
            return Ok(true);
        };
        if through_symlink(&root, &relative) {
            result.skipped.push(Skipped {
                path,
                reason: "path goes through a symlink",
            });
            return Ok(true);
        }
        let target = root.join(&relative);
        let written = match meta.kind {
            Kind::Dir => fs::create_dir_all(&target)
                .map(|()| {
                    result.dirs += 1;
                })
                .map_err(|e| write_error("create a directory", e)),
            _ => write_file(
                &target,
                data,
                meta.mode,
                req.max_bytes.saturating_sub(result.bytes),
            )
            .map(|n| {
                result.files += 1;
                result.bytes += n;
            }),
        };
        match written {
            Ok(()) => Ok(true),
            Err(err) => {
                failure = Some(err);
                Ok(false)
            }
        }
    })
    .map_err(|detail| unreadable(format, detail))?;
    failure.map_or(Ok(result), Err)
}

/// Where an entry goes, relative to the output directory. `Ok(None)` means it
/// is filtered out by `only`; `Err` gives the reason it is skipped.
fn plan_entry(meta: &Meta, only: &[String]) -> Result<Option<PathBuf>, &'static str> {
    let name = meta.path.replace('\\', "/");
    if !only.is_empty() {
        let name = name.trim_start_matches("./").trim_end_matches('/');
        let wanted = only.iter().any(|want| {
            let want = want.trim_start_matches("./").trim_end_matches('/');
            name == want || name.starts_with(&format!("{want}/"))
        });
        if !wanted {
            return Ok(None);
        }
    }
    match meta.kind {
        Kind::Symlink | Kind::Hardlink => return Err("links are not extracted"),
        Kind::Other => return Err("special files are not extracted"),
        Kind::File | Kind::Dir => {}
    }
    let windows_drive = name.len() >= 2 && name.as_bytes()[1] == b':';
    if name.starts_with('/') || windows_drive {
        return Err("absolute path");
    }
    let mut relative = PathBuf::new();
    for part in Path::new(&name).components() {
        match part {
            Component::Normal(part) => relative.push(part),
            Component::CurDir => {}
            Component::ParentDir => return Err("path leaves the output directory"),
            Component::RootDir | Component::Prefix(_) => return Err("absolute path"),
        }
    }
    if relative.as_os_str().is_empty() {
        return Err("empty path");
    }
    Ok(Some(relative))
}

/// Whether any existing part of `root/relative`, including the file itself, is a
/// symlink that a write would follow out of the output tree.
fn through_symlink(root: &Path, relative: &Path) -> bool {
    let mut current = root.to_path_buf();
    relative.components().any(|part| {
        current.push(part);
        fs::symlink_metadata(&current).is_ok_and(|m| m.file_type().is_symlink())
    })
}

fn write_file(
    target: &Path,
    data: &mut dyn Read,
    mode: Option<u32>,
    budget: u64,
) -> Result<u64, ErrorInfo> {
    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent).map_err(|e| write_error("create a directory", e))?;
    }
    let mut file = fs::File::create(target).map_err(|e| write_error("create a file", e))?;
    let written = io::copy(&mut data.take(budget.saturating_add(1)), &mut file).map_err(|e| {
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The archive data is damaged.",
            None,
        )
        .with_detail(e.to_string())
    })?;
    file.flush().map_err(|e| write_error("write a file", e))?;
    if written > budget {
        let _ = fs::remove_file(target);
        return Err(too_big(written, budget));
    }
    #[cfg(unix)]
    if let Some(mode) = mode {
        use std::os::unix::fs::PermissionsExt;
        // Keep only the permission bits; never setuid, setgid or sticky.
        let perms = fs::Permissions::from_mode((mode & 0o777) | 0o600);
        let _ = fs::set_permissions(target, perms);
    }
    Ok(written)
}

fn open(path: &Path) -> Result<Format, ErrorInfo> {
    check_input(path)?;
    formats::detect(path)
        .map_err(|detail| {
            ErrorInfo::new(ErrorCode::InvalidInput, "The file cannot be read.", None).with_detail(detail)
        })?
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorCode::InvalidInput,
                "The file is not a supported archive.",
                "Use zip, 7z, ar, tar (plain, gz, zst, xz or bz2) or a single .gz, .zst, .xz or .bz2 file.".to_string(),
            )
        })
}

fn unreadable(format: Format, detail: String) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        format!(
            "The {} archive is damaged, encrypted or unsupported.",
            format.name()
        ),
        "Check the file, or list it with `rait archive list <file>`.".to_string(),
    )
    .with_detail(detail)
}

fn too_big(bytes: u64, limit: u64) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::OutputLimitExceeded,
        format!("The files add up to {bytes} bytes, over the {limit}-byte limit."),
        "Raise --max-bytes if you trust this archive, or extract less with --only.".to_string(),
    )
}

fn write_error(action: &str, err: io::Error) -> ErrorInfo {
    let code = if err.kind() == io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::InternalError
    };
    ErrorInfo::new(code, format!("Failed to {action}: {err}"), None)
}

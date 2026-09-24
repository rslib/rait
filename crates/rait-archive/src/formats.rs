use std::fs::File;
use std::io::{self, BufReader, Read};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Codec {
    None,
    Gzip,
    Zstd,
    Xz,
    Bzip2,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Format {
    Zip,
    SevenZ,
    Ar,
    Tar(Codec),
    /// A single compressed file, not an archive.
    Compressed(Codec),
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Self::Zip => "zip",
            Self::SevenZ => "7z",
            Self::Ar => "ar",
            Self::Tar(Codec::None) => "tar",
            Self::Tar(Codec::Gzip) => "tar.gz",
            Self::Tar(Codec::Zstd) => "tar.zst",
            Self::Tar(Codec::Xz) => "tar.xz",
            Self::Tar(Codec::Bzip2) => "tar.bz2",
            Self::Compressed(Codec::Gzip) => "gz",
            Self::Compressed(Codec::Zstd) => "zst",
            Self::Compressed(Codec::Xz) => "xz",
            Self::Compressed(Codec::Bzip2) => "bz2",
            Self::Compressed(Codec::None) => "raw",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    File,
    Dir,
    Symlink,
    Hardlink,
    /// Devices, FIFOs and other special entries.
    Other,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Dir => "dir",
            Self::Symlink => "symlink",
            Self::Hardlink => "hardlink",
            Self::Other => "other",
        }
    }
}

/// An entry as stored, before any path checks.
pub struct Meta {
    pub path: String,
    pub kind: Kind,
    /// Uncompressed size; `None` for single compressed files, which do not store it.
    pub size: Option<u64>,
    pub mode: Option<u32>,
}

/// Called for each entry with its data; returns `false` to stop early.
pub type Visit<'a> = dyn FnMut(&Meta, &mut dyn Read) -> Result<bool, String> + 'a;

pub fn detect(path: &Path) -> Result<Option<Format>, String> {
    let mut head = [0u8; 8];
    let n = read_up_to(&mut File::open(path).map_err(|e| e.to_string())?, &mut head)?;
    let head = &head[..n];
    let codec = if head.starts_with(&[0x1f, 0x8b]) {
        Codec::Gzip
    } else if head.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        Codec::Zstd
    } else if head.starts_with(&[0xfd, b'7', b'z', b'X', b'Z', 0]) {
        Codec::Xz
    } else if head.starts_with(b"BZh") {
        Codec::Bzip2
    } else if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Ok(Some(Format::Zip));
    } else if head.starts_with(&[b'7', b'z', 0xbc, 0xaf, 0x27, 0x1c]) {
        return Ok(Some(Format::SevenZ));
    } else if head.starts_with(b"!<arch>\n") {
        return Ok(Some(Format::Ar));
    } else {
        Codec::None
    };
    let mut block = [0u8; 512];
    let n = read_up_to(&mut decoder(path, codec)?, &mut block)?;
    Ok(match (is_tar(&block[..n]), codec) {
        (true, codec) => Some(Format::Tar(codec)),
        (false, Codec::None) => None,
        (false, codec) => Some(Format::Compressed(codec)),
    })
}

/// A ustar magic, or a v7 header whose checksum adds up.
fn is_tar(block: &[u8]) -> bool {
    if block.len() < 512 {
        return false;
    }
    if &block[257..262] == b"ustar" {
        return true;
    }
    let stored = std::str::from_utf8(&block[148..156])
        .ok()
        .and_then(|s| u32::from_str_radix(s.trim_matches(['\0', ' ']), 8).ok());
    let sum: u32 = block
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                b as u32
            }
        })
        .sum();
    stored == Some(sum) && block[0] != 0
}

fn decoder(path: &Path, codec: Codec) -> Result<Box<dyn Read>, String> {
    let file = BufReader::new(File::open(path).map_err(|e| e.to_string())?);
    Ok(match codec {
        Codec::None => Box::new(file),
        Codec::Gzip => Box::new(flate2::read::MultiGzDecoder::new(file)),
        Codec::Zstd => {
            Box::new(zstd::stream::read::Decoder::with_buffer(file).map_err(|e| e.to_string())?)
        }
        Codec::Xz => Box::new(lzma_rust2::XzReader::new(file, true)),
        Codec::Bzip2 => Box::new(bzip2::read::MultiBzDecoder::new(file)),
    })
}

fn read_up_to(reader: &mut impl Read, buf: &mut [u8]) -> Result<usize, String> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(filled)
}

pub fn walk(path: &Path, format: Format, visit: &mut Visit) -> Result<(), String> {
    match format {
        Format::Tar(codec) => walk_tar(decoder(path, codec)?, visit),
        Format::Zip => walk_zip(path, visit),
        Format::SevenZ => walk_7z(path, visit),
        Format::Ar => walk_ar(path, visit),
        Format::Compressed(codec) => {
            let meta = Meta {
                path: single_name(path),
                kind: Kind::File,
                size: None,
                mode: None,
            };
            visit(&meta, &mut decoder(path, codec)?).map(|_| ())
        }
    }
}

/// `notes.txt.gz` holds `notes.txt`.
fn single_name(path: &Path) -> String {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    for ext in [".gz", ".zst", ".zstd", ".xz", ".bz2"] {
        if let Some(stem) = name.strip_suffix(ext) {
            return stem.to_string();
        }
    }
    format!("{name}.out")
}

fn walk_tar(reader: Box<dyn Read>, visit: &mut Visit) -> Result<(), String> {
    let mut archive = tar::Archive::new(reader);
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let header = entry.header();
        let kind = match header.entry_type() {
            t if t.is_file() => Kind::File,
            t if t.is_dir() => Kind::Dir,
            t if t.is_symlink() => Kind::Symlink,
            t if t.is_hard_link() => Kind::Hardlink,
            _ => Kind::Other,
        };
        let meta = Meta {
            path: String::from_utf8_lossy(&entry.path_bytes()).into_owned(),
            kind,
            size: header.size().ok(),
            mode: header.mode().ok(),
        };
        if !visit(&meta, &mut entry)? {
            break;
        }
    }
    Ok(())
}

fn walk_zip(path: &Path, visit: &mut Visit) -> Result<(), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(|e| e.to_string())?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let kind = if entry.is_symlink() {
            Kind::Symlink
        } else if entry.is_dir() {
            Kind::Dir
        } else {
            Kind::File
        };
        let meta = Meta {
            path: String::from_utf8_lossy(entry.name_raw()).into_owned(),
            kind,
            size: Some(entry.size()),
            mode: entry.unix_mode(),
        };
        if !visit(&meta, &mut entry)? {
            break;
        }
    }
    Ok(())
}

fn walk_7z(path: &Path, visit: &mut Visit) -> Result<(), String> {
    let mut reader = sevenz_rust2::ArchiveReader::open(path, sevenz_rust2::Password::empty())
        .map_err(|e| e.to_string())?;
    // sevenz-rust2 0.23 stops a solid block early when a directory sits between
    // its files, dropping the last files without an error. Count what should
    // arrive so a short read fails instead of passing as a complete archive.
    let expected = reader
        .archive()
        .files
        .iter()
        .filter(|f| f.has_stream() && f.size() > 0)
        .count();
    let mut delivered = 0;
    let mut stopped = false;
    let mut failure = None;
    reader
        .for_each_entries(|entry, data| {
            if entry.is_anti_item() {
                return Ok(true);
            }
            if entry.has_stream() && entry.size() > 0 {
                delivered += 1;
            }
            let meta = Meta {
                path: entry.name().to_string(),
                kind: if entry.is_directory() {
                    Kind::Dir
                } else {
                    Kind::File
                },
                size: Some(entry.size()),
                mode: None,
            };
            match visit(&meta, data) {
                Ok(go_on) => {
                    stopped |= !go_on;
                    Ok(go_on)
                }
                Err(err) => {
                    failure = Some(err);
                    Ok(false)
                }
            }
        })
        .map_err(|e| e.to_string())?;
    if let Some(err) = failure {
        return Err(err);
    }
    if !stopped && delivered < expected {
        return Err(format!(
            "the 7z reader returned {delivered} of {expected} files; this archive layout (a directory between files in a solid block) is not supported yet"
        ));
    }
    Ok(())
}

fn walk_ar(path: &Path, visit: &mut Visit) -> Result<(), String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mut archive = ar::Archive::new(BufReader::new(file));
    while let Some(entry) = archive.next_entry() {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let header = entry.header();
        let meta = Meta {
            path: String::from_utf8_lossy(header.identifier()).into_owned(),
            kind: Kind::File,
            size: Some(header.size()),
            mode: Some(header.mode()),
        };
        if !visit(&meta, &mut entry)? {
            break;
        }
    }
    Ok(())
}

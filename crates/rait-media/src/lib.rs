mod probe;

use rait_core::exec::{self, Dependency};
use rait_core::{ErrorCode, ErrorInfo, check_input, path_to_protocol};
use rayon::prelude::*;
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

const FFMPEG: &str = "ffmpeg";
const FFPROBE: &str = "ffprobe";
const FFMPEG_INSTALL: &str = "macOS: brew install ffmpeg; Debian/Ubuntu: sudo apt install ffmpeg; Fedora: sudo dnf install ffmpeg";

pub const EXTENSIONS: &[&str] = &[
    "mp3", "wav", "flac", "ogg", "oga", "opus", "m4a", "aac", "aiff", "aif", "caf", "mp4", "m4v",
    "mov", "mkv", "webm", "avi", "wmv", "flv", "mpg", "mpeg", "ts",
];

pub const DEPENDENCIES: &[Dependency] = &[
    Dependency {
        program: FFMPEG,
        capabilities: &["media.frames", "media.audio"],
        fallback: None,
        install: FFMPEG_INSTALL,
        version_flag: "-version",
    },
    Dependency {
        program: FFPROBE,
        capabilities: &["media.info"],
        fallback: Some("symphonia"),
        install: FFMPEG_INSTALL,
        version_flag: "-version",
    },
];

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MediaInfo {
    /// `video` when there is a video stream, else `audio`.
    pub kind: String,
    /// Seconds.
    pub duration: Option<f64>,
    /// Codec of the main stream: video for video files, else audio.
    pub codec: Option<String>,
    /// Bits per second over the whole file.
    pub bitrate: Option<u64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub audio_codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    pub title: Option<String>,
    pub artist: Option<String>,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

impl MediaInfo {
    fn fill_from(&mut self, other: MediaInfo) {
        self.duration = self.duration.or(other.duration);
        self.bitrate = self.bitrate.or(other.bitrate);
        self.title = self.title.take().or(other.title);
        self.artist = self.artist.take().or(other.artist);
    }
}

#[derive(Clone, Debug)]
pub struct FramesRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    /// Frames, evenly spaced over the video.
    pub count: u32,
    /// Shrink so the longer side is at most this many pixels.
    pub max: Option<u32>,
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frame {
    /// Seconds from the start.
    pub time: f64,
    pub file: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Frames {
    pub frames: Vec<Frame>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioFormat {
    Wav,
    Mp3,
    Flac,
    Ogg,
    M4a,
}

impl AudioFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Wav => "wav",
            Self::Mp3 => "mp3",
            Self::Flac => "flac",
            Self::Ogg => "ogg",
            Self::M4a => "m4a",
        }
    }

    fn codec(self) -> &'static str {
        match self {
            Self::Wav => "pcm_s16le",
            Self::Mp3 => "libmp3lame",
            Self::Flac => "flac",
            Self::Ogg => "libopus",
            Self::M4a => "aac",
        }
    }
}

#[derive(Clone, Debug)]
pub struct AudioRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    pub format: AudioFormat,
    pub mono: bool,
    /// Sample rate in Hz; the source rate by default.
    pub rate: Option<u32>,
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Audio {
    pub output: String,
}

/// Media details, read with the built-in demuxers or `ffprobe` for formats they
/// do not cover.
pub fn info(path: &Path) -> Result<MediaInfo, ErrorInfo> {
    check_input(path)?;
    let mut info = match probe::native(path) {
        // Some containers, such as Matroska, keep duration and tags where the
        // built-in reader does not look; fill only those gaps from ffprobe.
        Ok(mut info) => {
            if info.duration.is_none()
                && let Ok(more) = probe::ffprobe(path)
            {
                info.fill_from(more);
            }
            info
        }
        Err(native) => match probe::ffprobe(path) {
            Ok(info) => info,
            Err(exec::ExecError::Missing(_)) => {
                return Err(ErrorInfo::new(
                    ErrorCode::InvalidInput,
                    "The built-in reader does not support this media file.",
                    "Install FFmpeg (run `rait doctor`) so `ffprobe` can read it.".to_string(),
                )
                .with_detail(native));
            }
            Err(err) => return Err(unreadable(err.to_string())),
        },
    };
    let bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if info.bitrate.is_none()
        && let Some(seconds) = info.duration.filter(|d| *d > 0.0)
    {
        info.bitrate = Some((bytes as f64 * 8.0 / seconds).round() as u64);
    }
    Ok(info)
}

pub fn frames(req: &FramesRequest) -> Result<Frames, ErrorInfo> {
    let details = info(&req.path)?;
    if details.kind != "video" {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The file has no video stream.",
            "Use `rait media audio` for audio files.".to_string(),
        ));
    }
    let duration = details.duration.unwrap_or(0.0);
    let count = req.count.max(1);
    let stem = stem(&req.path)?;
    let width = count.to_string().len();
    let planned: Vec<(f64, PathBuf)> = (0..count)
        .map(|i| {
            let time = duration * (f64::from(i) + 0.5) / f64::from(count);
            let file = req.output_dir.join(format!("{stem}-{:0width$}.jpg", i + 1));
            ((time * 1000.0).round() / 1000.0, file)
        })
        .collect();
    let targets: Vec<PathBuf> = planned.iter().map(|(_, f)| f.clone()).collect();
    prepare(&req.output_dir, &targets, req.force)?;
    let scale = req.max.map(|max| {
        format!("scale='min({max},iw)':'min({max},ih)':force_original_aspect_ratio=decrease")
    });
    planned
        .par_iter()
        .map(|(time, file)| {
            let mut args: Vec<OsString> = vec![
                "-v".into(),
                "error".into(),
                "-nostdin".into(),
                "-y".into(),
                "-ss".into(),
                format!("{time:.3}").into(),
                "-i".into(),
                req.path.clone().into(),
                "-frames:v".into(),
                "1".into(),
            ];
            if let Some(scale) = &scale {
                args.extend(["-vf".into(), scale.into()]);
            }
            args.push(file.clone().into());
            run_ffmpeg(args)?;
            Ok(Frame {
                time: *time,
                file: written(file)?,
            })
        })
        .collect::<Result<Vec<_>, ErrorInfo>>()
        .map(|frames| Frames { frames })
}

pub fn audio(req: &AudioRequest) -> Result<Audio, ErrorInfo> {
    let details = info(&req.path)?;
    if details.audio_codec.is_none() {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The file has no audio stream.",
            "Check the file with `rait media info <file>`.".to_string(),
        ));
    }
    let output = req
        .output_dir
        .join(format!("{}.{}", stem(&req.path)?, req.format.extension()));
    if same_file(&req.path, &output) {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The output would replace the input.",
            "Choose a different --output directory or --to format.".to_string(),
        ));
    }
    prepare(&req.output_dir, std::slice::from_ref(&output), req.force)?;
    let mut args: Vec<OsString> = vec![
        "-v".into(),
        "error".into(),
        "-nostdin".into(),
        "-y".into(),
        "-i".into(),
        req.path.clone().into(),
        "-vn".into(),
        "-c:a".into(),
        req.format.codec().into(),
    ];
    if req.mono {
        args.extend(["-ac".into(), "1".into()]);
    }
    if let Some(rate) = req.rate {
        args.extend(["-ar".into(), rate.to_string().into()]);
    }
    args.push(output.clone().into());
    run_ffmpeg(args)?;
    Ok(Audio {
        output: written(&output)?,
    })
}

fn run_ffmpeg(args: Vec<OsString>) -> Result<(), ErrorInfo> {
    exec::run(FFMPEG, args)
        .map(|_| ())
        .map_err(|err| match err {
            exec::ExecError::Missing(_) => err.into(),
            _ => ErrorInfo::new(
                ErrorCode::DependencyFailed,
                "`ffmpeg` could not process the file.",
                "Check the file with `rait media info <file>`.".to_string(),
            )
            .with_detail(err.to_string()),
        })
}

fn prepare(dir: &Path, targets: &[PathBuf], force: bool) -> Result<(), ErrorInfo> {
    if !force && let Some(existing) = targets.iter().find(|t| t.exists()) {
        return Err(ErrorInfo::new(
            ErrorCode::OutputExists,
            format!("Output {} already exists.", existing.display()),
            "Use --force to replace existing files.".to_string(),
        ));
    }
    fs::create_dir_all(dir).map_err(|e| {
        ErrorInfo::new(
            ErrorCode::PermissionDenied,
            format!("Failed to create the output directory: {e}"),
            None,
        )
    })
}

fn written(path: &Path) -> Result<String, ErrorInfo> {
    if !path.exists() {
        return Err(ErrorInfo::new(
            ErrorCode::DependencyFailed,
            format!("`ffmpeg` did not write {}.", path.display()),
            "Check the file with `rait media info <file>`.".to_string(),
        ));
    }
    path_to_protocol(path)
}

fn stem(path: &Path) -> Result<String, ErrorInfo> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(String::from)
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorCode::InvalidInput,
                "Input path has no UTF-8 file name.",
                "Use a UTF-8 file name.".to_string(),
            )
        })
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

fn unreadable(detail: String) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        "The file is not a readable audio or video file.",
        "Check the file, or re-encode it with ffmpeg.".to_string(),
    )
    .with_detail(detail)
}

use image::codecs::jpeg::JpegEncoder;
use image::codecs::webp::WebPEncoder;
use image::imageops::FilterType;
use image::metadata::Orientation;
use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader, Rgb, RgbImage};
use rait_core::exec::{self, Dependency, ExecError};
use rait_core::{ErrorCode, ErrorInfo, check_input, path_to_protocol};
use serde::Serialize;
use std::ffi::OsString;
use std::fs;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

const TESSERACT: &str = "tesseract";

pub const EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "pnm", "pbm", "pgm", "ppm",
    "qoi", "tga",
];

pub const DEPENDENCIES: &[Dependency] = &[Dependency {
    program: TESSERACT,
    capabilities: &["image.ocr"],
    fallback: None,
    install: "macOS: brew install tesseract; Debian/Ubuntu: sudo apt install tesseract-ocr; Fedora: sudo dnf install tesseract",
    version_flag: "--version",
}];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ImageInfo {
    pub format: String,
    /// Displayed size, after the EXIF orientation is applied.
    pub width: u32,
    pub height: u32,
    pub color: String,
    /// EXIF orientation, 1 to 8; 1 means stored upright.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<u32>,
    /// EXIF capture time as `YYYY-MM-DDTHH:MM:SS`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taken: Option<String>,
    /// EXIF make and model.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub camera: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Png,
    Jpeg,
    Webp,
    Gif,
    Bmp,
    Tiff,
}

impl Target {
    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Tiff => "tif",
        }
    }

    /// The same format as the source when it can be written, else PNG.
    fn from_source(format: ImageFormat) -> Self {
        match format {
            ImageFormat::Jpeg => Self::Jpeg,
            ImageFormat::WebP => Self::Webp,
            ImageFormat::Gif => Self::Gif,
            ImageFormat::Bmp => Self::Bmp,
            ImageFormat::Tiff => Self::Tiff,
            _ => Self::Png,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConvertRequest {
    pub path: PathBuf,
    pub output_dir: PathBuf,
    /// Output format; the source format by default.
    pub to: Option<Target>,
    /// Shrink so the longer side is at most this many pixels. Never enlarges.
    pub max: Option<u32>,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
    pub force: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Converted {
    pub output: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug)]
pub struct OcrRequest {
    pub path: PathBuf,
    /// Tesseract language codes, such as `eng` or `eng+deu`.
    pub lang: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OcrText {
    pub lang: String,
    pub text: String,
}

pub fn info(path: &Path) -> Result<ImageInfo, ErrorInfo> {
    let (format, mut decoder) = open(path)?;
    let exif = decoder.exif_metadata().ok().flatten().and_then(|raw| {
        // JPEG APP1 data may still carry its `Exif\0\0` marker.
        let tiff = raw
            .strip_prefix(b"Exif\0\0")
            .map_or(raw.clone(), <[u8]>::to_vec);
        // Real files often have small EXIF faults; keep what parses.
        let mut reader = exif::Reader::new();
        reader.continue_on_error(true);
        match reader.read_raw(tiff) {
            Ok(exif) => Some(exif),
            Err(exif::Error::PartialResult(partial)) => Some(partial.into_inner().0),
            Err(_) => None,
        }
    });
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let (mut width, mut height) = decoder.dimensions();
    if swaps_sides(orientation) {
        (width, height) = (height, width);
    }
    let field = |tag| {
        let field = exif.as_ref()?.get_field(tag, exif::In::PRIMARY)?;
        let text = field.display_value().to_string();
        let text = text.trim().trim_matches('"').trim().to_string();
        (!text.is_empty()).then_some(text)
    };
    let camera = match (field(exif::Tag::Make), field(exif::Tag::Model)) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (make, model) => make.or(model),
    };
    Ok(ImageInfo {
        format: format_name(format),
        width,
        height,
        color: format!("{:?}", decoder.color_type()).to_lowercase(),
        orientation: exif.as_ref().and_then(|exif| {
            exif.get_field(exif::Tag::Orientation, exif::In::PRIMARY)?
                .value
                .get_uint(0)
        }),
        taken: field(exif::Tag::DateTimeOriginal)
            .or_else(|| field(exif::Tag::DateTime))
            .map(|t| iso_time(&t)),
        camera,
    })
}

pub fn convert(req: &ConvertRequest) -> Result<Converted, ErrorInfo> {
    let (format, mut decoder) = open(&req.path)?;
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let mut image = DynamicImage::from_decoder(decoder).map_err(unreadable)?;
    image.apply_orientation(orientation);
    if let Some(max) = req.max
        && image.width().max(image.height()) > max
    {
        image = image.resize(max, max, FilterType::Lanczos3);
    }
    let target = req.to.unwrap_or(Target::from_source(format));
    let stem = req
        .path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or_else(|| {
            ErrorInfo::new(
                ErrorCode::InvalidInput,
                "Input path has no UTF-8 file name.",
                "Use a UTF-8 file name.".to_string(),
            )
        })?;
    let output = req
        .output_dir
        .join(format!("{stem}.{}", target.extension()));
    if same_file(&req.path, &output) {
        return Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The output would replace the input.",
            "Choose a different --output directory or --to format.".to_string(),
        ));
    }
    let shown = path_to_protocol(&output)?;
    if output.exists() && !req.force {
        return Err(ErrorInfo::new(
            ErrorCode::OutputExists,
            format!("Output {shown} already exists."),
            "Use --force to replace existing files.".to_string(),
        ));
    }
    fs::create_dir_all(&req.output_dir)
        .map_err(|e| write_error("create the output directory", e))?;
    let tmp = output.with_extension(format!("{}.rait-tmp", target.extension()));
    let written = write(&image, target, req.quality, &tmp).and_then(|()| {
        fs::rename(&tmp, &output).map_err(|e| write_error("move the output into place", e))
    });
    if written.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    written?;
    Ok(Converted {
        output: shown,
        width: image.width(),
        height: image.height(),
    })
}

pub fn ocr(req: &OcrRequest) -> Result<OcrText, ErrorInfo> {
    open(&req.path)?;
    let args: Vec<OsString> = vec![
        req.path.clone().into(),
        "stdout".into(),
        "-l".into(),
        req.lang.clone().into(),
    ];
    let out = exec::run(TESSERACT, args).map_err(|err| match &err {
        ExecError::Failed { stderr, .. } if stderr.contains("Failed loading language") => {
            ErrorInfo::new(
                ErrorCode::InvalidInput,
                format!("Tesseract has no language data for `{}`.", req.lang),
                "Install that language's traineddata, or use --lang eng.".to_string(),
            )
            .with_detail(err.to_string())
        }
        _ => err.into(),
    })?;
    Ok(OcrText {
        lang: req.lang.clone(),
        text: String::from_utf8_lossy(&out.stdout).trim_end().to_string(),
    })
}

fn open(path: &Path) -> Result<(ImageFormat, impl ImageDecoder), ErrorInfo> {
    check_input(path)?;
    let reader = ImageReader::open(path)
        .and_then(|r| r.with_guessed_format())
        .map_err(|e| unreadable(e.into()))?;
    let format = reader.format().ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            "The file is not a supported image.",
            format!("Supported: {}.", EXTENSIONS.join(", ")),
        )
    })?;
    let decoder = reader.into_decoder().map_err(unreadable)?;
    Ok((format, decoder))
}

fn write(image: &DynamicImage, target: Target, quality: u8, path: &Path) -> Result<(), ErrorInfo> {
    let file = fs::File::create(path).map_err(|e| write_error("write the output", e))?;
    let mut out = BufWriter::new(file);
    let alpha = image.color().has_alpha();
    let encoded = match target {
        Target::Jpeg => JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100))
            .encode_image(&flatten(image)),
        Target::Webp if alpha => WebPEncoder::new_lossless(&mut out).encode(
            image.to_rgba8().as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgba8,
        ),
        Target::Webp => WebPEncoder::new_lossless(&mut out).encode(
            image.to_rgb8().as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        ),
        Target::Png => image.write_to(&mut out, ImageFormat::Png),
        Target::Gif => {
            DynamicImage::ImageRgba8(image.to_rgba8()).write_to(&mut out, ImageFormat::Gif)
        }
        Target::Bmp => image.write_to(&mut out, ImageFormat::Bmp),
        Target::Tiff => image.write_to(&mut out, ImageFormat::Tiff),
    };
    encoded.map_err(|e| {
        ErrorInfo::new(
            ErrorCode::InternalError,
            format!("Failed to encode the image as {}.", target.extension()),
            "Try another --to format.".to_string(),
        )
        .with_detail(e.to_string())
    })?;
    out.flush().map_err(|e| write_error("write the output", e))
}

/// JPEG has no alpha: composite transparent pixels over white.
fn flatten(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend = |c: u8| ((c as u16 * a as u16 + 255 * (255 - a as u16)) / 255) as u8;
        Rgb([blend(r), blend(g), blend(b)])
    })
}

fn swaps_sides(orientation: Orientation) -> bool {
    matches!(
        orientation,
        Orientation::Rotate90
            | Orientation::Rotate270
            | Orientation::Rotate90FlipH
            | Orientation::Rotate270FlipH
    )
}

fn format_name(format: ImageFormat) -> String {
    format!("{format:?}").to_lowercase()
}

/// EXIF writes times as `YYYY:MM:DD HH:MM:SS`.
fn iso_time(exif: &str) -> String {
    match exif.split_once(' ') {
        Some((date, time)) if date.len() == 10 => format!("{}T{time}", date.replace(':', "-")),
        _ => exif.to_string(),
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    matches!((a.canonicalize(), b.canonicalize()), (Ok(a), Ok(b)) if a == b)
}

fn unreadable(err: image::ImageError) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        "The file is not a supported image, or it is damaged.",
        "Check the file, or convert it to PNG or JPEG.".to_string(),
    )
    .with_detail(err.to_string())
}

fn write_error(action: &str, err: std::io::Error) -> ErrorInfo {
    let code = if err.kind() == std::io::ErrorKind::PermissionDenied {
        ErrorCode::PermissionDenied
    } else {
        ErrorCode::InternalError
    };
    ErrorInfo::new(code, format!("Failed to {action}: {err}"), None)
}

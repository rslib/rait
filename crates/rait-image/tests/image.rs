use rait_core::ErrorCode;
use rait_image::{ConvertRequest, OcrRequest, Target, convert, info, ocr};
use std::path::{Path, PathBuf};
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/fixtures")).join(name)
}

fn request(name: &str, dir: &Path) -> ConvertRequest {
    ConvertRequest {
        path: fixture(name),
        output_dir: dir.into(),
        to: None,
        max: None,
        quality: 85,
        force: false,
    }
}

#[test]
fn reads_formats_and_exif() {
    let photo = info(&fixture("photo.jpg")).unwrap();
    assert_eq!(
        (photo.format.as_str(), photo.width, photo.height),
        ("jpeg", 40, 60)
    );
    assert_eq!(photo.orientation, Some(6));
    assert_eq!(photo.taken.as_deref(), Some("2026-01-02T03:04:05"));
    assert_eq!(photo.camera.as_deref(), Some("RAIT Fixture Cam"));
    for (name, format, w, h) in [
        ("text.png", "png", 700, 220),
        ("alpha.png", "png", 32, 16),
        ("tiny.gif", "gif", 8, 8),
        ("tiny.webp", "webp", 10, 6),
    ] {
        let got = info(&fixture(name)).unwrap();
        assert_eq!(
            (got.format.as_str(), got.width, got.height),
            (format, w, h),
            "{name}"
        );
        assert_eq!(got.orientation, None, "{name}");
    }
    assert_eq!(info(&fixture("alpha.png")).unwrap().color, "rgba8");
}

#[test]
fn convert_applies_orientation_and_shrinks() {
    let dir = tempfile::tempdir().unwrap();
    let out = convert(&ConvertRequest {
        to: Some(Target::Png),
        ..request("photo.jpg", dir.path())
    })
    .unwrap();
    assert_eq!((out.width, out.height), (40, 60));
    let upright = info(Path::new(&out.output)).unwrap();
    assert_eq!(
        (upright.width, upright.height, upright.orientation),
        (40, 60, None)
    );
    let small = convert(&ConvertRequest {
        max: Some(20),
        to: Some(Target::Webp),
        ..request("photo.jpg", dir.path())
    })
    .unwrap();
    assert_eq!((small.width, small.height), (13, 20));
    let same = convert(&ConvertRequest {
        max: Some(1000),
        ..request("tiny.gif", dir.path())
    })
    .unwrap();
    assert_eq!((same.width, same.height), (8, 8));
    assert!(same.output.ends_with("tiny.gif"));
}

#[test]
fn jpeg_output_flattens_alpha_onto_white() {
    let dir = tempfile::tempdir().unwrap();
    let out = convert(&ConvertRequest {
        to: Some(Target::Jpeg),
        ..request("alpha.png", dir.path())
    })
    .unwrap();
    let pixels = image::open(&out.output).unwrap().to_rgb8();
    let [r, g, b] = pixels.get_pixel(2, 8).0;
    assert!(
        r > 240 && g > 240 && b > 240,
        "transparent side should be white"
    );
    let [r, g, b] = pixels.get_pixel(29, 8).0;
    assert!(r < 30 && g > 220 && b < 30, "opaque side stays green");
}

#[test]
fn convert_refuses_to_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    convert(&request("tiny.webp", dir.path())).unwrap();
    let err = convert(&request("tiny.webp", dir.path())).unwrap_err();
    assert_eq!(err.code, ErrorCode::OutputExists);
    assert!(
        convert(&ConvertRequest {
            force: true,
            ..request("tiny.webp", dir.path())
        })
        .is_ok()
    );
    let err = convert(&request("tiny.webp", fixture("").as_path())).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(!dir.path().join("tiny.webp.rait-tmp").exists());
}

#[test]
fn rejects_non_images() {
    let err = info(&fixture("simple.pdf")).unwrap_err();
    assert_eq!(err.code, ErrorCode::InvalidInput);
    assert!(err.hint.unwrap().contains("png"));
    assert_eq!(
        info(&fixture("missing.png")).unwrap_err().code,
        ErrorCode::FileNotFound
    );
}

#[test]
fn ocr_reads_text() {
    if Command::new("tesseract").arg("--version").output().is_err() {
        eprintln!("skipped: tesseract is not installed");
        return;
    }
    let out = ocr(&OcrRequest {
        path: fixture("text.png"),
        lang: "eng".into(),
    })
    .unwrap();
    assert!(out.text.contains("RAIT OCR TEST"));
    assert!(out.text.contains("Invoice 4217"));
    let err = ocr(&OcrRequest {
        path: fixture("text.png"),
        lang: "zzz".into(),
    })
    .unwrap_err();
    assert!(err.message.contains("`zzz`"));
}

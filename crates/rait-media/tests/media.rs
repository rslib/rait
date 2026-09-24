use rait_core::ErrorCode;
use rait_media::{AudioFormat, AudioRequest, FramesRequest, audio, frames, info};
use std::path::PathBuf;
use std::process::Command;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/media"
    ))
    .join(name)
}

fn has_ffmpeg() -> bool {
    let found = Command::new("ffmpeg").arg("-version").output().is_ok();
    if !found {
        eprintln!("skipped: ffmpeg is not installed");
    }
    found
}

#[test]
fn reads_audio_formats_natively() {
    for (name, codec) in [
        ("tone.wav", "pcm_s16"),
        ("tone.flac", "flac"),
        ("tone.mp3", "mp3"),
        ("tone.ogg", "vorbis"),
        ("tone.m4a", "aac"),
    ] {
        let got = info(&fixture(name)).unwrap();
        assert_eq!(
            (got.kind.as_str(), got.codec.as_deref()),
            ("audio", Some(codec)),
            "{name}"
        );
        assert_eq!(got.sample_rate, Some(16000), "{name}");
        assert!((got.duration.unwrap() - 1.0).abs() < 0.1, "{name}");
        assert!(got.bitrate.unwrap() > 0, "{name}");
    }
    let flac = info(&fixture("tone.flac")).unwrap();
    assert_eq!(
        (flac.title.as_deref(), flac.artist.as_deref()),
        (Some("RAIT Tone"), Some("RAIT"))
    );
    assert_eq!(flac.channels, Some(1));
}

#[test]
fn reads_video_formats() {
    let clip = info(&fixture("clip.mp4")).unwrap();
    assert_eq!(
        (clip.kind.as_str(), clip.codec.as_deref()),
        ("video", Some("h264"))
    );
    assert_eq!((clip.width, clip.height), (Some(160), Some(90)));
    assert_eq!(clip.audio_codec.as_deref(), Some("aac"));
    assert_eq!(clip.title.as_deref(), Some("RAIT Clip"));
    let silent = info(&fixture("silent.mp4")).unwrap();
    assert_eq!((silent.kind.as_str(), silent.audio_codec), ("video", None));
    let mkv = info(&fixture("clip.mkv")).unwrap();
    assert_eq!((mkv.codec.as_deref(), mkv.width), (Some("h264"), Some(160)));
    if has_ffmpeg() {
        assert!((mkv.duration.unwrap() - 3.0).abs() < 0.2);
    }
}

#[test]
fn frames_are_evenly_spaced_and_bounded() {
    if !has_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let req = FramesRequest {
        path: fixture("clip.mp4"),
        output_dir: dir.path().into(),
        count: 3,
        max: Some(80),
        force: false,
    };
    let got = frames(&req).unwrap();
    let times: Vec<f64> = got.frames.iter().map(|f| f.time).collect();
    assert_eq!(times.len(), 3);
    assert!(times.windows(2).all(|w| w[0] < w[1]));
    assert!(got.frames[0].file.ends_with("clip-1.jpg"));
    let size = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
        ])
        .arg(&got.frames[0].file)
        .output()
        .unwrap();
    assert_eq!(String::from_utf8_lossy(&size.stdout).trim(), "80,45");
    assert_eq!(frames(&req).unwrap_err().code, ErrorCode::OutputExists);
    let err = frames(&FramesRequest {
        path: fixture("tone.wav"),
        ..req
    })
    .unwrap_err();
    assert!(err.message.contains("no video"));
}

#[test]
fn audio_extracts_speech_ready_wav() {
    if !has_ffmpeg() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let req = AudioRequest {
        path: fixture("clip.mp4"),
        output_dir: dir.path().into(),
        format: AudioFormat::Wav,
        mono: true,
        rate: Some(8000),
        force: false,
    };
    let got = audio(&req).unwrap();
    let wav = info(std::path::Path::new(&got.output)).unwrap();
    assert_eq!(
        (wav.codec.as_deref(), wav.sample_rate, wav.channels),
        (Some("pcm_s16"), Some(8000), Some(1))
    );
    for format in [
        AudioFormat::Mp3,
        AudioFormat::Flac,
        AudioFormat::Ogg,
        AudioFormat::M4a,
    ] {
        audio(&AudioRequest {
            format,
            rate: None,
            ..req.clone()
        })
        .unwrap();
    }
    let err = audio(&AudioRequest {
        path: fixture("silent.mp4"),
        ..req
    })
    .unwrap_err();
    assert!(err.message.contains("no audio"));
}

#[test]
fn rejects_non_media() {
    let pdf = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/simple.pdf"
    ));
    assert_eq!(info(&pdf).unwrap_err().code, ErrorCode::InvalidInput);
}

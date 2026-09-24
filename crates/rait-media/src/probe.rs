use crate::{FFPROBE, MediaInfo};
use rait_core::exec::{self, ExecError};
use serde_json::Value;
use std::fs::File;
use std::path::Path;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::{AudioCodecId, well_known as audio};
use symphonia::core::codecs::video::{VideoCodecId, well_known as video};
use symphonia::core::formats::FormatOptions;
use symphonia::core::formats::probe::Hint;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::{MetadataOptions, StandardTag};
use symphonia::core::units::Timestamp;

/// Read with symphonia's demuxers. `Err` means the format is not covered.
pub fn native(path: &Path) -> Result<MediaInfo, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| e.to_string())?;
    let mut info = MediaInfo {
        kind: "audio".into(),
        ..MediaInfo::default()
    };
    let mut known = false;
    for track in format.tracks() {
        let seconds = track
            .time_base
            .zip(track.duration)
            .and_then(|(base, duration)| {
                base.calc_time(Timestamp::new(i64::try_from(duration.get()).ok()?))
            })
            .map(|time| time.as_secs_f64());
        if let Some(seconds) = seconds {
            info.duration = Some(info.duration.map_or(seconds, |d| d.max(seconds)));
        }
        match &track.codec_params {
            Some(CodecParameters::Video(params)) if info.width.is_none() => {
                known = true;
                info.kind = "video".into();
                info.codec = Some(video_name(params.codec));
                info.width = params.width.map(u32::from);
                info.height = params.height.map(u32::from);
            }
            Some(CodecParameters::Audio(params)) if info.audio_codec.is_none() => {
                known = true;
                info.audio_codec = Some(audio_name(params.codec));
                info.sample_rate = params.sample_rate;
                info.channels = params.channels.as_ref().map(|c| c.count() as u32);
            }
            _ => {}
        }
    }
    if !known {
        return Err("no audio or video track the built-in reader understands".into());
    }
    if info.kind == "audio" {
        info.codec = info.audio_codec.clone();
    }
    if let Some(revision) = format.metadata().current() {
        for tag in &revision.media.tags {
            match &tag.std {
                Some(StandardTag::TrackTitle(title)) => info.title = Some(title.to_string()),
                Some(StandardTag::Artist(artist)) => info.artist = Some(artist.to_string()),
                _ => {}
            }
        }
    }
    Ok(info)
}

/// Read with `ffprobe` from FFmpeg, for formats symphonia does not cover.
pub fn ffprobe(path: &Path) -> Result<MediaInfo, ExecError> {
    let out = exec::run(
        FFPROBE,
        [
            "-v".as_ref(),
            "error".as_ref(),
            "-print_format".as_ref(),
            "json".as_ref(),
            "-show_format".as_ref(),
            "-show_streams".as_ref(),
            path.as_os_str(),
        ],
    )?;
    let json: Value = serde_json::from_slice(&out.stdout).unwrap_or(Value::Null);
    let streams = json["streams"].as_array().cloned().unwrap_or_default();
    let of_type = |kind: &str| streams.iter().find(|s| s["codec_type"] == kind);
    let number = |v: &Value| {
        v.as_str()
            .and_then(|s| s.parse::<f64>().ok())
            .or(v.as_f64())
    };
    let tag = |key: &str| {
        let tags = &json["format"]["tags"];
        tags[key]
            .as_str()
            .or_else(|| tags[key.to_uppercase()].as_str())
            .map(String::from)
    };
    let video = of_type("video");
    let audio = of_type("audio");
    let audio_codec = audio
        .and_then(|a| a["codec_name"].as_str())
        .map(String::from);
    Ok(MediaInfo {
        kind: if video.is_some() { "video" } else { "audio" }.into(),
        duration: number(&json["format"]["duration"]),
        codec: video
            .and_then(|v| v["codec_name"].as_str())
            .map(String::from)
            .or_else(|| audio_codec.clone()),
        bitrate: number(&json["format"]["bit_rate"]).map(|b| b as u64),
        width: video.and_then(|v| v["width"].as_u64()).map(|w| w as u32),
        height: video.and_then(|v| v["height"].as_u64()).map(|h| h as u32),
        audio_codec,
        sample_rate: audio
            .and_then(|a| number(&a["sample_rate"]))
            .map(|r| r as u32),
        channels: audio.and_then(|a| a["channels"].as_u64()).map(|c| c as u32),
        title: tag("title"),
        artist: tag("artist"),
        warnings: vec![],
    })
}

fn audio_name(id: AudioCodecId) -> String {
    let name = match id {
        audio::CODEC_ID_MP1 => "mp1",
        audio::CODEC_ID_MP2 => "mp2",
        audio::CODEC_ID_MP3 => "mp3",
        audio::CODEC_ID_AAC => "aac",
        audio::CODEC_ID_ALAC => "alac",
        audio::CODEC_ID_FLAC => "flac",
        audio::CODEC_ID_VORBIS => "vorbis",
        audio::CODEC_ID_OPUS => "opus",
        audio::CODEC_ID_AC3 => "ac3",
        audio::CODEC_ID_EAC3 => "eac3",
        audio::CODEC_ID_WMA => "wma",
        audio::CODEC_ID_PCM_S16LE | audio::CODEC_ID_PCM_S16BE => "pcm_s16",
        audio::CODEC_ID_PCM_S24LE | audio::CODEC_ID_PCM_S24BE => "pcm_s24",
        audio::CODEC_ID_PCM_S32LE | audio::CODEC_ID_PCM_S32BE => "pcm_s32",
        audio::CODEC_ID_PCM_F32LE | audio::CODEC_ID_PCM_F32BE => "pcm_f32",
        audio::CODEC_ID_PCM_U8 => "pcm_u8",
        audio::CODEC_ID_PCM_ALAW => "pcm_alaw",
        audio::CODEC_ID_PCM_MULAW => "pcm_mulaw",
        _ => return format!("unknown ({id})"),
    };
    name.to_string()
}

fn video_name(id: VideoCodecId) -> String {
    let name = match id {
        video::CODEC_ID_H264 => "h264",
        video::CODEC_ID_HEVC => "hevc",
        video::CODEC_ID_VVC => "vvc",
        video::CODEC_ID_AV1 => "av1",
        video::CODEC_ID_VP8 => "vp8",
        video::CODEC_ID_VP9 => "vp9",
        video::CODEC_ID_MPEG4 => "mpeg4",
        video::CODEC_ID_MPEG2 => "mpeg2",
        video::CODEC_ID_MPEG1 => "mpeg1",
        video::CODEC_ID_MJPEG => "mjpeg",
        video::CODEC_ID_THEORA => "theora",
        video::CODEC_ID_H263 => "h263",
        video::CODEC_ID_VC1 => "vc1",
        _ => return format!("unknown ({id})"),
    };
    name.to_string()
}

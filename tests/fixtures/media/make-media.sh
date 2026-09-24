#!/bin/sh
# Generates synthetic audio/video fixtures with ffmpeg lavfi sources.
# Run from the repository root: ./tests/fixtures/media/make-media.sh
set -eu

FFMPEG=ffmpeg
OUT_DIR="tests/fixtures/media"

if [ ! -d "$OUT_DIR" ]; then
    echo "must be run from repository root (expected $OUT_DIR to exist)" >&2
    exit 1
fi

DET_FLAGS="-fflags +bitexact -flags:v +bitexact -flags:a +bitexact"

# 1. tone.wav: 1.0s, 440 Hz sine, 16 kHz, mono, pcm_s16le
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1.0" \
    $DET_FLAGS \
    -map_metadata -1 -metadata title="RAIT Tone" -metadata artist="RAIT" \
    -ac 1 -ar 16000 -c:a pcm_s16le \
    "$OUT_DIR/tone.wav"

# 2. tone.flac: same audio, FLAC
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1.0" \
    $DET_FLAGS \
    -map_metadata -1 -metadata title="RAIT Tone" -metadata artist="RAIT" \
    -ac 1 -ar 16000 -c:a flac \
    "$OUT_DIR/tone.flac"

# 3. tone.mp3: same audio, libmp3lame, 64 kb/s, ID3v2 metadata
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1.0" \
    $DET_FLAGS \
    -map_metadata -1 -metadata title="RAIT Tone" -metadata artist="RAIT" \
    -ac 1 -ar 16000 -c:a libmp3lame -b:a 64k -write_id3v2 1 \
    "$OUT_DIR/tone.mp3"

# 4. tone.ogg: same audio, Vorbis. Prefer libvorbis; this ffmpeg build only
#    has the native experimental "vorbis" encoder, so fall back to that
#    (requires -strict -2); skip entirely if neither is available.
if $FFMPEG -hide_banner -encoders 2>/dev/null | grep -q "libvorbis"; then
    $FFMPEG -y -hide_banner -loglevel error \
        -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1.0" \
        $DET_FLAGS \
        -map_metadata -1 -metadata title="RAIT Tone" -metadata artist="RAIT" \
        -ac 1 -ar 16000 -c:a libvorbis -q:a 1 \
        "$OUT_DIR/tone.ogg"
elif $FFMPEG -hide_banner -encoders 2>/dev/null | grep -qE "^\s*A.*X.*D\s+vorbis\s"; then
    # The native "vorbis" encoder only supports 2 channels, unlike
    # libvorbis, so this one file is stereo instead of mono.
    $FFMPEG -y -hide_banner -loglevel error \
        -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1.0" \
        $DET_FLAGS \
        -map_metadata -1 -metadata title="RAIT Tone" -metadata artist="RAIT" \
        -ac 2 -ar 16000 -c:a vorbis -strict -2 -q:a 3 \
        "$OUT_DIR/tone.ogg"
else
    echo "skipping tone.ogg: no vorbis encoder available in this ffmpeg build"
fi

# 5. tone.m4a: same audio, AAC in MP4
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "sine=frequency=440:sample_rate=16000:duration=1.0" \
    $DET_FLAGS \
    -map_metadata -1 -metadata title="RAIT Tone" -metadata artist="RAIT" \
    -ac 1 -ar 16000 -c:a aac -b:a 64k \
    "$OUT_DIR/tone.m4a"

# 6. clip.mp4: 3.0s video (testsrc2, 160x90, 10fps, H.264) + AAC mono 16kHz
#    sine audio (660 Hz)
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=160x90:rate=10:duration=3.0" \
    -f lavfi -i "sine=frequency=660:sample_rate=16000:duration=3.0" \
    $DET_FLAGS \
    -map_metadata -1 -metadata title="RAIT Clip" \
    -c:v libx264 -pix_fmt yuv420p -crf 35 -preset veryslow -threads 1 \
    -x264-params threads=1:sliced_threads=0 \
    -c:a aac -b:a 32k -ac 1 -ar 16000 \
    -movflags +faststart \
    "$OUT_DIR/clip.mp4"

# 7. clip.mkv: same video+audio, Matroska container (H.264 + AAC)
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=160x90:rate=10:duration=3.0" \
    -f lavfi -i "sine=frequency=660:sample_rate=16000:duration=3.0" \
    $DET_FLAGS \
    -map_metadata -1 -metadata title="RAIT Clip" \
    -c:v libx264 -pix_fmt yuv420p -crf 35 -preset veryslow -threads 1 \
    -x264-params threads=1:sliced_threads=0 \
    -c:a aac -b:a 32k -ac 1 -ar 16000 \
    "$OUT_DIR/clip.mkv"

# 8. silent.mp4: 2.0s video only, no audio track, 160x90, 10fps, H.264
$FFMPEG -y -hide_banner -loglevel error \
    -f lavfi -i "testsrc2=size=160x90:rate=10:duration=2.0" \
    $DET_FLAGS \
    -map_metadata -1 \
    -c:v libx264 -pix_fmt yuv420p -crf 35 -preset veryslow -threads 1 \
    -x264-params threads=1:sliced_threads=0 \
    -an \
    -movflags +faststart \
    "$OUT_DIR/silent.mp4"

echo "done"

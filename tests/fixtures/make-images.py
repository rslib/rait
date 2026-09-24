"""Generate synthetic image fixtures for testing.

Run via tests/fixtures/make-images.sh, which invokes this script with
`uv run --no-project --with pillow --with piexif`. All content is
synthetic (solid colors, gradients, hand-written text) with no external
source. Output is deterministic: re-running produces byte-identical
files, since no timestamps other than the fixed EXIF DateTimeOriginal
below are embedded.
"""

import os

import piexif
from PIL import Image, ImageDraw, ImageFont

HERE = os.path.dirname(os.path.abspath(__file__))


def out(name):
    return os.path.join(HERE, name)


def make_photo_jpg():
    # 60x40 as stored; left half red, right half blue.
    w, h = 60, 40
    img = Image.new("RGB", (w, h))
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = (255, 0, 0) if x < w // 2 else (0, 0, 255)

    exif_dict = {
        "0th": {
            piexif.ImageIFD.Make: b"RAIT",
            piexif.ImageIFD.Model: b"Fixture Cam",
            piexif.ImageIFD.Orientation: 6,
        },
        "Exif": {
            piexif.ExifIFD.DateTimeOriginal: b"2026:01:02 03:04:05",
        },
        "1st": {},
        "thumbnail": None,
    }
    exif_bytes = piexif.dump(exif_dict)
    img.save(out("photo.jpg"), format="JPEG", quality=90, exif=exif_bytes)


def make_text_png():
    w, h = 700, 220
    img = Image.new("RGB", (w, h), (255, 255, 255))
    draw = ImageDraw.Draw(img)

    font = None
    try:
        font = ImageFont.load_default(size=48)
    except TypeError:
        for candidate in (
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
        ):
            if os.path.exists(candidate):
                font = ImageFont.truetype(candidate, 48)
                break
        if font is None:
            font = ImageFont.load_default()

    draw.text((20, 30), "RAIT OCR TEST", fill=(0, 0, 0), font=font)
    draw.text((20, 120), "Invoice 4217", fill=(0, 0, 0), font=font)

    img.save(out("text.png"), format="PNG")


def make_alpha_png():
    w, h = 32, 16
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    px = img.load()
    for y in range(h):
        for x in range(w):
            if x >= w // 2:
                px[x, y] = (0, 255, 0, 255)
    img.save(out("alpha.png"), format="PNG")


def make_tiny_gif():
    w, h = 8, 8
    img = Image.new("P", (w, h))
    palette = [255, 0, 0, 0, 0, 255] + [0, 0, 0] * 254
    img.putpalette(palette)
    px = img.load()
    for y in range(h):
        for x in range(w):
            px[x, y] = 0 if x < w // 2 else 1
    img.save(out("tiny.gif"), format="GIF")


def make_tiny_webp():
    w, h = 10, 6
    img = Image.new("RGB", (w, h), (0, 128, 255))
    img.save(out("tiny.webp"), format="WEBP", lossless=True)


def main():
    make_photo_jpg()
    make_text_png()
    make_alpha_png()
    make_tiny_gif()
    make_tiny_webp()


if __name__ == "__main__":
    main()

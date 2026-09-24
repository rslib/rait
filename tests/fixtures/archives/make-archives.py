#!/usr/bin/env python3
"""Generate deterministic archive test fixtures.

Writes every file directly into the directory this script lives in
(tests/fixtures/archives/). Re-running this script must produce
byte-for-byte identical output; see make-archives.sh for the
double-run cmp check.
"""

import bz2
import gzip
import io
import lzma
import os
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))

EPOCH = 1767225600  # 2026-01-01T00:00:00Z
DATE_TIME = (2026, 1, 1, 0, 0, 0)

README_TXT = b"RAIT archive fixture\nsecond line\n"
NOTES_MD = b"# Notes\n\n- one\n- two\n"
NUMBERS_CSV = b"n,square\n1,1\n2,4\n3,9\n"

# The sample tree, in fixed order. None content means "directory entry".
ENTRIES = [
    ("docs/", None),
    ("docs/readme.txt", README_TXT),
    ("docs/notes.md", NOTES_MD),
    ("data/", None),
    ("data/numbers.csv", NUMBERS_CSV),
    ("empty/", None),
]


def out(name):
    return os.path.join(HERE, name)


# ---------------------------------------------------------------- zip -----

def make_zip(path):
    with zipfile.ZipFile(path, "w") as zf:
        for name, content in ENTRIES:
            zi = zipfile.ZipInfo(name, date_time=DATE_TIME)
            zi.create_system = 3  # unix
            if content is None:
                zi.external_attr = (0o40755 << 16) | 0x10
                zi.compress_type = zipfile.ZIP_STORED
                zf.writestr(zi, b"")
            else:
                zi.external_attr = 0o100644 << 16
                zi.compress_type = zipfile.ZIP_DEFLATED
                zf.writestr(zi, content)


# ---------------------------------------------------------------- tar -----

def build_tar_bytes(entries):
    buf = io.BytesIO()
    tf = tarfile.TarFile(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT)
    for name, content in entries:
        ti = tarfile.TarInfo(name=name)
        ti.mtime = EPOCH
        ti.uid = 0
        ti.gid = 0
        ti.uname = ""
        ti.gname = ""
        if content is None:
            ti.type = tarfile.DIRTYPE
            ti.mode = 0o755
            tf.addfile(ti)
        else:
            ti.type = tarfile.REGTYPE
            ti.mode = 0o644
            ti.size = len(content)
            tf.addfile(ti, io.BytesIO(content))
    tf.close()
    return buf.getvalue()


def gzip_bytes(data, mtime=0):
    buf = io.BytesIO()
    with gzip.GzipFile(fileobj=buf, mode="wb", mtime=mtime, filename="") as gz:
        gz.write(data)
    return buf.getvalue()


def xz_bytes(data):
    return lzma.compress(data, format=lzma.FORMAT_XZ)


def bz2_bytes(data):
    return bz2.compress(data)


def zstd_bytes(data, level=19):
    proc = subprocess.run(
        ["zstd", "-q", str("-" + str(level)), "--no-progress", "-c"],
        input=data,
        stdout=subprocess.PIPE,
        check=True,
    )
    return proc.stdout


# ----------------------------------------------------------------- 7z -----

def make_7z(path):
    # bsdtar's --format 7zip writer embeds each staged file/directory's
    # inode number in the compressed output (confirmed by recreating an
    # otherwise byte-identical staging tree: only the inode numbers changed,
    # and the resulting .7z differed), so it cannot produce reproducible
    # archives across runs. py7zr (pure Python) does not have that problem,
    # so we shell out to it via uv instead.
    gen_script = os.path.join(HERE, "_gen_7z.py")
    with tempfile.TemporaryDirectory() as stage:
        for name, content in ENTRIES:
            full = os.path.join(stage, name)
            if content is None:
                os.makedirs(full, exist_ok=True)
                os.chmod(full, 0o755)
            else:
                os.makedirs(os.path.dirname(full), exist_ok=True)
                with open(full, "wb") as f:
                    f.write(content)
                os.chmod(full, 0o644)
            os.utime(full, (EPOCH, EPOCH))
        # Creating a file inside a directory bumps that directory's own
        # mtime, so re-stamp directories last, after all their children
        # exist, to make the mtime actually stick.
        for name, content in ENTRIES:
            if content is None:
                os.utime(os.path.join(stage, name), (EPOCH, EPOCH))

        if os.path.exists(path):
            os.remove(path)

        with open(gen_script, "w") as f:
            f.write(
                "import py7zr, sys\n"
                "stage, dest = sys.argv[1], sys.argv[2]\n"
                "names = " + repr([n.rstrip("/") for n, _ in ENTRIES]) + "\n"
                "import os\n"
                "with py7zr.SevenZipFile(dest, 'w') as z:\n"
                "    for name in names:\n"
                "        z.write(os.path.join(stage, name), name)\n"
            )
        try:
            subprocess.run(
                [
                    "uv",
                    "run",
                    "--no-project",
                    "--with",
                    "py7zr",
                    "python3",
                    gen_script,
                    stage,
                    path,
                ],
                check=True,
            )
        finally:
            os.remove(gen_script)


# ------------------------------------------------------------------ ar -----

def ar_field(value, width):
    b = str(value).encode("ascii")
    if len(b) > width:
        raise ValueError(f"field {value!r} too long for width {width}")
    return b + b" " * (width - len(b))


def build_ar(files):
    out_bytes = bytearray(b"!<arch>\n")
    for name, content in files:
        header = (
            ar_field(name + "/", 16)
            + ar_field(EPOCH, 12)
            + ar_field(0, 6)
            + ar_field(0, 6)
            + ar_field("100644", 8)
            + ar_field(len(content), 10)
            + b"\x60\x0a"
        )
        assert len(header) == 60, len(header)
        out_bytes += header
        out_bytes += content
        if len(content) % 2 == 1:
            out_bytes += b"\n"
    return bytes(out_bytes)


# --------------------------------------------------------------- evil -----

def make_evil_zip(path):
    entries = [
        ("ok.txt", b"safe\n", False),
        ("../escape.txt", b"escaped\n", False),
        ("/abs.txt", b"absolute\n", False),
        ("nested/../../escape2.txt", b"escaped2\n", False),
        ("link", b"/etc/passwd", True),
    ]
    with zipfile.ZipFile(path, "w") as zf:
        for name, content, is_symlink in entries:
            zi = zipfile.ZipInfo(name, date_time=DATE_TIME)
            zi.create_system = 3
            zi.compress_type = zipfile.ZIP_STORED
            if is_symlink:
                zi.external_attr = (0o120777 << 16) | 0x10
            else:
                zi.external_attr = 0o100644 << 16
            zf.writestr(zi, content)


def make_evil_tar(path):
    buf = io.BytesIO()
    tf = tarfile.TarFile(fileobj=buf, mode="w", format=tarfile.USTAR_FORMAT)

    def add(name, ttype, content=b"", linkname="", mode=0o644, devmajor=0, devminor=0):
        ti = tarfile.TarInfo(name=name)
        ti.mtime = EPOCH
        ti.uid = 0
        ti.gid = 0
        ti.uname = ""
        ti.gname = ""
        ti.type = ttype
        ti.mode = mode
        ti.linkname = linkname
        ti.devmajor = devmajor
        ti.devminor = devminor
        if ttype == tarfile.REGTYPE:
            ti.size = len(content)
            tf.addfile(ti, io.BytesIO(content))
        else:
            ti.size = 0
            tf.addfile(ti)

    add("ok.txt", tarfile.REGTYPE, content=b"safe\n")
    add("../escape.txt", tarfile.REGTYPE, content=b"escaped\n")
    add("/abs.txt", tarfile.REGTYPE, content=b"absolute\n")
    add("link", tarfile.SYMTYPE, linkname="/etc/passwd", mode=0o777)
    add("hard", tarfile.LNKTYPE, linkname="../outside.txt", mode=0o644)
    add("dev", tarfile.CHRTYPE, mode=0o644, devmajor=1, devminor=3)

    tf.close()
    with open(path, "wb") as f:
        f.write(buf.getvalue())


# ----------------------------------------------------------------- main ----

def main():
    make_zip(out("sample.zip"))

    tar_bytes = build_tar_bytes(ENTRIES)
    with open(out("sample.tar"), "wb") as f:
        f.write(tar_bytes)

    gz_bytes = gzip_bytes(tar_bytes, mtime=0)
    with open(out("sample.tar.gz"), "wb") as f:
        f.write(gz_bytes)
    with open(out("sample.tgz"), "wb") as f:
        f.write(gz_bytes)

    with open(out("sample.tar.xz"), "wb") as f:
        f.write(xz_bytes(tar_bytes))

    with open(out("sample.tar.bz2"), "wb") as f:
        f.write(bz2_bytes(tar_bytes))

    with open(out("sample.tar.zst"), "wb") as f:
        f.write(zstd_bytes(tar_bytes))

    make_7z(out("sample.7z"))

    with open(out("sample.a"), "wb") as f:
        f.write(build_ar([("readme.txt", README_TXT), ("numbers.csv", NUMBERS_CSV)]))

    with open(out("readme.txt.gz"), "wb") as f:
        f.write(gzip_bytes(README_TXT, mtime=0))
    with open(out("readme.txt.zst"), "wb") as f:
        f.write(zstd_bytes(README_TXT))
    with open(out("readme.txt.xz"), "wb") as f:
        f.write(xz_bytes(README_TXT))
    with open(out("readme.txt.bz2"), "wb") as f:
        f.write(bz2_bytes(README_TXT))

    make_evil_zip(out("evil.zip"))
    make_evil_tar(out("evil.tar"))


if __name__ == "__main__":
    main()

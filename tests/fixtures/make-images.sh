#!/bin/sh
# Generate image test fixtures. Run from the repository root:
#   ./tests/fixtures/make-images.sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

uv run --no-project --with pillow --with piexif python3 "$SCRIPT_DIR/make-images.py"

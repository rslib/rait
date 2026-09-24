#!/bin/sh
# Regenerate the generated document fixtures under tests/fixtures/ from
# sample.md (via pandoc) and edge.docx (via make-edge-docx.py).
#
# Run from the repository root:
#   ./tests/fixtures/make-documents.sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
FIXTURES_DIR="$SCRIPT_DIR"

export SOURCE_DATE_EPOCH=0
export TZ=UTC

cd "$FIXTURES_DIR"

pandoc sample.md -s -o sample.html
pandoc sample.md -o sample.docx
pandoc sample.md -o sample.epub
pandoc sample.md -o sample.pptx

python3 make-edge-docx.py

echo "Generated fixtures in $FIXTURES_DIR"

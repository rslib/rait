#!/bin/sh
# Regenerate tests/fixtures/tables.pdf from tables.tex with pdflatex.
#
# Run from the repository root:
#   ./tests/fixtures/make-pdf-tables.sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
FIXTURES_DIR="$SCRIPT_DIR"
PDFLATEX="${PDFLATEX:-pdflatex}"

export SOURCE_DATE_EPOCH=0
export FORCE_SOURCE_DATE=1

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT INT TERM

cp "$FIXTURES_DIR/tables.tex" "$WORK_DIR/tables.tex"

cd "$WORK_DIR"
"$PDFLATEX" -interaction=nonstopmode -halt-on-error tables.tex >pdflatex.log 2>&1
"$PDFLATEX" -interaction=nonstopmode -halt-on-error tables.tex >>pdflatex.log 2>&1

cp "$WORK_DIR/tables.pdf" "$FIXTURES_DIR/tables.pdf"

echo "Generated $FIXTURES_DIR/tables.pdf"

#!/bin/sh
# Regenerate tests/fixtures/figures.pdf from figures.tex with pdflatex.
#
# Run from the repository root:
#   ./tests/fixtures/make-pdf-figures.sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
FIXTURES_DIR="$SCRIPT_DIR"
PDFLATEX="${PDFLATEX:-pdflatex}"

export SOURCE_DATE_EPOCH=0
export FORCE_SOURCE_DATE=1

WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT INT TERM

cp "$FIXTURES_DIR/figures.tex" "$WORK_DIR/figures.tex"

cd "$WORK_DIR"
"$PDFLATEX" -interaction=nonstopmode -halt-on-error figures.tex >pdflatex.log 2>&1
"$PDFLATEX" -interaction=nonstopmode -halt-on-error figures.tex >>pdflatex.log 2>&1

cp "$WORK_DIR/figures.pdf" "$FIXTURES_DIR/figures.pdf"

echo "Generated $FIXTURES_DIR/figures.pdf"

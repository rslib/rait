#!/bin/sh
# Generate deterministic archive test fixtures.
# Run from the repository root: ./tests/fixtures/archives/make-archives.sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)

python3 "$script_dir/make-archives.py"

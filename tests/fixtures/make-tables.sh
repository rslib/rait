#!/bin/sh
set -eu
uv run --no-project --with openpyxl --with pyarrow python3 "$(dirname "$0")/make-tables.py"

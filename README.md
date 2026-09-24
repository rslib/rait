# RAIT

RAIT is a command-line tool that gives coding agents a stable, token-efficient interface to local PDFs: metadata, text, search, rendering, splitting, merging and OCR.

RAIT follows the [AXI (Agent eXperience Interface)](https://axi.md/) principles for agent-ergonomic CLIs:

- TOON output with small default fields. More on request with flags such as `--fields`.
- Long text is truncated, with the total size and the exact command to get the rest.
- Errors go to stdout as structured records with a code and a fix. Unknown flags are rejected, and the error lists the valid ones.
- Empty results say so. List output ends with `help` next steps.
- `rait` with no arguments shows live state, not a manual.

## Quickstart

Install from a clone. This needs Rust 1.96 or newer.

```sh
cargo install --locked --path crates/rait
rait pdf info tests/fixtures/simple.pdf
```

Expected output:

```text
count: 1
pdfs[1]{file,pages,title,encrypted}:
  tests/fixtures/simple.pdf,1,null,false
help[2]: ...
```

Output is [TOON](https://toonformat.dev/) by default. Add `--format json` when a script parses it.

## External programs

RAIT wraps proven local tools and does not bundle them. Run `rait doctor` to see what is installed and the install command for anything missing.

| Program | Used by | Without it |
| --- | --- | --- |
| Poppler (`pdfinfo`, `pdftotext`, `pdftoppm`) | `info`, `text`, `search`, `render` | `info`, `text` and `search` use a built-in reader with a warning; `render` fails |
| `qpdf` | `split`, `merge` | fails with `dependency_missing` |
| `ocrmypdf` | `ocr` | fails with `dependency_missing` |

## Use it from an agent

Install the embedded skill for your agent:

```sh
rait skills install --target claude             # ~/.claude/skills/rait-pdf
rait skills install --target pi --scope project # .agents/skills/rait-pdf
```

Run `rait` with no arguments to see the live home view: dependency status, PDFs in the current directory and the next commands to try.

## Reference

`--help` is the reference. Every command lists its flags and examples, for example `rait pdf text --help`.

## Development

Enable the pre-commit hook once per clone. It runs `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test`, the same checks as CI.

```sh
git config core.hooksPath .githooks
```

`skills/pdf/SKILL.md` is generated from the guide in `crates/rait/src/guide.rs`. After changing the guide, regenerate it:

```sh
RAIT_BLESS=1 cargo test -p rait
```

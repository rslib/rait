# Vendored sevenz-rust2 0.23.0

Copied from crates.io (https://github.com/hasenbanck/sevenz-rust, Apache-2.0, see LICENSE) with one change, marked `RAIT PATCH` in `src/reader.rs`:

`BlockDecoder::for_each_entries` iterated `num_unpack_sub_streams` files from the block's first file, but that count covers only files with a stream. A directory between files in a solid block used up a slot, so the block's last files were never decoded and no error was raised. The loop now counts only files with a stream.

Remove this directory and the `[patch.crates-io]` entry in the workspace `Cargo.toml` once an upstream release includes the fix.

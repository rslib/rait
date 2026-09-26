---
name: rait
description: Read, search and transform local PDF, DOCX, PPTX, EPUB and HTML files and preview CSV, Excel, ODS and Parquet tables, inspect, shrink and OCR images, list, read and safely extract archives, and inspect audio and video with RAIT. Use when reading, summarizing, searching or transforming PDFs, office documents, data files, images, archives or media from an agent.
---

# RAIT

Read and transform local PDFs, documents, tables, images, archives and media for coding agents. Output is TOON by default.

- Run `rait pdf info <file>...` for page counts and titles, and `rait pdf outline <file>` for the table of contents with pages; without bookmarks the headings are guessed, so check them.
- Run `rait pdf text <file> --pages <range>` to read text, or `--section <title>` for one section from the outline; detected tables appear as Markdown tables, narrow --pages or lower --max-chars to save context.
- Run `rait search <file>... "<query>"` to find which PDFs, documents, notebooks or tables mention something (-i ignores case); hits give the page, part or row to read next.
- Run `rait pdf tables <file>` or `rait doc tables <file>` for tables as rows keyed by their header; PDF tables are detected, so check odd results with `rait pdf text <file> --layout`.
- Run `rait models fetch` once to install the table models; PDF table detection uses them automatically and falls back to a heuristic without them.
- Run `rait pdf search <file>... "<query>"` to find lines (-i ignores case), then read hit pages with `rait pdf text <file> --pages <page>`.
- Pass many PDFs to one command instead of a shell loop; --jobs <n> bounds the parallel work.
- Run `rait pdf figures <file> --output <dir>` to save each captioned figure as a PNG you can look at, with its caption.
- Run `rait pdf render|split|merge <file>... --output <path>` to write files; add --force to replace existing ones.
- Run `rait pdf ocr <file> --output <dir>` only when `rait pdf text` finds no usable text.
- Run `rait doc text <file>... [--parts <range>]` to read DOCX, PPTX, EPUB, HTML or Jupyter notebooks as Markdown with tables; parts are sections, slides or chapters; notebook outputs are shortened and images left out.
- Run `rait table info|schema|rows <file>` to preview CSV, TSV, Excel, ODS, Parquet or SQLite; rows come as a table, 20 by default, more with --rows <range> and --columns <names>; pick a sheet or SQLite table with --sheet.
- Run `rait image info <file>...` for size and format, `rait image convert <file>... --output <dir> --max <px>` to shrink or convert (EXIF rotation applied), and `rait image ocr <file>` to read text in an image.
- Run `rait archive list <file>` and `rait archive read <file> <entry>` to look inside zip, 7z, ar and tar archives without extracting; `rait archive extract <file> --output <dir>` skips unsafe paths and links.
- Run `rait media info <file>...` for duration and codecs, `rait media frames <video> --output <dir>` for stills to look at, and `rait media audio <file> --output <dir> --mono --rate 16000` for speech-ready audio.
- Run `rait doctor` after a dependency_missing error.
- Add --format json when a script parses the output.
- Run `rait <command> --help` for flags and examples.

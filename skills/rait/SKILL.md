---
name: rait
description: Read, search and transform local PDF, DOCX, PPTX, EPUB and HTML files with RAIT. Use when reading, summarizing, searching or transforming PDFs or office documents from an agent.
---

# RAIT

Read and transform local PDFs and documents for coding agents. Output is TOON by default.

- Run `rait pdf info <file>...` for page counts and titles.
- Run `rait pdf text <file> --pages <range>` to read text; narrow --pages or lower --max-chars to save context.
- Run `rait pdf search <file>... "<query>"` to find lines (-i ignores case), then read hit pages with `rait pdf text <file> --pages <page>`.
- Pass many PDFs to one command instead of a shell loop; --jobs <n> bounds the parallel work.
- Run `rait pdf render|split|merge <file>... --output <path>` to write files; add --force to replace existing ones.
- Run `rait pdf ocr <file> --output <dir>` only when `rait pdf text` finds no usable text.
- Run `rait doc text <file>... [--parts <range>]` to read DOCX, PPTX, EPUB or HTML as Markdown with tables; parts are sections, slides or chapters.
- Run `rait doctor` after a dependency_missing error.
- Add --format json when a script parses the output.
- Run `rait <command> --help` for flags and examples.

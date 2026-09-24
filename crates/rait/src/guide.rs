pub const DESCRIPTION: &str = "Read and transform local PDFs, documents, tables, images, archives and media for coding agents";

pub const SKILL_DESCRIPTION: &str = "Read, search and transform local PDF, DOCX, PPTX, EPUB and HTML files and preview CSV, Excel, ODS and Parquet tables, inspect, shrink and OCR images, list, read and safely extract archives, and inspect audio and video with RAIT. Use when reading, summarizing, searching or transforming PDFs, office documents, data files, images, archives or media from an agent.";

/// Usage guidance shared by the home view and the generated PDF skill.
pub const GUIDE: &[&str] = &[
    "Run `rait pdf info <file>...` for page counts and titles",
    "Run `rait pdf text <file> --pages <range>` to read text; detected tables appear as Markdown tables, narrow --pages or lower --max-chars to save context",
    "Run `rait pdf tables <file>` or `rait doc tables <file>` for tables as rows keyed by their header; PDF tables are detected, so check odd results with `rait pdf text <file> --layout`",
    "Run `rait models fetch` once to install the table model; PDF table detection uses it automatically and falls back to a heuristic without it",
    "Run `rait pdf search <file>... \"<query>\"` to find lines (-i ignores case), then read hit pages with `rait pdf text <file> --pages <page>`",
    "Pass many PDFs to one command instead of a shell loop; --jobs <n> bounds the parallel work",
    "Run `rait pdf render|split|merge <file>... --output <path>` to write files; add --force to replace existing ones",
    "Run `rait pdf ocr <file> --output <dir>` only when `rait pdf text` finds no usable text",
    "Run `rait doc text <file>... [--parts <range>]` to read DOCX, PPTX, EPUB or HTML as Markdown with tables; parts are sections, slides or chapters",
    "Run `rait table info|schema|rows <file>` to preview CSV, TSV, Excel, ODS or Parquet; rows come as a table, 20 by default, more with --rows <range> and --columns <names>",
    "Run `rait image info <file>...` for size and format, `rait image convert <file>... --output <dir> --max <px>` to shrink or convert (EXIF rotation applied), and `rait image ocr <file>` to read text in an image",
    "Run `rait archive list <file>` and `rait archive read <file> <entry>` to look inside zip, 7z, ar and tar archives without extracting; `rait archive extract <file> --output <dir>` skips unsafe paths and links",
    "Run `rait media info <file>...` for duration and codecs, `rait media frames <video> --output <dir>` for stills to look at, and `rait media audio <file> --output <dir> --mono --rate 16000` for speech-ready audio",
    "Run `rait doctor` after a dependency_missing error",
    "Add --format json when a script parses the output",
    "Run `rait <command> --help` for flags and examples",
];

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL_NAME: &str = "rait";

    /// The PDF skill: the home view guidance without live state.
    fn skill_markdown() -> String {
        let mut md = format!(
            "---\nname: {SKILL_NAME}\ndescription: {SKILL_DESCRIPTION}\n---\n\n# RAIT\n\n{DESCRIPTION}. Output is TOON by default.\n\n"
        );
        for line in GUIDE {
            md.push_str(&format!("- {line}.\n"));
        }
        md
    }

    /// Fails when `skills/rait/SKILL.md` drifts from the guide.
    /// `RAIT_BLESS=1 cargo test -p rait` rewrites it.
    #[test]
    fn committed_skill_matches_guide() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/rait/SKILL.md");
        let expected = skill_markdown();
        if std::env::var_os("RAIT_BLESS").is_some() {
            std::fs::write(path, &expected).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(path).unwrap();
        assert!(
            committed == expected,
            "skills/rait/SKILL.md is stale. Run `RAIT_BLESS=1 cargo test -p rait`."
        );
    }
}

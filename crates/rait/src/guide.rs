pub const DESCRIPTION: &str = "Inspect and transform local PDFs for coding agents";

pub const SKILL_DESCRIPTION: &str = "Inspect, read, search, render, split, merge and OCR local PDF files with RAIT. Use when reading, summarizing, searching or transforming PDFs from an agent.";

/// Usage guidance shared by the home view and the generated PDF skill.
pub const GUIDE: &[&str] = &[
    "Run `rait pdf info <file>...` for page counts and titles",
    "Run `rait pdf text <file> --pages <range>` to read text; narrow --pages or lower --max-chars to save context",
    "Run `rait pdf search <file>... \"<query>\"` to find lines (-i ignores case), then read hit pages with `rait pdf text <file> --pages <page>`",
    "Pass many PDFs to one command instead of a shell loop; --jobs <n> bounds the parallel work",
    "Run `rait pdf render|split|merge <file>... --output <path>` to write files; add --force to replace existing ones",
    "Run `rait pdf ocr <file> --output <dir>` only when `rait pdf text` finds no usable text",
    "Run `rait doctor` after a dependency_missing error",
    "Add --format json when a script parses the output",
    "Run `rait <command> --help` for flags and examples",
];

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL_NAME: &str = "rait-pdf";

    /// The PDF skill: the home view guidance without live state.
    fn skill_markdown() -> String {
        let mut md = format!(
            "---\nname: {SKILL_NAME}\ndescription: {SKILL_DESCRIPTION}\n---\n\n# RAIT PDF\n\n{DESCRIPTION}. Output is TOON by default.\n\n"
        );
        for line in GUIDE {
            md.push_str(&format!("- {line}.\n"));
        }
        md
    }

    /// Fails when `skills/pdf/SKILL.md` drifts from the guide.
    /// `RAIT_BLESS=1 cargo test -p rait` rewrites it.
    #[test]
    fn committed_skill_matches_guide() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../skills/pdf/SKILL.md");
        let expected = skill_markdown();
        if std::env::var_os("RAIT_BLESS").is_some() {
            std::fs::write(path, &expected).unwrap();
            return;
        }
        let committed = std::fs::read_to_string(path).unwrap();
        assert!(
            committed == expected,
            "skills/pdf/SKILL.md is stale. Run `RAIT_BLESS=1 cargo test -p rait`."
        );
    }
}

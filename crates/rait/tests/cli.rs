use std::path::Path;
use std::process::{Command, Output};

const FIXTURE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/simple.pdf"
);
const TWO_PAGES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/two-pages.pdf"
);

fn fixture(name: &str) -> String {
    format!("{}/../../tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// A throwaway directory used as both the working directory and `$HOME`.
struct Sandbox(tempfile::TempDir);

impl Sandbox {
    fn new() -> Self {
        Self(tempfile::tempdir().unwrap())
    }

    fn path(&self) -> &Path {
        self.0.path()
    }

    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_rait"));
        cmd.current_dir(self.path()).env("HOME", self.path());
        cmd
    }

    fn run(&self, args: &[&str]) -> Output {
        self.cmd().args(args).output().unwrap()
    }
}

fn run(args: &[&str]) -> Output {
    Sandbox::new().run(args)
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn json(out: &Output) -> serde_json::Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn has(program: &str) -> bool {
    let found = Command::new(program).arg("--version").output().is_ok();
    if !found {
        eprintln!("skipped: `{program}` is not installed");
    }
    found
}

#[test]
fn help_succeeds() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let stdout = stdout(&out);
    assert!(stdout.contains("doctor"));
    assert!(stdout.contains("pdf"));
    assert!(stdout.contains("skills"));
}

#[test]
fn version_flags_print_bare_version() {
    for flag in ["-v", "-V", "--version"] {
        let out = run(&[flag]);
        assert!(out.status.success());
        assert_eq!(stdout(&out).trim(), env!("CARGO_PKG_VERSION"));
    }
}

#[test]
fn home_view_shows_bin_and_hints() {
    let out = run(&[]);
    assert!(out.status.success());
    let stdout = stdout(&out);
    assert!(stdout.starts_with("bin: "));
    assert!(stdout.contains("description: "));
    assert!(stdout.contains("help["));
}

#[test]
fn one_input_produces_one_record_in_toon_and_json() {
    let out = run(&["pdf", "info", FIXTURE]);
    assert!(out.status.success());
    let toon = stdout(&out);
    assert!(toon.starts_with("count: 1\npdfs[1]{file,pages,title,encrypted}:"));
    assert!(toon.contains(&format!("\"{FIXTURE}\",1,null,false")));
    let out = run(&["--format", "json", "pdf", "info", FIXTURE]);
    let records = json(&out);
    assert_eq!(records.as_array().unwrap().len(), 1);
    assert_eq!(records[0]["schema"], 1);
    assert_eq!(records[0]["data"]["pages"], 1);
}

#[test]
fn missing_input_is_file_not_found() {
    let out = run(&["--format", "json", "pdf", "info", "missing.pdf"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out)[0]["error"]["code"], "file_not_found");
}

#[test]
fn mixed_results_keep_order_for_any_worker_count() {
    for jobs in ["0", "1", "3"] {
        let out = run(&[
            "--format",
            "json",
            "--jobs",
            jobs,
            "pdf",
            "info",
            "missing-a.pdf",
            FIXTURE,
            "missing-b.pdf",
        ]);
        assert_eq!(out.status.code(), Some(1));
        let records = json(&out);
        let inputs: Vec<&str> = records
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["input"].as_str().unwrap())
            .collect();
        assert_eq!(inputs, ["missing-a.pdf", FIXTURE, "missing-b.pdf"]);
        assert_eq!(records[1]["ok"], true);
    }
}

#[test]
fn extracts_fixture_text() {
    let out = run(&["--format", "json", "pdf", "text", FIXTURE, "--pages", "1"]);
    assert!(out.status.success());
    let data = &json(&out)[0]["data"];
    assert_eq!(data["range"], "1");
    assert_eq!(data["total_pages"], 1);
    assert_eq!(data["pages"][0]["page"], 1);
    let text = data["pages"][0]["text"].as_str().unwrap();
    assert!(text.contains("RAIT simple PDF fixture"));
    assert!(!text.contains('\u{c}'));
}

#[test]
fn text_returns_one_row_per_page() {
    let toon = stdout(&run(&["pdf", "text", TWO_PAGES]));
    assert!(toon.contains("pages[2]{page,text}:"));
    assert!(toon.contains("1,\"First page"));
    assert!(toon.contains("2,\"Second page"));
}

#[test]
fn truncation_keeps_whole_pages_and_names_the_rest() {
    let out = run(&[
        "--format",
        "json",
        "--max-chars",
        "40",
        "pdf",
        "text",
        TWO_PAGES,
    ]);
    let record = &json(&out)[0];
    let pages = record["data"]["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    assert!(
        pages[0]["text"]
            .as_str()
            .unwrap()
            .starts_with("First page of the RAIT fixture")
    );
    let hint = record["truncation"]["hint"].as_str().unwrap();
    assert!(hint.contains("--pages 2`"), "{hint}");
}

#[test]
fn text_over_limit_reports_truncation_and_full_disables_it() {
    let out = run(&[
        "--format",
        "json",
        "--max-chars",
        "4",
        "pdf",
        "text",
        FIXTURE,
    ]);
    let record = &json(&out)[0];
    assert_eq!(record["truncated"], true);
    assert_eq!(record["data"]["pages"][0]["text"], "RAIT");
    assert!(
        record["truncation"]["hint"]
            .as_str()
            .unwrap()
            .contains("--pages 1 --full")
    );
    assert_eq!(record["truncation"]["returned_chars"], 4);
    assert!(record["truncation"]["total_chars"].as_u64().unwrap() > 4);
    let toon = stdout(&run(&["--max-chars", "4", "pdf", "text", FIXTURE]));
    assert!(toon.contains("total_chars:"));
    assert!(toon.contains("--full"));
    let out = run(&[
        "--format",
        "json",
        "--max-chars",
        "4",
        "pdf",
        "text",
        FIXTURE,
        "--full",
    ]);
    let record = &json(&out)[0];
    assert_eq!(record["truncated"], false);
    assert!(record["data"]["pages"][0]["text"].as_str().unwrap().len() > 4);
}

#[test]
fn page_outside_document_is_item_failure() {
    let out = run(&["--format", "json", "pdf", "text", FIXTURE, "--pages", "3"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out)[0]["error"]["code"], "invalid_page_range");
}

#[test]
fn invalid_range_fails_before_subprocess() {
    let out = run(&["pdf", "text", "missing.pdf", "--pages", "1,"]);
    assert_eq!(out.status.code(), Some(2));
    let stdout = stdout(&out);
    assert!(stdout.contains("code: invalid_page_range"));
    assert!(!stdout.contains("file_not_found"));
}

#[test]
fn unknown_flag_is_structured_and_lists_valid_flags() {
    let out = run(&["pdf", "text", FIXTURE, "--stat"]);
    assert_eq!(out.status.code(), Some(2));
    let stdout = stdout(&out);
    assert!(stdout.contains("--stat"));
    assert!(stdout.contains("--pages"));
    assert!(stdout.contains("--max-chars"));
}

fn without_programs(args: &[&str]) -> Output {
    Sandbox::new()
        .cmd()
        .env("PATH", "/nonexistent")
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn missing_poppler_falls_back_to_builtin_reader() {
    let out = without_programs(&["--format", "json", "pdf", "info", FIXTURE]);
    assert!(out.status.success());
    let record = &json(&out)[0];
    assert_eq!(record["data"]["pages"], 1);
    assert!(record["warnings"][0].as_str().unwrap().contains("built-in"));
    let out = without_programs(&["--format", "json", "pdf", "text", FIXTURE, "--layout"]);
    assert!(out.status.success());
    let record = &json(&out)[0];
    let text = record["data"]["pages"][0]["text"].as_str().unwrap();
    assert!(text.contains("RAIT simple PDF fixture"));
    assert_eq!(record["warnings"].as_array().unwrap().len(), 2);
    let out = without_programs(&["--format", "json", "pdf", "text", TWO_PAGES]);
    let pages = json(&out)[0]["data"]["pages"].clone();
    assert_eq!(pages[1]["page"], 2);
    assert!(pages[1]["text"].as_str().unwrap().contains("Second page"));
}

#[test]
fn fallback_warning_is_shown_once_in_toon() {
    let out = without_programs(&["pdf", "info", FIXTURE, FIXTURE]);
    assert!(stdout(&out).contains("warnings[1]:"));
}

#[test]
fn missing_backend_without_fallback_is_dependency_missing() {
    let dir = tempfile::tempdir().unwrap();
    let out = without_programs(&[
        "--format",
        "json",
        "pdf",
        "render",
        FIXTURE,
        "--output",
        dir.path().to_str().unwrap(),
    ]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out)[0]["error"]["code"], "dependency_missing");
}

#[test]
fn doctor_reports_missing_programs_without_failing() {
    let out = without_programs(&["--format", "json", "doctor"]);
    assert!(out.status.success());
    for record in json(&out).as_array().unwrap() {
        assert_eq!(record["data"]["available"], false);
    }
    let toon = stdout(&without_programs(&["doctor"]));
    assert!(toon.contains("programs[5]{program,available,version,used_by}:"));
    assert!(toon.contains("Install pdfinfo (built-in reader used meanwhile)"));
}

#[test]
fn failures_render_as_errors_table_with_stderr_detail() {
    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.pdf");
    std::fs::write(&bad, "not a pdf").unwrap();
    let out = run(&["pdf", "info", FIXTURE, bad.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let toon = stdout(&out);
    assert!(toon.contains("failed: 1"));
    assert!(toon.contains("errors[1]{file,code,message,hint}:"));
    assert!(!toon.contains("Syntax Error"));
    assert!(!String::from_utf8_lossy(&out.stderr).is_empty());
}

#[test]
fn missing_subcommand_lists_subcommands() {
    let out = run(&["pdf"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stdout(&out).contains("<info|text|search|render|split|merge|ocr>"));
}

#[test]
fn search_finds_fixture_line() {
    let out = run(&["--format", "json", "pdf", "search", FIXTURE, "simple", "-i"]);
    assert!(out.status.success());
    let data = &json(&out)[0]["data"];
    assert_eq!(data["matches"], 1);
    assert_eq!(data["hits"][0]["page"], 1);
}

#[test]
fn render_writes_pages_and_refuses_overwrite() {
    if !has("pdftoppm") {
        return;
    }
    let sandbox = Sandbox::new();
    let render = ["pdf", "render", FIXTURE, "--output", "out", "--dpi", "20"];
    let out = sandbox.run(&render);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(sandbox.path().join("out/simple-1.png").exists());
    let out = sandbox.run(&[&["--format", "json"], &render[..]].concat());
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out)[0]["error"]["code"], "output_exists");
    assert!(
        sandbox
            .run(&[&render[..], &["--force"]].concat())
            .status
            .success()
    );
}

#[test]
fn split_and_merge_round_trip() {
    if !has("qpdf") {
        return;
    }
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["pdf", "split", FIXTURE, "--output", "out"]);
    assert!(out.status.success(), "{}", stdout(&out));
    assert!(sandbox.path().join("out/simple-1.pdf").exists());
    let merge = [
        "pdf",
        "merge",
        FIXTURE,
        "out/simple-1.pdf",
        "--output",
        "merged.pdf",
    ];
    let out = sandbox.run(&merge);
    assert!(out.status.success(), "{}", stdout(&out));
    let out = sandbox.run(&["--format", "json", "pdf", "info", "merged.pdf"]);
    assert_eq!(json(&out)[0]["data"]["pages"], 2);
    let out = sandbox.run(&[&["--format", "json"], &merge[..]].concat());
    assert_eq!(json(&out)[0]["error"]["code"], "output_exists");
    assert!(!sandbox.path().join(".merged.pdf.rait-tmp").exists());
}

#[test]
fn duplicate_output_names_fail_before_writing() {
    let sandbox = Sandbox::new();
    let out = sandbox.run(&["pdf", "render", FIXTURE, FIXTURE, "--output", "out"]);
    assert_eq!(out.status.code(), Some(2));
    assert!(!sandbox.path().join("out").exists());
}

fn install(sandbox: &Sandbox, extra: &[&str]) -> Output {
    sandbox
        .cmd()
        .args(["skills", "install", "--target", "pi", "--scope", "project"])
        .args(extra)
        .output()
        .unwrap()
}

#[test]
fn skill_install_is_safe_and_idempotent() {
    let sandbox = Sandbox::new();
    assert!(install(&sandbox, &[]).status.success());
    let path = sandbox.path().join(".agents/skills/rait/SKILL.md");
    let first = std::fs::read_to_string(&path).unwrap();
    assert_eq!(first, include_str!("../../../skills/rait/SKILL.md"));
    let again = install(&sandbox, &[]);
    assert!(again.status.success());
    assert!(stdout(&again).contains("already installed (no-op)"));
    std::fs::write(&path, "changed").unwrap();
    let refused = install(&sandbox, &[]);
    assert_eq!(refused.status.code(), Some(1));
    assert!(stdout(&refused).contains("output_exists"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "changed");
    assert!(install(&sandbox, &["--force"]).status.success());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), first);
}

#[test]
fn claude_installs_write_expected_layouts() {
    let sandbox = Sandbox::new();
    for scope in ["project", "user"] {
        let out = sandbox.run(&["skills", "install", "--target", "claude", "--scope", scope]);
        assert!(out.status.success());
    }
    assert!(sandbox.path().join(".claude/skills/rait/SKILL.md").exists());
}

#[test]
fn home_view_lists_pdfs_in_working_directory() {
    let sandbox = Sandbox::new();
    assert!(stdout(&sandbox.run(&[])).contains("0 PDF or document files"));
    std::fs::copy(FIXTURE, sandbox.path().join("a.pdf")).unwrap();
    std::fs::copy(fixture("sample.docx"), sandbox.path().join("b.docx")).unwrap();
    std::fs::write(sandbox.path().join("c.txt"), "x").unwrap();
    assert!(stdout(&sandbox.run(&[])).contains("files[2]: a.pdf,b.docx"));
}

#[test]
fn doc_text_returns_markdown_parts() {
    let docx = fixture("sample.docx");
    let toon = stdout(&run(&["doc", "text", &docx]));
    assert!(toon.contains("format: docx"));
    assert!(toon.contains("total_parts: 2"));
    assert!(toon.contains("parts[2]{part,text}:"));
    assert!(toon.contains("| EU | 10 | 12 |"));
    let out = run(&["--format", "json", "doc", "text", &docx, "--parts", "2"]);
    let data = &json(&out)[0]["data"];
    assert_eq!(data["parts"].as_array().unwrap().len(), 1);
    assert!(
        data["parts"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("# Results")
    );
}

#[test]
fn doc_text_truncates_by_part() {
    let out = run(&[
        "--format",
        "json",
        "--max-chars",
        "150",
        "doc",
        "text",
        &fixture("sample.pptx"),
    ]);
    let record = &json(&out)[0];
    assert_eq!(record["data"]["parts"].as_array().unwrap().len(), 1);
    let hint = record["truncation"]["hint"].as_str().unwrap();
    assert!(hint.contains("--parts 2-3`"), "{hint}");
}

#[test]
fn doc_text_rejects_unsupported_files() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.path().join("notes.txt"), "x").unwrap();
    let out = sandbox.run(&["--format", "json", "doc", "text", "notes.txt"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(json(&out)[0]["error"]["code"], "invalid_input");
}

#[test]
fn table_rows_print_as_a_toon_table() {
    let toon = stdout(&run(&[
        "table",
        "rows",
        &fixture("sample.csv"),
        "--rows",
        "1-2",
        "--columns",
        "region,zip",
    ]));
    assert!(
        toon.contains("rows[2]{region,zip}:\n      EU,\"01001\"\n      US,\"01002\""),
        "{toon}"
    );
    assert!(toon.contains("--rows 3-25"));
}

#[test]
fn table_info_and_schema() {
    let toon = stdout(&run(&["table", "info", &fixture("sample.xlsx")]));
    assert!(toon.contains("sheets[2]{sheet,rows,columns}:\n      Sales,25,8\n      Notes,2,2"));
    let toon = stdout(&run(&["table", "schema", &fixture("sample.parquet")]));
    assert!(toon.contains("columns[8]{column,type,empty}:"));
    assert!(toon.contains("day,date,0"));
}

#[test]
fn table_rows_truncation_names_the_rest() {
    let out = run(&[
        "--format",
        "json",
        "--max-chars",
        "300",
        "table",
        "rows",
        &fixture("sample.csv"),
    ]);
    let record = &json(&out)[0];
    let shown = record["data"]["rows"].as_array().unwrap().len();
    let hint = record["truncation"]["hint"].as_str().unwrap();
    assert!(
        hint.contains(&format!("--rows {}-20`", shown + 1)),
        "{hint}"
    );
}

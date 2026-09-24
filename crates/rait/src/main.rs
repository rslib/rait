mod guide;
mod render;

use clap::error::{ContextKind, ContextValue, ErrorKind as ClapErrorKind};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use guide::{DESCRIPTION, GUIDE};
use rait_core::{
    ErrorCode, ErrorInfo, Record, Truncation, limit_chunks, path_to_protocol, run_ordered,
};
use rait_pdf::{
    DEPENDENCIES, ImageFormat, MergeRequest, OcrRequest, PageRange, PageText, RenderRequest,
    SearchRequest, SplitRequest, TextRequest,
};
use render::{Format, Output, emit, emit_usage_error, print_value, shell_quote, write_stdout};
use serde::Serialize;
use serde_json::{Map, Value, json};
use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const PDF_SKILL: &str = include_str!("../../../skills/pdf/SKILL.md");
const HOME_PDF_LIMIT: usize = 20;

#[derive(Parser)]
#[command(name = "rait", version, about = DESCRIPTION)]
struct Cli {
    /// Output format. TOON for agents, JSON for scripts, text for people
    #[arg(long, value_enum, default_value_t = Format::Toon, global = true)]
    format: Format,
    /// Parallel jobs across inputs. 0 uses all cores
    #[arg(long, default_value_t = 0, global = true)]
    jobs: usize,
    /// Per-input limit on returned text
    #[arg(long, default_value_t = 50_000, global = true)]
    max_chars: usize,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Check the external programs that PDF commands use
    Doctor,
    /// Inspect, extract and transform PDFs
    Pdf {
        #[command(subcommand)]
        command: PdfCommand,
    },
    /// List, show and install the embedded agent skills
    Skills {
        #[command(subcommand)]
        command: SkillsCommand,
    },
}

#[derive(Subcommand)]
enum PdfCommand {
    /// Page count, title and encryption. More with --fields
    #[command(
        after_help = "Examples:\n  rait pdf info paper.pdf\n  rait pdf info *.pdf --fields author,page-size"
    )]
    Info {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// Extra TOON columns. JSON always has every field
        #[arg(long, value_enum, value_delimiter = ',')]
        fields: Vec<InfoField>,
    },
    /// Extract text. Pages are separated by form feed
    #[command(
        after_help = "Examples:\n  rait pdf text paper.pdf --pages 1-3\n  rait pdf text a.pdf b.pdf --max-chars 20000\n  rait pdf text paper.pdf --full"
    )]
    Text {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// A page like 4 or a closed range like 2-7
        #[arg(long)]
        pages: Option<String>,
        /// Keep the physical layout
        #[arg(long)]
        layout: bool,
        /// Do not apply --max-chars
        #[arg(long)]
        full: bool,
    },
    /// Find lines containing a literal query
    #[command(
        after_help = "Examples:\n  rait pdf search paper.pdf \"cache miss\"\n  rait pdf search *.pdf latency -i --pages 1-5"
    )]
    Search {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        query: String,
        /// A page like 4 or a closed range like 2-7
        #[arg(long)]
        pages: Option<String>,
        #[arg(short, long)]
        ignore_case: bool,
        /// Do not apply --max-chars to hit lines
        #[arg(long)]
        full: bool,
    },
    /// Render pages to images named <stem>-<page>.<ext>
    #[command(
        after_help = "Examples:\n  rait pdf render paper.pdf --output out --pages 1\n  rait pdf render a.pdf b.pdf --output out --dpi 300 --image jpeg"
    )]
    Render {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        /// A page like 4 or a closed range like 2-7
        #[arg(long)]
        pages: Option<String>,
        #[arg(long, default_value_t = 150)]
        dpi: u32,
        #[arg(long, value_enum, default_value_t = ImageArg::Png)]
        image: ImageArg,
        /// Replace existing output files
        #[arg(long)]
        force: bool,
    },
    /// Write each page to its own PDF named <stem>-<page>.pdf
    #[command(
        after_help = "Examples:\n  rait pdf split paper.pdf --output pages\n  rait pdf split paper.pdf --pages 2-4 --output pages"
    )]
    Split {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        /// A page like 4 or a closed range like 2-7
        #[arg(long)]
        pages: Option<String>,
        /// Replace existing output files
        #[arg(long)]
        force: bool,
    },
    /// Join PDFs in argument order into one file
    #[command(after_help = "Examples:\n  rait pdf merge a.pdf b.pdf --output ab.pdf")]
    Merge {
        #[arg(required = true, num_args = 2..)]
        files: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        /// Replace an existing output file
        #[arg(long)]
        force: bool,
    },
    /// Add a text layer to scanned PDFs. Pages that already have text are kept
    #[command(after_help = "Examples:\n  rait pdf ocr scan.pdf --output ocr")]
    Ocr {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        /// Replace existing output files
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum InfoField {
    Author,
    PageSize,
}

#[derive(Clone, Copy, ValueEnum)]
enum ImageArg {
    Png,
    Jpeg,
}

#[derive(Subcommand)]
enum SkillsCommand {
    /// List embedded skills
    List,
    /// Print the embedded skill Markdown
    Show { name: String },
    /// Install the PDF skill for an agent
    #[command(
        after_help = "Examples:\n  rait skills install --target claude\n  rait skills install --target pi --scope project"
    )]
    Install {
        #[arg(long, value_enum)]
        target: SkillTarget,
        #[arg(long, value_enum, default_value_t = SkillScope::User)]
        scope: SkillScope,
        /// Replace a changed skill file
        #[arg(long)]
        force: bool,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum SkillTarget {
    Pi,
    Claude,
}

#[derive(Clone, Copy, ValueEnum)]
enum SkillScope {
    User,
    Project,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if let [_, flag] = args.as_slice()
        && matches!(flag.as_str(), "-v" | "-V" | "--version")
    {
        write_stdout(&format!("{VERSION}\n"));
        return ExitCode::SUCCESS;
    }
    if args.len() == 1 {
        print_value(Format::Toon, &home());
        return ExitCode::SUCCESS;
    }
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            if matches!(
                err.kind(),
                ClapErrorKind::DisplayHelp | ClapErrorKind::DisplayVersion
            ) {
                write_stdout(&err.render().to_string());
                return ExitCode::SUCCESS;
            }
            return emit_usage_error(format_hint(&args), usage_error(&err, &args));
        }
    };
    let format = cli.format;
    match run(cli) {
        Ok(Some(output)) => emit(format, output),
        Ok(None) => ExitCode::SUCCESS,
        Err(err) => emit_usage_error(format, err),
    }
}

/// `Ok(None)` means the command already wrote its output.
fn run(cli: Cli) -> Result<Option<Output>, ErrorInfo> {
    let (jobs, max_chars) = (cli.jobs, cli.max_chars);
    match cli.command {
        Commands::Doctor => Ok(Some(doctor())),
        Commands::Pdf { command } => run_pdf(command, jobs, max_chars).map(Some),
        Commands::Skills { command } => run_skills(command, cli.format),
    }
}

fn run_pdf(command: PdfCommand, jobs: usize, max_chars: usize) -> Result<Output, ErrorInfo> {
    let output = match command {
        PdfCommand::Info { files, fields } => Output::new(
            "pdfs",
            "file",
            each(jobs, "pdf.info", files, |path| {
                let mut info = rait_pdf::info(path)?;
                let warnings = std::mem::take(&mut info.warnings);
                item(info, None, warnings)
            }),
        )
        .shape(move |data| info_columns(data, &fields))
        .help([
            "Run `rait pdf text <file> --pages <range>` to read text",
            "Run `rait pdf search <file>... \"<query>\"` to find lines",
        ]),
        PdfCommand::Text {
            files,
            pages,
            layout,
            full,
        } => {
            let pages = PageRange::parse(pages.as_deref())?;
            Output::new(
                "texts",
                "file",
                each(jobs, "pdf.text", files, |path| {
                    let req = TextRequest {
                        path: path.into(),
                        pages,
                        layout,
                    };
                    let mut text = rait_pdf::text(&req)?;
                    let warnings = std::mem::take(&mut text.warnings);
                    let truncation = if full {
                        None
                    } else {
                        limit_pages(path, &mut text.pages, max_chars)
                    };
                    item(text, truncation, warnings)
                }),
            )
        }
        PdfCommand::Search {
            files,
            query,
            pages,
            ignore_case,
            full,
        } => {
            let pages = PageRange::parse(pages.as_deref())?;
            let records = each(jobs, "pdf.search", files, |path| {
                let req = SearchRequest {
                    path: path.into(),
                    query: query.clone(),
                    pages,
                    ignore_case,
                    max_chars: (!full).then_some(max_chars),
                };
                let mut result = rait_pdf::search(&req)?;
                let warnings = std::mem::take(&mut result.warnings);
                let truncation = (result.hits.len() < result.matches).then(|| Truncation {
                    limit_chars: max_chars,
                    returned_chars: result.hits.iter().map(|hit| hit.line.chars().count()).sum(),
                    total_chars: result.matched_chars,
                    hint: format!(
                        "Run `rait pdf search {} {} --full` for all {} hits",
                        shell_quote(&protocol_or_display(path)),
                        shell_quote(&query),
                        result.matches
                    ),
                });
                item(result, truncation, warnings)
            });
            let found = records.iter().any(|record| {
                record
                    .data
                    .as_ref()
                    .is_some_and(|data| data["matches"].as_u64() > Some(0))
            });
            let help = if found {
                "Run `rait pdf text <file> --pages <page>` to read a hit in context".to_string()
            } else {
                format!(
                    "No line matched {query:?}. Retry with -i, a shorter query or other --pages"
                )
            };
            Output::new("results", "file", records).help([help])
        }
        PdfCommand::Render {
            files,
            output,
            pages,
            dpi,
            image,
            force,
        } => {
            let pages = PageRange::parse(pages.as_deref())?;
            unique_stems(&files)?;
            let format = match image {
                ImageArg::Png => ImageFormat::Png,
                ImageArg::Jpeg => ImageFormat::Jpeg,
            };
            Output::new(
                "rendered",
                "file",
                each(jobs, "pdf.render", files, |path| {
                    to_value(rait_pdf::render(&RenderRequest {
                        path: path.into(),
                        output_dir: output.clone(),
                        pages,
                        dpi,
                        format,
                        force,
                    })?)
                }),
            )
        }
        PdfCommand::Split {
            files,
            output,
            pages,
            force,
        } => {
            let pages = PageRange::parse(pages.as_deref())?;
            unique_stems(&files)?;
            Output::new(
                "split",
                "file",
                each(jobs, "pdf.split", files, |path| {
                    to_value(rait_pdf::split(&SplitRequest {
                        path: path.into(),
                        output_dir: output.clone(),
                        pages,
                        force,
                    })?)
                }),
            )
        }
        PdfCommand::Merge {
            files,
            output,
            force,
        } => {
            let input = protocol_or_display(&output);
            let req = MergeRequest {
                inputs: files,
                output,
                force,
            };
            Output::new(
                "merged",
                "output",
                vec![record(0, "pdf.merge", input, || {
                    to_value(rait_pdf::merge(&req)?)
                })],
            )
            .shape(|mut data| {
                if let Value::Object(fields) = &mut data {
                    fields.remove("output");
                }
                data
            })
        }
        PdfCommand::Ocr {
            files,
            output,
            force,
        } => {
            unique_stems(&files)?;
            Output::new(
                "ocr",
                "file",
                each(jobs, "pdf.ocr", files, |path| {
                    to_value(rait_pdf::ocr(&OcrRequest {
                        path: path.into(),
                        output_dir: output.clone(),
                        force,
                    })?)
                }),
            )
            .help(["Run `rait pdf text <output>` to read the OCR text"])
        }
    };
    Ok(output)
}

/// Keep whole pages within `max_chars` and point at the pages left out.
fn limit_pages(path: &Path, pages: &mut Vec<PageText>, max_chars: usize) -> Option<Truncation> {
    let numbers: Vec<u32> = pages.iter().map(|page| page.page).collect();
    let first_len = pages.first().map_or(0, |page| page.text.chars().count());
    let texts = std::mem::take(pages)
        .into_iter()
        .map(|page| page.text)
        .collect();
    let (kept, truncation) = limit_chunks(texts, max_chars);
    *pages = numbers
        .iter()
        .zip(kept)
        .map(|(&page, text)| PageText { page, text })
        .collect();
    let mut truncation = truncation?;
    let file = shell_quote(&protocol_or_display(path));
    let last = *numbers.last()?;
    let first_cut = pages.len() == 1 && first_len > max_chars;
    truncation.hint = if first_cut {
        format!(
            "Page {} alone passes --max-chars. Run `rait pdf text {file} --pages {} --full`",
            numbers[0], numbers[0]
        )
    } else {
        let next = numbers[pages.len()];
        let rest = if next == last {
            next.to_string()
        } else {
            format!("{next}-{last}")
        };
        format!("Run `rait pdf text {file} --pages {rest}` for the rest, or add --full")
    };
    Some(truncation)
}

/// TOON columns for `pdf info`. Every row gets the same keys so TOON prints a table.
fn info_columns(data: Value, fields: &[InfoField]) -> Value {
    let mut row = Map::new();
    for key in ["pages", "title", "encrypted"] {
        row.insert(key.into(), data[key].clone());
    }
    if fields.contains(&InfoField::Author) {
        row.insert("author".into(), data["author"].clone());
    }
    if fields.contains(&InfoField::PageSize) {
        let size = &data["page_size_points"];
        let size = match (size["width"].as_f64(), size["height"].as_f64()) {
            (Some(width), Some(height)) => json!(format!("{width}x{height} pt")),
            _ => Value::Null,
        };
        row.insert("page_size".into(), size);
    }
    Value::Object(row)
}

fn run_skills(command: SkillsCommand, format: Format) -> Result<Option<Output>, ErrorInfo> {
    match command {
        SkillsCommand::List => Ok(Some(
            Output::new(
                "skills",
                "name",
                vec![Record::success(
                    0,
                    "skills.list",
                    "pdf".into(),
                    json!({ "description": guide::SKILL_DESCRIPTION }),
                )],
            )
            .help([
                "Run `rait skills show <name>` to print a skill",
                "Run `rait skills install --target <claude|pi>` to install it",
            ]),
        )),
        SkillsCommand::Show { name } => {
            if name != "pdf" {
                return Err(ErrorInfo::new(
                    ErrorCode::InvalidInput,
                    format!("Unknown skill `{name}`. Available: pdf."),
                    "Run `rait skills show pdf`.".to_string(),
                ));
            }
            if format == Format::Json {
                return Ok(Some(Output::new(
                    "skills",
                    "name",
                    vec![Record::success(
                        0,
                        "skills.show",
                        "pdf".into(),
                        json!({ "markdown": PDF_SKILL }),
                    )],
                )));
            }
            write_stdout(PDF_SKILL);
            Ok(None)
        }
        SkillsCommand::Install {
            target,
            scope,
            force,
        } => {
            let path = skill_path(target, scope)?;
            let record = record(0, "skills.install", "pdf".into(), || {
                let status = if install_skill(&path, force)? {
                    "installed"
                } else {
                    "already installed (no-op)"
                };
                to_value(json!({ "path": path_to_protocol(&path)?, "status": status }))
            });
            Ok(Some(Output::new("skills", "name", vec![record])))
        }
    }
}

struct Item {
    data: Value,
    truncation: Option<Truncation>,
    warnings: Vec<String>,
}

type ItemResult = Result<Item, ErrorInfo>;

fn each(
    jobs: usize,
    command: &str,
    files: Vec<PathBuf>,
    op: impl Fn(&Path) -> ItemResult + Sync + Send,
) -> Vec<Record> {
    run_ordered(jobs, files, |id, path| {
        record(id, command, protocol_or_display(&path), || op(&path))
    })
}

fn record(id: usize, command: &str, input: String, op: impl FnOnce() -> ItemResult) -> Record {
    match op() {
        Ok(item) => {
            let mut record = Record::success(id, command, input, item.data);
            record.truncated = item.truncation.is_some();
            record.truncation = item.truncation;
            record.warnings = item.warnings;
            record
        }
        Err(err) => Record::failure(id, command, input, err),
    }
}

fn protocol_or_display(path: &Path) -> String {
    path_to_protocol(path).unwrap_or_else(|_| path.display().to_string())
}

fn to_value(value: impl Serialize) -> ItemResult {
    item(value, None, vec![])
}

fn item(
    value: impl Serialize,
    truncation: Option<Truncation>,
    warnings: Vec<String>,
) -> ItemResult {
    serde_json::to_value(value)
        .map(|data| Item {
            data,
            truncation,
            warnings,
        })
        .map_err(|err| {
            ErrorInfo::new(
                ErrorCode::InternalError,
                format!("Serialization failed: {err}"),
                None,
            )
        })
}

fn unique_stems(files: &[PathBuf]) -> Result<(), ErrorInfo> {
    let mut seen = HashSet::new();
    for file in files {
        let stem = rait_pdf::output_stem(file)?;
        if !seen.insert(stem.clone()) {
            return Err(ErrorInfo::new(
                ErrorCode::InvalidInput,
                format!("Two inputs share the file name `{stem}`, so their outputs would collide."),
                "Run them in separate invocations with different --output directories.".to_string(),
            ));
        }
    }
    Ok(())
}

fn doctor() -> Output {
    let mut help = vec![];
    let records = DEPENDENCIES
        .iter()
        .enumerate()
        .map(|(id, dep)| {
            let (available, version) = match dep.probe() {
                Ok(version) => (true, version),
                Err(_) => {
                    let fallback = match dep.fallback {
                        Some(_) => " (built-in reader used meanwhile)",
                        None => "",
                    };
                    help.push(format!(
                        "Install {}{fallback}: {}",
                        dep.program, dep.install
                    ));
                    (false, None)
                }
            };
            Record::success(
                id,
                "doctor",
                dep.program.into(),
                json!({
                    "available": available,
                    "version": version,
                    "used_by": dep.capabilities.join(" "),
                }),
            )
        })
        .collect();
    Output::new("programs", "program", records).help(help)
}

fn home() -> Value {
    let bin = std::env::current_exe()
        .map(|path| tilde(&path))
        .unwrap_or_else(|_| "rait".into());
    let missing: Vec<&str> = DEPENDENCIES
        .iter()
        .filter(|dep| dep.probe().is_err())
        .map(|dep| dep.program)
        .collect();
    let mut pdfs: Vec<String> = fs::read_dir(".")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name.to_lowercase().ends_with(".pdf"))
        .collect();
    pdfs.sort();
    let mut doc = Map::new();
    doc.insert("bin".into(), json!(bin));
    doc.insert("description".into(), json!(DESCRIPTION));
    doc.insert(
        "dependencies".into(),
        json!(if missing.is_empty() {
            "all installed".to_string()
        } else {
            format!("missing {}", missing.join(", "))
        }),
    );
    let mut help = vec![];
    if pdfs.is_empty() {
        doc.insert("pdfs".into(), json!("0 PDF files in the current directory"));
    } else {
        doc.insert(
            "count".into(),
            json!(format!(
                "{} of {} PDF files in the current directory",
                pdfs.len().min(HOME_PDF_LIMIT),
                pdfs.len()
            )),
        );
        if pdfs.len() > HOME_PDF_LIMIT {
            help.push(format!("Run `ls *.pdf` for all {} PDF files", pdfs.len()));
        }
        pdfs.truncate(HOME_PDF_LIMIT);
        doc.insert("pdfs".into(), json!(pdfs));
    }
    if !missing.is_empty() {
        help.push("Run `rait doctor` to see install commands".into());
    }
    help.extend(GUIDE.iter().map(|line| line.to_string()));
    doc.insert("help".into(), json!(help));
    Value::Object(doc)
}

fn tilde(path: &Path) -> String {
    let text = path.display().to_string();
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && text.starts_with(&home) => {
            format!("~{}", &text[home.len()..])
        }
        _ => text,
    }
}

fn format_hint(args: &[String]) -> Format {
    let json = args
        .windows(2)
        .any(|pair| pair[0] == "--format" && pair[1] == "json")
        || args.iter().any(|arg| arg == "--format=json");
    if json { Format::Json } else { Format::Toon }
}

fn usage_error(err: &clap::Error, args: &[String]) -> ErrorInfo {
    let mut cmd = Cli::command();
    cmd.build();
    let mut path = vec!["rait".to_string()];
    for arg in args.iter().skip(1) {
        let Some(sub) = cmd.find_subcommand(arg).cloned() else {
            continue;
        };
        path.push(arg.clone());
        cmd = sub;
    }
    let path = path.join(" ");
    let message = match err.kind() {
        ClapErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            format!("`{path}` needs a subcommand.")
        }
        _ => clap_message(err),
    };
    let hint = match err.kind() {
        ClapErrorKind::UnknownArgument => {
            let flags: Vec<String> = cmd
                .get_arguments()
                .filter_map(|arg| arg.get_long())
                .filter(|long| *long != "help" && *long != "version")
                .map(|long| format!("--{long}"))
                .collect();
            format!("Valid flags for `{path}`: {}", flags.join(", "))
        }
        ClapErrorKind::InvalidSubcommand
        | ClapErrorKind::MissingSubcommand
        | ClapErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
            let subs: Vec<&str> = cmd
                .get_subcommands()
                .map(|sub| sub.get_name())
                .filter(|name| *name != "help")
                .collect();
            format!("Run `{path} <{}>`", subs.join("|"))
        }
        _ => format!("Usage: {}", usage_line(&mut cmd)),
    };
    ErrorInfo::new(ErrorCode::InvalidInput, message, hint)
}

/// Clap's error text without its usage block and `--help` tip.
fn clap_message(err: &clap::Error) -> String {
    let rendered = err.render().to_string();
    let mut parts = vec![];
    for line in rendered.lines().map(str::trim) {
        if line.starts_with("Usage:") || line.starts_with("For more information") {
            break;
        }
        if !line.is_empty() && !line.starts_with("tip:") {
            parts.push(line.trim_start_matches("error: "));
        }
    }
    if let Some(ContextValue::Strings(valid)) = err.get(ContextKind::ValidValue)
        && !parts.iter().any(|part| part.contains("possible values"))
    {
        parts.push("possible values:");
        return format!("{} {}", parts.join(" "), valid.join(", "));
    }
    parts.join(" ")
}

fn usage_line(cmd: &mut clap::Command) -> String {
    cmd.render_usage()
        .to_string()
        .trim_start_matches("Usage:")
        .trim()
        .to_string()
}

fn skill_path(target: SkillTarget, scope: SkillScope) -> Result<PathBuf, ErrorInfo> {
    let base = match (target, scope) {
        (SkillTarget::Pi, SkillScope::User) => user_home()?.join(".agents/skills"),
        (SkillTarget::Pi, SkillScope::Project) => PathBuf::from(".agents/skills"),
        (SkillTarget::Claude, SkillScope::User) => user_home()?.join(".claude/skills"),
        (SkillTarget::Claude, SkillScope::Project) => PathBuf::from(".claude/skills"),
    };
    Ok(base.join("rait-pdf/SKILL.md"))
}

fn user_home() -> Result<PathBuf, ErrorInfo> {
    std::env::var_os("HOME").map(PathBuf::from).ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            "HOME is not set.",
            "Set HOME or use --scope project.".to_string(),
        )
    })
}

/// Returns whether the file changed. An identical existing file is a no-op.
fn install_skill(path: &Path, force: bool) -> Result<bool, ErrorInfo> {
    if let Ok(existing) = fs::read(path) {
        if existing == PDF_SKILL.as_bytes() {
            return Ok(false);
        }
        if !force {
            return Err(ErrorInfo::new(
                ErrorCode::OutputExists,
                format!("{} exists with different content.", path.display()),
                "Rerun with --force to replace it.".to_string(),
            ));
        }
    }
    let parent = path
        .parent()
        .ok_or_else(|| ErrorInfo::new(ErrorCode::InvalidInput, "Invalid skill path.", None))?;
    fs::create_dir_all(parent).map_err(|err| write_error("create skill directory", err))?;
    let tmp = path.with_extension("md.rait-tmp");
    fs::write(&tmp, PDF_SKILL).map_err(|err| write_error("write skill file", err))?;
    fs::rename(&tmp, path).map_err(|err| write_error("install skill file", err))?;
    Ok(true)
}

fn write_error(action: &str, err: io::Error) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::PermissionDenied,
        format!("Failed to {action}: {err}"),
        "Check write permissions, or use the other --scope.".to_string(),
    )
}

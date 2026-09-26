mod cells;
mod columnar;
mod delimited;
mod sheet;
mod sqlite;

use rait_core::{ErrorCode, ErrorInfo, PageRange, check_input};
use serde::Serialize;
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};

/// Rows returned when `--rows` is not given.
pub const DEFAULT_ROWS: u64 = 20;
/// Rows read to infer a text format's column types.
pub const SAMPLE_ROWS: u64 = 10_000;

pub const EXTENSIONS: &[&str] = &[
    "csv", "tsv", "xlsx", "xlsm", "xlsb", "xls", "ods", "parquet", "db", "sqlite", "sqlite3", "db3",
];

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Format {
    Csv,
    Tsv,
    Xlsx,
    Xls,
    Ods,
    Parquet,
    Sqlite,
}

impl Format {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?.to_lowercase();
        match ext.as_str() {
            "csv" => Some(Self::Csv),
            "tsv" | "tab" => Some(Self::Tsv),
            "xlsx" | "xlsm" | "xlsb" => Some(Self::Xlsx),
            "xls" => Some(Self::Xls),
            "ods" => Some(Self::Ods),
            "parquet" => Some(Self::Parquet),
            "db" | "sqlite" | "sqlite3" | "db3" => Some(Self::Sqlite),
            _ => None,
        }
    }

    /// Tables/views are listed like workbook sheets, and picking one when
    /// `--sheet` is not given follows the same rule.
    fn is_workbook(self) -> bool {
        matches!(self, Self::Xlsx | Self::Xls | Self::Ods | Self::Sqlite)
    }

    fn name(self) -> &'static str {
        match self {
            Self::Csv => "CSV",
            Self::Tsv => "TSV",
            Self::Xlsx => "Excel workbook",
            Self::Xls => "Excel 97 workbook",
            Self::Ods => "OpenDocument spreadsheet",
            Self::Parquet => "Parquet file",
            Self::Sqlite => "SQLite database",
        }
    }
}

/// One sheet opened for reading. CSV, TSV and Parquet files have a single sheet
/// named after the file.
pub(crate) struct Table {
    sheet: String,
    header: Vec<String>,
    total_rows: u64,
    /// Declared column types, when the format stores them.
    types: Option<Vec<String>>,
    rows: Box<dyn Iterator<Item = Result<Vec<Value>, String>>>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SheetInfo {
    pub sheet: String,
    /// Data rows, not counting the header.
    pub rows: u64,
    pub columns: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TableInfo {
    pub format: Format,
    pub sheets: Vec<SheetInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Column {
    pub column: String,
    /// `int`, `float`, `bool`, `date`, `datetime`, `string`, `mixed` or `empty`;
    /// Parquet also reports `decimal`, `time`, `binary` and `nested`.
    #[serde(rename = "type")]
    pub kind: String,
    /// Empty cells among the rows read.
    pub empty: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Schema {
    pub format: Format,
    pub sheet: String,
    pub rows: u64,
    /// Rows read to infer types and count empty cells.
    pub sampled: u64,
    pub columns: Vec<Column>,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RowsRequest {
    pub path: PathBuf,
    /// Sheet name or 1-based number; the first sheet by default.
    pub sheet: Option<String>,
    /// Data rows to return; the first [`DEFAULT_ROWS`] by default.
    pub rows: Option<PageRange>,
    /// Columns to keep, by name; all by default.
    pub columns: Vec<String>,
    /// Stop adding rows once their JSON text would pass this many chars.
    pub max_chars: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Rows {
    pub format: Format,
    pub sheet: String,
    pub total_rows: u64,
    /// The data rows returned, such as `1-20`.
    pub range: String,
    pub rows: Vec<Map<String, Value>>,
    /// Last row asked for; more than the rows returned when `max_chars` cut them.
    #[serde(skip)]
    pub last_requested: u64,
    #[serde(skip)]
    pub requested_chars: usize,
    #[serde(skip)]
    pub warnings: Vec<String>,
}

pub fn info(path: &Path) -> Result<TableInfo, ErrorInfo> {
    let format = format_of(path)?;
    let sheets = if format == Format::Sqlite {
        sqlite::info(path).map_err(|detail| unreadable(format, detail))?
    } else if format.is_workbook() {
        sheet::info(path).map_err(|detail| unreadable(format, detail))?
    } else {
        let table = open(path, format, None)?;
        vec![SheetInfo {
            sheet: table.sheet,
            rows: table.total_rows,
            columns: table.header.len(),
        }]
    };
    Ok(TableInfo { format, sheets })
}

pub fn schema(path: &Path, sheet: Option<&str>) -> Result<Schema, ErrorInfo> {
    let format = format_of(path)?;
    let mut table = open(path, format, sheet)?;
    let width = table.header.len();
    let mut kinds = vec!["empty"; width];
    let mut empty = vec![0; width];
    let mut sampled = 0;
    for row in table.rows.by_ref().take(SAMPLE_ROWS as usize) {
        let row = row.map_err(|detail| unreadable(format, detail))?;
        sampled += 1;
        for (i, value) in row.iter().enumerate().take(width) {
            if value.is_null() {
                empty[i] += 1;
            }
            kinds[i] = cells::merge(kinds[i], cells::kind(value));
        }
    }
    let columns = table
        .header
        .into_iter()
        .enumerate()
        .map(|(i, column)| Column {
            column,
            kind: match &table.types {
                Some(types) => types[i].clone(),
                None => kinds[i].to_string(),
            },
            empty: empty[i],
        })
        .collect();
    Ok(Schema {
        format,
        sheet: table.sheet,
        rows: table.total_rows,
        sampled,
        columns,
        warnings: table.warnings,
    })
}

/// Every data row of one sheet in order, streamed: `visit` gets the 1-based
/// row number, the header and the cells, and returns false to stop.
pub fn scan(
    path: &Path,
    sheet: Option<&str>,
    mut visit: impl FnMut(u64, &[String], &[Value]) -> bool,
) -> Result<(), ErrorInfo> {
    let format = format_of(path)?;
    let table = open(path, format, sheet)?;
    for (i, row) in table.rows.enumerate() {
        let row = row.map_err(|detail| unreadable(format, detail))?;
        if !visit(i as u64 + 1, &table.header, &row) {
            break;
        }
    }
    Ok(())
}

pub fn rows(req: &RowsRequest) -> Result<Rows, ErrorInfo> {
    let format = format_of(&req.path)?;
    let table = open(&req.path, format, req.sheet.as_deref())?;
    let picked = pick_columns(&table.header, &req.columns)?;
    let total = table.total_rows;
    let (first, last) = match req.rows {
        Some(range) => {
            let count = u32::try_from(total).unwrap_or(u32::MAX);
            let (first, last) = range.bounds(count, "--rows")?;
            (u64::from(first), u64::from(last))
        }
        None if total == 0 => (1, 0),
        None => (1, total.min(DEFAULT_ROWS)),
    };
    let mut rows = vec![];
    let mut requested_chars = 0;
    let mut budget = req.max_chars.unwrap_or(usize::MAX);
    let mut cut = false;
    let wanted = (last + 1).saturating_sub(first) as usize;
    for row in table.rows.skip(first as usize - 1).take(wanted) {
        let row = row.map_err(|detail| unreadable(format, detail))?;
        let object: Map<String, Value> = picked
            .iter()
            .map(|&i| {
                (
                    table.header[i].clone(),
                    row.get(i).cloned().unwrap_or(Value::Null),
                )
            })
            .collect();
        let chars = Value::Object(object.clone()).to_string().chars().count();
        requested_chars += chars;
        // Rows stay contiguous: stop at the first that does not fit, but return
        // at least one.
        if rows.is_empty() || (!cut && chars <= budget) {
            budget = budget.saturating_sub(chars);
            rows.push(object);
        } else {
            cut = true;
        }
    }
    let returned_last = first + rows.len() as u64 - 1;
    Ok(Rows {
        format,
        sheet: table.sheet,
        total_rows: total,
        range: match rows.len() {
            0 => "none".into(),
            1 => first.to_string(),
            _ => format!("{first}-{returned_last}"),
        },
        rows,
        last_requested: last,
        requested_chars,
        warnings: table.warnings,
    })
}

fn open(path: &Path, format: Format, sheet: Option<&str>) -> Result<Table, ErrorInfo> {
    let unreadable = |detail| unreadable(format, detail);
    let table = match format {
        Format::Csv | Format::Tsv => {
            let name = only_sheet(path, sheet)?;
            let delimiter = if format == Format::Csv { b',' } else { b'\t' };
            delimited::open(path, delimiter, name).map_err(unreadable)?
        }
        Format::Parquet => {
            let name = only_sheet(path, sheet)?;
            columnar::open(path, name).map_err(unreadable)?
        }
        Format::Xlsx | Format::Xls | Format::Ods => {
            let names = sheet::names(path).map_err(unreadable)?;
            let name = pick_sheet(&names, sheet)?;
            sheet::open(path, name).map_err(unreadable)?
        }
        Format::Sqlite => {
            let names = sqlite::names(path).map_err(unreadable)?;
            let name = pick_sheet(&names, sheet)?;
            sqlite::open(path, name).map_err(unreadable)?
        }
    };
    Ok(table)
}

fn format_of(path: &Path) -> Result<Format, ErrorInfo> {
    check_input(path)?;
    Format::from_path(path).ok_or_else(|| {
        let ext = path
            .extension()
            .map(|ext| format!("`.{}`", ext.to_string_lossy()))
            .unwrap_or_else(|| "without an extension".into());
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            format!("Files {ext} are not supported."),
            "Use .csv, .tsv, .xlsx, .xls, .ods, .parquet or .sqlite.".to_string(),
        )
    })
}

/// Single-sheet formats accept only their own name or `1` as `--sheet`.
fn only_sheet(path: &Path, want: Option<&str>) -> Result<String, ErrorInfo> {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default();
    match want {
        None | Some("1") => Ok(name),
        Some(want) if want == name => Ok(name),
        Some(want) => Err(ErrorInfo::new(
            ErrorCode::InvalidInput,
            format!("Sheet `{want}` not found; this file has one sheet, `{name}`."),
            "Drop --sheet for this file.".to_string(),
        )),
    }
}

fn pick_sheet(names: &[String], want: Option<&str>) -> Result<String, ErrorInfo> {
    let found = match want {
        None => names.first(),
        Some(want) => want
            .parse::<usize>()
            .ok()
            .and_then(|n| names.get(n.wrapping_sub(1)))
            .or_else(|| names.iter().find(|name| *name == want))
            .or_else(|| names.iter().find(|name| name.eq_ignore_ascii_case(want))),
    };
    found.cloned().ok_or_else(|| {
        ErrorInfo::new(
            ErrorCode::InvalidInput,
            match want {
                Some(want) => format!("Sheet `{want}` not found."),
                None => "The workbook has no sheets.".into(),
            },
            format!("Sheets: {}. Use --sheet <name|number>.", names.join(", ")),
        )
    })
}

fn pick_columns(header: &[String], want: &[String]) -> Result<Vec<usize>, ErrorInfo> {
    if want.is_empty() {
        return Ok((0..header.len()).collect());
    }
    want.iter()
        .map(|name| {
            header
                .iter()
                .position(|column| column == name)
                .or_else(|| {
                    header
                        .iter()
                        .position(|column| column.eq_ignore_ascii_case(name))
                })
                .or_else(|| {
                    name.parse::<usize>()
                        .ok()
                        .filter(|n| (1..=header.len()).contains(n))
                        .map(|n| n - 1)
                })
                .ok_or_else(|| {
                    ErrorInfo::new(
                        ErrorCode::InvalidInput,
                        format!("Column `{name}` not found."),
                        format!("Columns: {}.", header.join(", ")),
                    )
                })
        })
        .collect()
}

fn unreadable(format: Format, detail: String) -> ErrorInfo {
    ErrorInfo::new(
        ErrorCode::InvalidInput,
        format!(
            "The file is not a valid {}, or it is damaged.",
            format.name()
        ),
        "Check the file, or open and re-save it.".to_string(),
    )
    .with_detail(detail)
}

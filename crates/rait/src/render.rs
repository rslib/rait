use rait_core::{ErrorCode, ErrorInfo, Record, Truncation};
use serde_json::{Map, Value, json};
use std::io::{self, Write};
use std::process::ExitCode;

#[derive(Clone, Copy, PartialEq, clap::ValueEnum)]
pub enum Format {
    Toon,
    Json,
    Text,
}

type Shape = Box<dyn Fn(Value) -> Value>;

/// Records plus how to present them in TOON: successes become a `noun` table keyed
/// by `key`, failures an `errors` table.
pub struct Output {
    noun: &'static str,
    key: &'static str,
    records: Vec<Record>,
    help: Vec<String>,
    shape: Option<Shape>,
}

impl Output {
    pub fn new(noun: &'static str, key: &'static str, records: Vec<Record>) -> Self {
        Self {
            noun,
            key,
            records,
            help: vec![],
            shape: None,
        }
    }

    pub fn help<S: ToString>(mut self, lines: impl IntoIterator<Item = S>) -> Self {
        self.help
            .extend(lines.into_iter().map(|line| line.to_string()));
        self
    }

    /// Rewrite each success's `data` for TOON. JSON output is never shaped.
    pub fn shape(mut self, shape: impl Fn(Value) -> Value + 'static) -> Self {
        self.shape = Some(Box::new(shape));
        self
    }
}

pub fn write_stdout(text: &str) {
    // A closed pipe (e.g. `| head`) is not an error worth reporting.
    let _ = io::stdout().lock().write_all(text.as_bytes());
}

pub fn emit(format: Format, output: Output) -> ExitCode {
    for record in &output.records {
        if let Some(detail) = record.error.as_ref().and_then(|err| err.detail.as_ref()) {
            eprintln!("rait: {}: {detail}", record.input);
        }
    }
    let ok = output.records.iter().all(|record| record.ok);
    match format {
        Format::Json => print_value(format, &json!(output.records)),
        Format::Toon => print_value(format, &toon(output)),
        Format::Text => output.records.iter().for_each(print_text),
    }
    ExitCode::from(if ok { 0 } else { 1 })
}

pub fn emit_usage_error(format: Format, err: ErrorInfo) -> ExitCode {
    if let Some(detail) = &err.detail {
        eprintln!("rait: {detail}");
    }
    print_value(format, &json!({ "error": err }));
    ExitCode::from(2)
}

pub fn print_value(format: Format, value: &Value) {
    let text = match format {
        Format::Json => serde_json::to_string(value).map_err(|err| err.to_string()),
        _ => toon_format::encode_default(value)
            .map(|toon| expand_lists(&toon))
            .map_err(|err| err.to_string()),
    };
    match text {
        Ok(text) => write_stdout(&format!("{text}\n")),
        Err(err) => write_stdout(&format!("error: output serialization failed: {err}\n")),
    }
}

/// Rewrite inline lists of plain values (`key[N]: a,b`) one item per line, as
/// AXI tools print them. Items stay unquoted unless quoting keeps them intact.
fn expand_lists(toon: &str) -> String {
    let mut out = String::with_capacity(toon.len());
    for line in toon.lines() {
        match inline_list(line) {
            Some((head, indent, items)) => {
                out.push_str(head);
                out.push_str(":\n");
                for item in items {
                    out.push_str(&" ".repeat(indent));
                    out.push_str(&item);
                    out.push('\n');
                }
            }
            None => {
                out.push_str(line);
                out.push('\n');
            }
        }
    }
    out.pop();
    out
}

/// Split `key[N]: a,"b, c"` into `key[N]`, the item indent and the items.
fn inline_list(line: &str) -> Option<(&str, usize, Vec<String>)> {
    let body = line.trim_start();
    let mut indent = line.len() - body.len();
    let key = match body.strip_prefix("- ") {
        Some(rest) => {
            indent += 2;
            rest
        }
        None => body,
    };
    let open = key.find('[')?;
    let close = open + key[open..].find("]: ")?;
    let count = &key[open + 1..close];
    if open == 0 || key[..open].starts_with('"') || !count.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let head_len = line.len() - key.len() + close + 1;
    let values = &key[close + 3..];
    Some((&line[..head_len], indent + 2, split_values(values)))
}

fn split_values(values: &str) -> Vec<String> {
    let mut items = vec![];
    let mut current = String::new();
    let mut quoted = false;
    let mut escaped = false;
    for c in values.chars() {
        match c {
            _ if escaped => {
                current.push(c);
                escaped = false;
            }
            '\\' if quoted => {
                current.push(c);
                escaped = true;
            }
            '"' => {
                current.push(c);
                quoted = !quoted;
            }
            ',' if !quoted => items.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    items.push(current);
    items.into_iter().map(|item| unquote(&item)).collect()
}

/// Drop TOON quotes when the plain text reads the same on its own line.
fn unquote(item: &str) -> String {
    let Some(inner) = item.strip_prefix('"').and_then(|i| i.strip_suffix('"')) else {
        return item.to_string();
    };
    if inner.is_empty() || inner.contains("\\n") || inner.trim() != inner {
        return item.to_string();
    }
    inner.replace("\\\"", "\"").replace("\\\\", "\\")
}

fn toon(output: Output) -> Value {
    let Output {
        noun,
        key,
        records,
        help,
        shape,
    } = output;
    let mut doc = Map::new();
    doc.insert("count".into(), json!(records.len()));
    let failed = records.iter().filter(|record| !record.ok).count();
    if failed > 0 {
        doc.insert("failed".into(), json!(failed));
    }
    let items: Vec<Value> = records
        .iter()
        .filter(|record| record.ok)
        .map(|record| {
            let mut item = Map::new();
            item.insert(key.into(), json!(record.input));
            let data = record.data.clone().unwrap_or(Value::Null);
            let data = match &shape {
                Some(shape) => shape(data),
                None => data,
            };
            if let Value::Object(fields) = data {
                item.extend(fields);
            }
            if let Some(truncation) = &record.truncation {
                item.insert("truncated".into(), truncated(truncation));
            }
            Value::Object(item)
        })
        .collect();
    if !items.is_empty() {
        doc.insert(noun.into(), json!(items));
    }
    let errors: Vec<Value> = records
        .iter()
        .filter_map(|record| {
            let err = record.error.as_ref()?;
            Some(json!({
                key: record.input,
                "code": err.code,
                "message": err.message,
                "hint": err.hint,
            }))
        })
        .collect();
    if !errors.is_empty() {
        doc.insert("errors".into(), json!(errors));
    }
    let warnings = hoist_warnings(&records);
    if !warnings.is_empty() {
        doc.insert("warnings".into(), json!(warnings));
    }
    let mut help = help;
    if records.iter().any(|record| {
        record
            .error
            .as_ref()
            .is_some_and(|err| err.code == ErrorCode::DependencyMissing)
    }) {
        help.push("Run `rait doctor` to see install commands".into());
    }
    if !help.is_empty() {
        doc.insert("help".into(), json!(help));
    }
    Value::Object(doc)
}

fn truncated(truncation: &Truncation) -> Value {
    json!({
        "returned_chars": truncation.returned_chars,
        "total_chars": truncation.total_chars,
        "help": truncation.hint,
    })
}

/// One line per distinct warning. A warning shared by every record is shown once;
/// otherwise it names the inputs it applies to.
fn hoist_warnings(records: &[Record]) -> Vec<String> {
    let mut seen: Vec<(&str, Vec<&str>)> = vec![];
    for record in records {
        for warning in &record.warnings {
            match seen.iter_mut().find(|(text, _)| text == warning) {
                Some((_, inputs)) => inputs.push(&record.input),
                None => seen.push((warning, vec![&record.input])),
            }
        }
    }
    seen.into_iter()
        .map(|(text, inputs)| {
            if inputs.len() == records.len() {
                text.to_string()
            } else {
                format!("{}: {text}", inputs.join(", "))
            }
        })
        .collect()
}

pub fn shell_quote(text: &str) -> String {
    let plain = text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "-_./~+:@%".contains(c));
    if plain && !text.is_empty() {
        text.into()
    } else {
        format!("'{}'", text.replace('\'', r"'\''"))
    }
}

fn print_text(record: &Record) {
    let line = match (&record.data, &record.error) {
        (Some(data), _) => match data
            .get("pages")
            .or(data.get("parts"))
            .and_then(Value::as_array)
        {
            Some(units) => {
                units
                    .iter()
                    .filter_map(|unit| unit["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n")
                    + "\n"
            }
            None => format!("{}: {data}\n", record.input),
        },
        (None, Some(err)) => format!("{}: {:?}: {}\n", record.input, err.code, err.message),
        (None, None) => return,
    };
    write_stdout(&line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_inline_lists_one_item_per_line() {
        let toon = "count: 1\nhelp[2]: Run `a`,\"Run `b \\\"q\\\"`, then c\"\nrows[1]{a,b}:\n  1,2\n  - tags[2]: x,\" y\"\nempty[0]:";
        assert_eq!(
            expand_lists(toon),
            "count: 1\nhelp[2]:\n  Run `a`\n  Run `b \"q\"`, then c\nrows[1]{a,b}:\n  1,2\n  - tags[2]:\n      x\n      \" y\"\nempty[0]:"
        );
    }
}

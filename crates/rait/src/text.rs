//! `--format text`: output for people. One aligned table when every input
//! gives one flat row, else a block per input; page and part text as plain
//! text. Warnings, truncation notes and next steps go to stderr, so piped
//! text stays clean.

use serde_json::{Map, Value};

/// Width tables are fitted to when `$COLUMNS` is not set.
const DEFAULT_WIDTH: usize = 120;
/// Narrowest a column is shrunk to when a table is too wide.
const MIN_COLUMN: usize = 8;

/// `doc` is the TOON document: `count`, the `noun` list keyed by `key`,
/// `errors`, `warnings` and `help`. Returns (stdout, stderr).
pub fn render(doc: &Value, noun: &str, key: &str) -> (String, String) {
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(DEFAULT_WIDTH);
    let mut out = String::new();
    let mut notes = String::new();
    let items = doc[noun].as_array().map(Vec::as_slice).unwrap_or_default();
    if items.iter().all(|item| text_units(item).is_some()) && !items.is_empty() {
        for item in items {
            if items.len() > 1 {
                out.push_str(&format!("==> {} <==\n", scalar(&item[key])));
            }
            out.push_str(&text_units(item).unwrap_or_default());
        }
    } else if !items.is_empty() && items.iter().all(flat) {
        let rows: Vec<&Map<String, Value>> = items.iter().filter_map(Value::as_object).collect();
        out.push_str(&table(&rows, "", width));
    } else {
        for (i, item) in items.iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&scalar(&item[key]));
            out.push('\n');
            if let Some(fields) = item.as_object() {
                let rest: Map<String, Value> = fields
                    .iter()
                    .filter(|(k, _)| *k != key && *k != "truncated")
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect();
                out.push_str(&block(&rest, "  ", width));
            }
        }
    }
    for err in doc["errors"].as_array().into_iter().flatten() {
        out.push_str(&format!(
            "error: {}: {}\n",
            scalar(&err[key]),
            scalar(&err["message"])
        ));
        if let Some(hint) = err["hint"].as_str() {
            out.push_str(&format!("  hint: {hint}\n"));
        }
    }
    for item in items {
        if let Some(help) = item["truncated"]["help"].as_str() {
            notes.push_str(&format!("note: output truncated. {help}\n"));
        }
    }
    for warning in doc["warnings"].as_array().into_iter().flatten() {
        notes.push_str(&format!("warning: {}\n", scalar(warning)));
    }
    for help in doc["help"].as_array().into_iter().flatten() {
        notes.push_str(&format!("next: {}\n", scalar(help)));
    }
    (out, notes)
}

/// A plain document such as the home view: `key  value` lines and lists,
/// with `help` as next steps on stderr. Returns (stdout, stderr).
pub fn document(value: &Value) -> (String, String) {
    let Some(fields) = value.as_object() else {
        return (format!("{}\n", scalar(value)), String::new());
    };
    if let Some(err) = fields.get("error") {
        let mut out = format!("error: {}\n", scalar(&err["message"]));
        if let Some(hint) = err["hint"].as_str() {
            out.push_str(&format!("  hint: {hint}\n"));
        }
        return (out, String::new());
    }
    let body: Map<String, Value> = fields
        .iter()
        .filter(|(k, _)| *k != "help")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let width = std::env::var("COLUMNS")
        .ok()
        .and_then(|c| c.parse().ok())
        .unwrap_or(DEFAULT_WIDTH);
    let notes = fields["help"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|h| format!("next: {}\n", scalar(h)))
        .collect();
    (block(&body, "", width), notes)
}

/// The page or part text of a `pdf text` or `doc text` item.
fn text_units(item: &Value) -> Option<String> {
    let units = item.get("pages").or(item.get("parts"))?.as_array()?;
    let texts: Option<Vec<&str>> = units.iter().map(|u| u["text"].as_str()).collect();
    let texts = texts?;
    Some(texts.join("\n\n") + "\n")
}

fn flat(item: &Value) -> bool {
    item.as_object()
        .is_some_and(|fields| fields.values().all(|v| !v.is_array() && !v.is_object()))
}

/// Fields as aligned `key  value` lines; lists of rows as tables and nested
/// objects as indented blocks below their key.
fn block(fields: &Map<String, Value>, indent: &str, width: usize) -> String {
    let mut out = String::new();
    let scalars: Vec<(&String, &Value)> = fields
        .iter()
        .filter(|(_, v)| !v.is_array() && !v.is_object())
        .collect();
    let pad = scalars
        .iter()
        .map(|(k, _)| label(k).chars().count())
        .max()
        .unwrap_or(0);
    for (k, v) in &scalars {
        let value = scalar(v);
        if value.contains('\n') {
            out.push_str(&format!("{indent}{}:\n", label(k)));
            for line in value.lines() {
                out.push_str(&format!("{indent}  {line}\n"));
            }
        } else {
            out.push_str(&format!("{indent}{:pad$}  {value}\n", label(k)));
        }
    }
    for (k, v) in fields.iter().filter(|(_, v)| v.is_array() || v.is_object()) {
        match v {
            Value::Array(list) if list.is_empty() => {
                out.push_str(&format!("{indent}{}: none\n", label(k)));
            }
            Value::Array(list) if list.iter().all(|x| x.is_object()) => {
                out.push_str(&format!("{indent}{} ({}):\n", label(k), list.len()));
                let rows: Vec<&Map<String, Value>> =
                    list.iter().filter_map(Value::as_object).collect();
                if rows
                    .iter()
                    .all(|r| r.values().all(|v| !v.is_array() && !v.is_object()))
                {
                    out.push_str(&table(&rows, &format!("{indent}  "), width));
                } else {
                    for row in rows {
                        out.push_str(&block(row, &format!("{indent}  "), width));
                        out.push('\n');
                    }
                }
            }
            Value::Array(list) => {
                out.push_str(&format!("{indent}{}:\n", label(k)));
                for x in list {
                    out.push_str(&format!("{indent}  - {}\n", scalar(x)));
                }
            }
            Value::Object(inner) => {
                out.push_str(&format!("{indent}{}:\n", label(k)));
                out.push_str(&block(inner, &format!("{indent}  "), width));
            }
            _ => {}
        }
    }
    out
}

/// Rows as columns under an upper-case header, shrinking the widest columns
/// with `…` until the table fits `width`.
fn table(rows: &[&Map<String, Value>], indent: &str, width: usize) -> String {
    let mut columns: Vec<&String> = vec![];
    for row in rows {
        for k in row.keys() {
            if !columns.contains(&k) {
                columns.push(k);
            }
        }
    }
    let header: Vec<String> = columns.iter().map(|c| label(c).to_uppercase()).collect();
    let cells: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            columns
                .iter()
                .map(|c| row.get(*c).map_or("-".into(), scalar).replace('\n', " "))
                .collect()
        })
        .collect();
    let mut widths: Vec<usize> = (0..columns.len())
        .map(|i| {
            cells
                .iter()
                .map(|r| r[i].chars().count())
                .chain([header[i].chars().count()])
                .max()
                .unwrap_or(0)
        })
        .collect();
    let room = width.saturating_sub(indent.len() + 2 * columns.len().saturating_sub(1));
    while widths.iter().sum::<usize>() > room {
        let (i, &w) = widths
            .iter()
            .enumerate()
            .max_by_key(|(_, w)| **w)
            .unwrap_or((0, &0));
        if w <= MIN_COLUMN {
            break;
        }
        widths[i] = w - 1;
    }
    let line = |values: &[String]| {
        let last = values.len().saturating_sub(1);
        let mut out = indent.to_string();
        for (i, v) in values.iter().enumerate() {
            let v = fit(v, widths[i]);
            if i == last {
                out.push_str(&v);
            } else {
                out.push_str(&format!("{v:w$}  ", w = widths[i]));
            }
        }
        out.trim_end().to_string() + "\n"
    };
    let mut out = line(&header);
    for row in &cells {
        out.push_str(&line(row));
    }
    out
}

fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(width.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

fn label(key: &str) -> String {
    key.replace('_', " ")
}

fn scalar(value: &Value) -> String {
    match value {
        Value::Null => "-".into(),
        Value::Bool(true) => "yes".into(),
        Value::Bool(false) => "no".into(),
        Value::String(s) => s.clone(),
        Value::Array(list) => list.iter().map(scalar).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn flat_records_form_one_table() {
        let doc = json!({
            "count": 2,
            "programs": [
                {"program": "pdfinfo", "available": true, "version": "26.09.0"},
                {"program": "qpdf", "available": false, "version": null}
            ],
            "help": ["Install qpdf"]
        });
        let (out, notes) = render(&doc, "programs", "program");
        assert_eq!(
            out,
            "PROGRAM  AVAILABLE  VERSION\npdfinfo  yes        26.09.0\nqpdf     no         -\n"
        );
        assert_eq!(notes, "next: Install qpdf\n");
    }

    #[test]
    fn nested_records_form_blocks_with_tables() {
        let doc = json!({
            "count": 1,
            "pdfs": [{
                "file": "a.pdf",
                "total_pages": 3,
                "entries": [{"level": 1, "title": "Intro", "page": 1}]
            }],
            "errors": [{"file": "b.pdf", "code": "invalid_input", "message": "Bad.", "hint": "Fix it."}]
        });
        let (out, _) = render(&doc, "pdfs", "file");
        assert_eq!(
            out,
            "a.pdf\n  total pages  3\n  entries (1):\n    LEVEL  TITLE  PAGE\n    1      Intro  1\nerror: b.pdf: Bad.\n  hint: Fix it.\n"
        );
    }

    #[test]
    fn documents_and_usage_errors_read_as_text() {
        let (out, notes) = document(&json!({"bin": "rait", "files": ["a.pdf"], "help": ["Run x"]}));
        assert_eq!(out, "bin  rait\nfiles:\n  - a.pdf\n");
        assert_eq!(notes, "next: Run x\n");
        let (out, _) = document(
            &json!({"error": {"code": "invalid_input", "message": "Bad flag.", "hint": "Run y"}}),
        );
        assert_eq!(out, "error: Bad flag.\n  hint: Run y\n");
    }

    #[test]
    fn wide_tables_shrink_to_fit() {
        let row = json!({"a": "x".repeat(50), "b": "y".repeat(50)});
        let rows = [row.as_object().unwrap()];
        let out = table(&rows, "", 40);
        assert!(out.lines().all(|l| l.chars().count() <= 40), "{out}");
        assert!(out.contains('…'));
    }
}

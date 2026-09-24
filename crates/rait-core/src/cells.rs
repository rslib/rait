use serde_json::{Map, Number, Value};

/// Type a text cell: empty is null, plain integers and decimals are numbers,
/// `true`/`false` are booleans. Numbers with leading zeros stay text, as they are
/// usually codes such as ZIP codes.
pub fn parse(text: &str) -> Value {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Value::Null;
    }
    match trimmed {
        "true" => return Value::Bool(true),
        "false" => return Value::Bool(false),
        _ => {}
    }
    let digits = trimmed.strip_prefix('-').unwrap_or(trimmed);
    let leading_zero = digits.len() > 1 && digits.starts_with('0') && !digits.starts_with("0.");
    let numeric = digits.chars().next().is_some_and(|c| c.is_ascii_digit())
        && digits
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    if numeric && !leading_zero {
        if let Ok(int) = trimmed.parse::<i64>() {
            return Value::Number(int.into());
        }
        if let Some(float) = trimmed.parse::<f64>().ok().and_then(Number::from_f64) {
            return Value::Number(float);
        }
    }
    Value::String(text.to_string())
}

/// Column names that are unique and non-empty.
pub fn header(raw: Vec<String>) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    for (i, name) in raw.into_iter().enumerate() {
        let base = match name.trim() {
            "" => format!("column_{}", i + 1),
            name => name.to_string(),
        };
        let mut name = base.clone();
        let mut n = 2;
        while names.contains(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        names.push(name);
    }
    names
}

/// Grid rows as objects keyed by the first row, which becomes the header.
/// Cells are typed with [`parse`]; short rows are padded with nulls.
pub fn keyed(rows: &[Vec<String>]) -> Vec<Map<String, Value>> {
    let Some((head, body)) = rows.split_first() else {
        return vec![];
    };
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let mut names = head.clone();
    names.resize(width, String::new());
    let names = header(names);
    body.iter()
        .map(|row| {
            names
                .iter()
                .enumerate()
                .map(|(i, name)| {
                    (
                        name.clone(),
                        row.get(i).map_or(Value::Null, |cell| parse(cell)),
                    )
                })
                .collect()
        })
        .collect()
}

/// GitHub-flavoured Markdown table; the first row is the header. Pipes are
/// escaped and line breaks inside a cell become `<br>`.
pub fn markdown_table(rows: &[Vec<String>]) -> String {
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let line = |row: &[String]| {
        let cells: Vec<String> = (0..width)
            .map(|i| row.get(i).map_or(String::new(), |cell| cell_text(cell)))
            .collect();
        format!("| {} |", cells.join(" | "))
    };
    let mut lines = vec![line(&rows[0])];
    lines.push(format!("|{}", " --- |".repeat(width)));
    lines.extend(rows[1..].iter().map(|row| line(row)));
    lines.join("\n")
}

fn cell_text(cell: &str) -> String {
    cell.trim()
        .replace('|', "\\|")
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("<br>")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn types_text_cells() {
        assert_eq!(parse(""), Value::Null);
        assert_eq!(parse("42"), json!(42));
        assert_eq!(parse("-3.5"), json!(-3.5));
        assert_eq!(parse("0.5"), json!(0.5));
        assert_eq!(parse("01001"), json!("01001"));
        assert_eq!(parse("true"), json!(true));
        assert_eq!(parse("1e3"), json!(1000.0));
        assert_eq!(parse("2026-01-01"), json!("2026-01-01"));
        assert_eq!(parse("12abc"), json!("12abc"));
    }

    #[test]
    fn dedupes_headers() {
        assert_eq!(
            header(vec!["a".into(), "a".into(), "".into(), "a".into()]),
            ["a", "a_2", "column_3", "a_3"]
        );
    }

    #[test]
    fn keys_rows_by_header() {
        let rows = vec![
            vec!["a".to_string(), "a".into()],
            vec!["1".into(), "x".into(), "extra".into()],
            vec!["2".into()],
        ];
        let got = keyed(&rows);
        assert_eq!(
            Value::Object(got[0].clone()),
            json!({"a": 1, "a_2": "x", "column_3": "extra"})
        );
        assert_eq!(
            Value::Object(got[1].clone()),
            json!({"a": 2, "a_2": null, "column_3": null})
        );
    }
}

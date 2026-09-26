use crate::html;
use crate::markdown::{Block, push};
use serde_json::{Map, Value};

const MAX_OUTPUT_LINES: usize = 20;
const MAX_OUTPUT_CHARS: usize = 2000;

/// Blocks of a Jupyter notebook, one group of cells per part.
pub fn blocks(json: &str) -> Result<Vec<Vec<Block>>, String> {
    let notebook: Value = serde_json::from_str(json).map_err(|err| err.to_string())?;
    let cells = notebook
        .get("cells")
        .and_then(Value::as_array)
        .ok_or("no \"cells\" array")?;
    let lang = language(&notebook);
    let mut parts: Vec<Vec<Block>> = vec![];
    for cell in cells {
        let cell_type = cell.get("cell_type").and_then(Value::as_str).unwrap_or("");
        let source = text_field(cell.get("source"));
        let blocks = match cell_type {
            "markdown" => markdown_cell_blocks(&source),
            "code" => code_cell_blocks(cell, &source, &lang),
            _ => vec![Block::Paragraph(source.clone())],
        };
        if blocks.is_empty() {
            continue;
        }
        let starts_part =
            cell_type == "markdown" && source.lines().next().is_some_and(|l| l.starts_with("# "));
        match parts.last_mut() {
            Some(part) if !starts_part || part.is_empty() => part.extend(blocks),
            _ => parts.push(blocks),
        }
    }
    Ok(parts)
}

/// The kernel's language, for the code cells' fence info-string.
fn language(notebook: &Value) -> String {
    notebook
        .pointer("/metadata/kernelspec/language")
        .or_else(|| notebook.pointer("/metadata/language_info/name"))
        .and_then(Value::as_str)
        .unwrap_or("python")
        .to_string()
}

fn code_cell_blocks(cell: &Value, source: &str, lang: &str) -> Vec<Block> {
    let mut blocks = vec![];
    push(
        &mut blocks,
        Block::Code(Some(lang.to_string()), source.to_string()),
    );
    for output in cell
        .get("outputs")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        blocks.extend(output_blocks(output));
    }
    blocks
}

fn output_blocks(output: &Value) -> Vec<Block> {
    match output.get("output_type").and_then(Value::as_str) {
        Some("stream") => {
            let text = text_field(output.get("text"));
            vec![Block::Code(
                Some("text".into()),
                truncate(&strip_ansi(&text)),
            )]
        }
        Some("execute_result" | "display_data") => {
            let empty = Map::new();
            let data = output
                .get("data")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            data_blocks(data)
        }
        Some("error") => {
            let ename = output
                .get("ename")
                .and_then(Value::as_str)
                .unwrap_or("Error");
            let evalue = output.get("evalue").and_then(Value::as_str).unwrap_or("");
            vec![Block::Paragraph(strip_ansi(&format!("{ename}: {evalue}")))]
        }
        _ => vec![],
    }
}

/// Image mime types, which never get inlined: always a placeholder.
const IMAGE_MIMES: &[&str] = &["image/png", "image/jpeg", "image/svg+xml"];

fn data_blocks(data: &Map<String, Value>) -> Vec<Block> {
    let mut blocks = vec![];
    if let Some(text) = data.get("text/plain") {
        blocks.push(Block::Code(
            Some("text".into()),
            truncate(&strip_ansi(&text_field(Some(text)))),
        ));
    }
    if let Some(markdown) = data.get("text/markdown") {
        blocks.push(Block::Paragraph(truncate(&strip_ansi(&text_field(Some(
            markdown,
        ))))));
    }
    for mime in IMAGE_MIMES.iter().filter(|mime| data.contains_key(**mime)) {
        blocks.push(Block::Paragraph(format!("[{mime} output omitted]")));
    }
    if let Some(html) = data.get("text/html") {
        let tables: Vec<Block> = html::blocks(&text_field(Some(html)))
            .into_iter()
            .filter(|block| matches!(block, Block::Table(_)))
            .collect();
        if !tables.is_empty() {
            blocks.extend(tables);
        } else if !data.contains_key("text/plain") && !data.contains_key("text/markdown") {
            blocks.push(Block::Paragraph("[text/html output omitted]".into()));
        }
    }
    blocks
}

/// Blocks of a Markdown cell: its source verbatim, with any pipe or HTML tables
/// pulled out as [`Block::Table`] so `rait doc tables` can find them.
fn markdown_cell_blocks(source: &str) -> Vec<Block> {
    let mut blocks = vec![];
    // Lowercase byte-for-byte (not `to_lowercase`) so indices still line up with `source`.
    let lower = source.to_ascii_lowercase();
    let mut pos = 0;
    while let Some(start) = lower[pos..].find("<table") {
        let start = pos + start;
        let Some(end) = lower[start..].find("</table>") else {
            break;
        };
        let end = start + end + "</table>".len();
        push_pipe_tables(&mut blocks, &source[pos..start]);
        if let Some(table) = html::blocks(&source[start..end])
            .into_iter()
            .find(|block| matches!(block, Block::Table(_)))
        {
            push(&mut blocks, table);
        }
        pos = end;
    }
    push_pipe_tables(&mut blocks, &source[pos..]);
    blocks
}

/// Splits `text` into paragraph and pipe-table blocks.
fn push_pipe_tables(blocks: &mut Vec<Block>, text: &str) {
    let lines: Vec<&str> = text.lines().collect();
    let mut paragraph: Vec<&str> = vec![];
    let mut i = 0;
    while i < lines.len() {
        if is_table_row(lines[i]) && lines.get(i + 1).is_some_and(|l| is_separator_row(l)) {
            flush_paragraph(blocks, &mut paragraph);
            let mut end = i + 2;
            while end < lines.len() && is_table_row(lines[end]) {
                end += 1;
            }
            push(blocks, Block::Table(pipe_table_rows(&lines[i..end])));
            i = end;
        } else {
            paragraph.push(lines[i]);
            i += 1;
        }
    }
    flush_paragraph(blocks, &mut paragraph);
}

fn flush_paragraph(blocks: &mut Vec<Block>, lines: &mut Vec<&str>) {
    if !lines.is_empty() {
        push(blocks, Block::Paragraph(lines.join("\n")));
        lines.clear();
    }
}

fn is_table_row(line: &str) -> bool {
    line.trim_start().starts_with('|')
}

fn is_separator_row(line: &str) -> bool {
    let trimmed = line.trim();
    !trimmed.is_empty()
        && trimmed.contains('-')
        && trimmed.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn pipe_table_rows(lines: &[&str]) -> Vec<Vec<String>> {
    lines
        .iter()
        .enumerate()
        .filter(|(i, _)| *i != 1)
        .map(|(_, line)| {
            line.trim()
                .trim_start_matches('|')
                .trim_end_matches('|')
                .split('|')
                .map(|cell| cell.trim().to_string())
                .collect()
        })
        .collect()
}

/// A `source` or output `text` field, which is a string or a list of strings.
fn text_field(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Strips ANSI escape (CSI) sequences, such as the color codes in tracebacks.
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Caps an output block at [`MAX_OUTPUT_LINES`] lines and [`MAX_OUTPUT_CHARS`]
/// characters, noting how many lines were cut.
fn truncate(text: &str) -> String {
    let total_lines = text.lines().count();
    let mut out = String::new();
    let mut out_chars = 0usize;
    let mut kept = 0usize;
    for line in text.lines().take(MAX_OUTPUT_LINES) {
        let sep = usize::from(!out.is_empty());
        let line_chars = line.chars().count();
        if out_chars + sep + line_chars > MAX_OUTPUT_CHARS {
            let budget = MAX_OUTPUT_CHARS.saturating_sub(out_chars + sep);
            if budget > 0 {
                if sep == 1 {
                    out.push('\n');
                }
                out.push_str(take_chars(line, budget));
                kept += 1;
            }
            break;
        }
        if sep == 1 {
            out.push('\n');
        }
        out.push_str(line);
        out_chars += sep + line_chars;
        kept += 1;
    }
    if kept < total_lines {
        out.push_str(&format!(
            "\n... ({} more lines omitted)",
            total_lines - kept
        ));
    }
    out
}

fn take_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_csi_sequences() {
        assert_eq!(strip_ansi("\u{1b}[0;31merror\u{1b}[0m"), "error");
    }

    #[test]
    fn truncates_by_line_count() {
        let text = (1..=25)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let out = truncate(&text);
        assert!(out.contains("line 20"));
        assert!(!out.contains("line 21"));
        assert!(out.ends_with("(5 more lines omitted)"));
    }

    #[test]
    fn truncates_by_char_count() {
        let text = "x".repeat(3000);
        let out = truncate(&text);
        assert_eq!(out, "x".repeat(2000));
    }

    #[test]
    fn parses_a_pipe_table() {
        let mut blocks = vec![];
        push_pipe_tables(
            &mut blocks,
            "before\n| a | b |\n| --- | --- |\n| 1 | 2 |\nafter",
        );
        assert_eq!(
            blocks,
            vec![
                Block::Paragraph("before".into()),
                Block::Table(vec![
                    vec!["a".into(), "b".into()],
                    vec!["1".into(), "2".into()],
                ]),
                Block::Paragraph("after".into()),
            ]
        );
    }
}

//! Table detection from word positions. PDFs store no table structure, so this
//! is a heuristic: rows whose words split into aligned cells at wide gaps.

use crate::layout::{Page, Word};
use rait_core::cells::markdown_table;
use std::collections::HashSet;

/// Gap that separates cells between words on different Poppler lines, as a
/// share of the text height.
const CELL_GAP: f64 = 0.3;
/// Gap that separates cells even inside one Poppler line.
const WIDE_GAP: f64 = 3.0;
/// A gap between rows larger than this many text heights ends a table.
const ROW_GAP: f64 = 2.5;
/// Fewer rows than this, counting the header, is not treated as a table.
const MIN_ROWS: usize = 3;
/// Rows where two cells both have this many words are two prose columns.
const PROSE_WORDS: usize = 5;
/// Tables average at most this many words per cell.
const MAX_MEAN_WORDS: f64 = 4.0;
/// A one-cell line narrower than this share of the text width can be a
/// wrapped line of the cell above; wider ones are prose.
const WRAP_WIDTH: f64 = 0.5;
/// A Poppler block with a line of at least this many words and this share of
/// the text width is prose, and none of its words can be table cells. Removing
/// prose first keeps a table in one text column from pairing with the prose
/// beside it.
const PROSE_LINE_WORDS: usize = 6;
const PROSE_LINE_WIDTH: f64 = 0.35;

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Cell text; the first row is the header.
    pub rows: Vec<Vec<String>>,
    /// Indexes of the page words inside the table.
    pub words: HashSet<usize>,
}

/// A row of words on one baseline, split into cells.
struct Row {
    y0: f64,
    y1: f64,
    cells: Vec<Vec<usize>>,
}

impl Row {
    fn span(&self, words: &[Word], cell: usize) -> (f64, f64) {
        let ws = &self.cells[cell];
        (
            ws.iter().map(|&i| words[i].x0).fold(f64::MAX, f64::min),
            ws.iter().map(|&i| words[i].x1).fold(f64::MIN, f64::max),
        )
    }

    fn is_candidate(&self) -> bool {
        self.cells.len() >= 2
            && self
                .cells
                .iter()
                .filter(|cell| cell.len() >= PROSE_WORDS)
                .count()
                < 2
    }
}

pub fn detect(page: &Page) -> Vec<Table> {
    let words = &page.words;
    if words.is_empty() {
        return vec![];
    }
    let mut heights: Vec<f64> = words.iter().map(Word::height).collect();
    heights.sort_by(f64::total_cmp);
    let height = heights[heights.len() / 2];
    let left = words.iter().map(|w| w.x0).fold(f64::MAX, f64::min);
    let right = words.iter().map(|w| w.x1).fold(f64::MIN, f64::max);
    let prose = prose_words(page, (right - left) * PROSE_LINE_WIDTH);
    let rows = rows(words, height, &prose);
    let continues = |row: &Row| {
        let (x0, x1) = row.span(words, 0);
        row.cells.len() == 1 && x1 - x0 < WRAP_WIDTH * (right - left)
    };
    let mut tables = vec![];
    let mut run: Vec<&Row> = vec![];
    for row in &rows {
        let close = run
            .last()
            .is_some_and(|last| row.y0 - last.y1 > ROW_GAP * height);
        if close || !(row.is_candidate() || !run.is_empty() && continues(row)) {
            tables.extend(build(&run, words));
            run.clear();
        }
        if row.is_candidate() || !run.is_empty() && continues(row) {
            run.push(row);
        }
    }
    tables.extend(build(&run, words));
    tables
}

fn prose_words(page: &Page, min_width: f64) -> HashSet<usize> {
    let words = &page.words;
    page.blocks
        .iter()
        .filter(|block| {
            block.iter().any(|line| {
                let (Some(&first), Some(&last)) = (line.first(), line.last()) else {
                    return false;
                };
                line.len() >= PROSE_LINE_WORDS && words[last].x1 - words[first].x0 >= min_width
            })
        })
        .flatten()
        .flatten()
        .copied()
        .collect()
}

fn rows(words: &[Word], height: f64, skip: &HashSet<usize>) -> Vec<Row> {
    let mut order: Vec<usize> = (0..words.len()).filter(|i| !skip.contains(i)).collect();
    order.sort_by(|&a, &b| words[a].center_y().total_cmp(&words[b].center_y()));
    let mut lines: Vec<Vec<usize>> = vec![];
    for i in order {
        match lines.last_mut() {
            Some(line)
                if (words[i].center_y() - words[line[0]].center_y()).abs() < 0.5 * height =>
            {
                line.push(i)
            }
            _ => lines.push(vec![i]),
        }
    }
    lines
        .into_iter()
        .map(|mut line| {
            line.sort_by(|&a, &b| words[a].x0.total_cmp(&words[b].x0));
            let mut cells: Vec<Vec<usize>> = vec![];
            for i in line {
                let joins = cells.last().is_some_and(|cell| {
                    let previous = &words[*cell.last().unwrap()];
                    let gap = words[i].x0 - previous.x1;
                    let limit = if previous.line == words[i].line {
                        WIDE_GAP
                    } else {
                        CELL_GAP
                    };
                    gap < limit * height
                });
                match cells.last_mut() {
                    Some(cell) if joins => cell.push(i),
                    _ => cells.push(vec![i]),
                }
            }
            let all = cells.iter().flatten();
            Row {
                y0: all.clone().map(|&i| words[i].y0).fold(f64::MAX, f64::min),
                y1: all.map(|&i| words[i].y1).fold(f64::MIN, f64::max),
                cells,
            }
        })
        .collect()
}

fn build(run: &[&Row], words: &[Word]) -> Option<Table> {
    let candidates: Vec<&&Row> = run.iter().filter(|r| r.is_candidate()).collect();
    if candidates.len() < MIN_ROWS {
        return None;
    }
    let cells: usize = candidates.iter().map(|r| r.cells.len()).sum();
    let words_in: usize = candidates.iter().flat_map(|r| &r.cells).map(Vec::len).sum();
    if words_in as f64 / cells as f64 > MAX_MEAN_WORDS {
        return None;
    }
    let columns = columns(&candidates, words);
    if columns.len() < 2 {
        return None;
    }
    let run = trim_trailing(run, words, columns[0].1);
    let mut grid: Vec<Vec<String>> = vec![];
    let mut used = HashSet::new();
    for row in run {
        let header = grid.len() <= 1;
        let mut line = vec![String::new(); columns.len()];
        for (c, cell) in row.cells.iter().enumerate() {
            let (x0, x1) = row.span(words, c);
            let text = cell
                .iter()
                .map(|&i| words[i].text.as_str())
                .collect::<Vec<_>>();
            // A header cell spanning columns labels each of them; elsewhere a
            // spanning cell fills its leftmost column, like a merged cell.
            let spanned = spanned(&columns, x0, x1);
            let targets = if header {
                spanned
            } else {
                spanned[..1].to_vec()
            };
            for col in targets {
                push_text(&mut line[col], &text.join(" "));
            }
            used.extend(cell.iter().copied());
        }
        let filled = line.iter().filter(|c| !c.is_empty()).count();
        match grid.last_mut() {
            // An empty first column continues the row above when it is a
            // second header line or a single wrapped cell. With more cells it
            // is a new row under a first-column value spanning several rows.
            Some(previous) if line[0].is_empty() && (header || filled == 1) => {
                for (cell, text) in previous.iter_mut().zip(line) {
                    push_text(cell, &text);
                }
            }
            _ => grid.push(line),
        }
    }
    let filled = grid.iter().flatten().filter(|c| !c.is_empty()).count();
    let total = grid.len() * columns.len();
    let header_filled = grid.first()?.iter().filter(|c| !c.is_empty()).count();
    (grid.len() >= 2 && filled * 2 >= total && header_filled * 2 >= columns.len()).then_some(
        Table {
            rows: grid,
            words: used,
        },
    )
}

fn push_text(cell: &mut String, text: &str) {
    if text.is_empty() {
        return;
    }
    // A line ending in a hyphen continues the word on the next line.
    if !cell.is_empty() && !cell.ends_with('-') {
        cell.push(' ');
    }
    cell.push_str(text);
}

/// Drop one-cell lines after the last full row unless they start right of the
/// first column, where a wrapped cell of the last row would be.
fn trim_trailing<'a>(run: &'a [&'a Row], words: &[Word], first_column_end: f64) -> &'a [&'a Row] {
    let last = run
        .iter()
        .rposition(|r| r.is_candidate())
        .map_or(0, |i| i + 1);
    let wrapped = run[last..]
        .iter()
        .take_while(|r| r.span(words, 0).0 > first_column_end)
        .count();
    &run[..last + wrapped]
}

/// Column extents from the rows with the most cells, so spanning cells do not
/// merge the columns under them.
fn columns(rows: &[&&Row], words: &[Word]) -> Vec<(f64, f64)> {
    let widest = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
    let mut spans: Vec<(f64, f64)> = rows
        .iter()
        .filter(|r| r.cells.len() == widest)
        .flat_map(|r| (0..r.cells.len()).map(move |c| r.span(words, c)))
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut columns: Vec<(f64, f64)> = vec![];
    for (x0, x1) in spans {
        match columns.last_mut() {
            Some(last) if x0 <= last.1 => last.1 = last.1.max(x1),
            _ => columns.push((x0, x1)),
        }
    }
    columns
}

/// Columns a cell overlaps, or the nearest one when it overlaps none.
fn spanned(columns: &[(f64, f64)], x0: f64, x1: f64) -> Vec<usize> {
    let hit: Vec<usize> = (0..columns.len())
        .filter(|&c| x1.min(columns[c].1) - x0.max(columns[c].0) > 0.0)
        .collect();
    if !hit.is_empty() {
        return hit;
    }
    let center = (x0 + x1) / 2.0;
    let distance = |&(a, b): &(f64, f64)| (center - (a + b) / 2.0).abs();
    vec![
        (0..columns.len())
            .min_by(|&a, &b| distance(&columns[a]).total_cmp(&distance(&columns[b])))
            .unwrap_or(0),
    ]
}

/// Page text in Poppler's reading order with each table as a Markdown table,
/// placed where its first line is.
pub fn render_page(page: &Page, tables: &[Table]) -> String {
    let mut out: Vec<String> = vec![];
    let mut emitted = vec![false; tables.len()];
    for block in &page.blocks {
        let mut lines = vec![];
        for line in block {
            match tables
                .iter()
                .position(|t| line.iter().any(|i| t.words.contains(i)))
            {
                Some(t) => {
                    if !lines.is_empty() {
                        out.push(std::mem::take(&mut lines).join("\n"));
                    }
                    if !emitted[t] {
                        emitted[t] = true;
                        out.push(markdown_table(&tables[t].rows));
                    }
                }
                None => lines.push(
                    line.iter()
                        .map(|&i| page.words[i].text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
            }
        }
        if !lines.is_empty() {
            out.push(lines.join("\n"));
        }
    }
    out.join("\n\n") + "\n"
}

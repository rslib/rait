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
/// A gap between rows wider than this multiple of the table's usual gap ends
/// the table, whatever the page's text height.
const PITCH_JUMP: f64 = 2.0;
/// Slack around a model's table box, in points, for words its box clips.
const REGION_MARGIN: f64 = 4.0;
/// How far past a caption's ends, in text heights, its table's columns can reach.
const CAPTION_SLACK: f64 = 2.0;
/// How far, in text heights, a `Table N` caption can sit from its table.
const CAPTION_REACH: f64 = 8.0;
/// Most header cells a real table leaves empty or starts with a digit or `(`,
/// as a share. Plots reach this through axis ticks and subfigure labels.
const MAX_NON_TEXT_HEADER: f64 = 0.3;

#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    /// Cell text; the first row is the header.
    pub rows: Vec<Vec<String>>,
    /// Indexes of the page words inside the table.
    pub words: HashSet<usize>,
}

/// Where detection runs. On a whole page the grid must prove it is a table on
/// its own; inside a region a vision model already found, it only has to be a
/// grid, so prose-like rows and long cells are allowed.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Page,
    Region,
}

/// A table before caption splitting and plausibility checks.
struct Draft {
    rows: Vec<Vec<String>>,
    /// Page words in each cell, parallel to `rows`.
    cell_words: Vec<Vec<Vec<usize>>>,
    columns: Vec<(f64, f64)>,
    top: f64,
    bottom: f64,
}

/// A `Table N` caption line: its horizontal extent and top.
struct Caption {
    x0: f64,
    x1: f64,
    y0: f64,
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

    /// Two or more cells; on a whole page, also not two prose columns.
    fn is_candidate(&self, mode: Mode) -> bool {
        let prose_cells = self
            .cells
            .iter()
            .filter(|cell| cell.len() >= PROSE_WORDS)
            .count();
        self.cells.len() >= 2 && (mode == Mode::Region || prose_cells < 2)
    }
}

pub fn detect(page: &Page) -> Vec<Table> {
    detect_with(page, Mode::Page)
}

/// Tables inside regions a vision model found, as (x0, y0, x1, y1) in PDF
/// points. Each region must still hold a grid of cells to count.
pub fn detect_in(page: &Page, regions: &[[f64; 4]]) -> Vec<Table> {
    let mut kept: Vec<[f64; 4]> = vec![];
    let mut tables = vec![];
    for &region in regions {
        if kept.iter().any(|k| overlap(k, &region) > 0.5) {
            continue;
        }
        kept.push(region);
        let [x0, y0, x1, y1] = region;
        let margin = REGION_MARGIN;
        let inside: Vec<usize> = (0..page.words.len())
            .filter(|&i| {
                let w = &page.words[i];
                let (cx, cy) = ((w.x0 + w.x1) / 2.0, w.center_y());
                cx >= x0 - margin && cx <= x1 + margin && cy >= y0 - margin && cy <= y1 + margin
            })
            .collect();
        let mut index = vec![usize::MAX; page.words.len()];
        for (new, &old) in inside.iter().enumerate() {
            index[old] = new;
        }
        let mut words: Vec<Word> = inside.iter().map(|&i| page.words[i].clone()).collect();
        if sideways(&words) {
            for word in &mut words {
                *word = turn(word, page.height);
            }
        }
        let sub = Page {
            width: page.width,
            height: page.height,
            words,
            blocks: page
                .blocks
                .iter()
                .map(|block| {
                    block
                        .iter()
                        .map(|line| {
                            line.iter()
                                .filter(|&&i| index[i] != usize::MAX)
                                .map(|&i| index[i])
                                .collect::<Vec<_>>()
                        })
                        .filter(|line| !line.is_empty())
                        .collect::<Vec<_>>()
                })
                .filter(|block| !block.is_empty())
                .collect(),
        };
        for table in detect_with(&sub, Mode::Region) {
            tables.push(Table {
                rows: table.rows,
                words: table.words.into_iter().map(|i| inside[i]).collect(),
            });
        }
    }
    tables
}

/// Most words taller than wide: a table turned on its side, as in a
/// landscape table on a portrait page.
fn sideways(words: &[Word]) -> bool {
    let long: Vec<&Word> = words
        .iter()
        .filter(|w| w.text.chars().count() >= 3)
        .collect();
    let tall = long
        .iter()
        .filter(|w| w.y1 - w.y0 > 1.5 * (w.x1 - w.x0))
        .count();
    !long.is_empty() && tall * 2 > long.len()
}

/// A word from text that reads bottom to top, turned to read left to right.
fn turn(word: &Word, page_height: f64) -> Word {
    Word {
        x0: page_height - word.y1,
        x1: page_height - word.y0,
        y0: word.x0,
        y1: word.x1,
        ..word.clone()
    }
}

/// Share of the smaller box covered by the intersection.
fn overlap(a: &[f64; 4], b: &[f64; 4]) -> f64 {
    let w = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let h = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: &[f64; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(f64::EPSILON);
    w * h / area(a).min(area(b))
}

fn detect_with(page: &Page, mode: Mode) -> Vec<Table> {
    let words = &page.words;
    if words.is_empty() {
        return vec![];
    }
    let mut heights: Vec<f64> = words.iter().map(Word::height).collect();
    heights.sort_by(f64::total_cmp);
    let height = heights[heights.len() / 2];
    let left = words.iter().map(|w| w.x0).fold(f64::MAX, f64::min);
    let right = words.iter().map(|w| w.x1).fold(f64::MIN, f64::max);
    let prose = match mode {
        Mode::Page => prose_words(page, (right - left) * PROSE_LINE_WIDTH),
        Mode::Region => HashSet::new(),
    };
    let rows = rows(words, height, &prose);
    let continues = |row: &Row| {
        let (x0, x1) = row.span(words, 0);
        row.cells.len() == 1 && x1 - x0 < WRAP_WIDTH * (right - left)
    };
    // The model already bounds the table, so a region is one run of rows.
    if mode == Mode::Region {
        let run: Vec<&Row> = rows.iter().collect();
        return build(&run, words, mode)
            .map(draft_table)
            .filter(|table| plausible(&table.rows, mode))
            .into_iter()
            .collect();
    }
    let mut drafts = vec![];
    let mut run: Vec<&Row> = vec![];
    for row in &rows {
        let close = run
            .last()
            .is_some_and(|last| row.y0 - last.y1 > ROW_GAP * height);
        if close || !(row.is_candidate(mode) || !run.is_empty() && continues(row)) {
            drafts.extend(build(&run, words, mode));
            run.clear();
        }
        if row.is_candidate(mode) || !run.is_empty() && continues(row) {
            run.push(row);
        }
    }
    drafts.extend(build(&run, words, mode));
    let captions = match mode {
        Mode::Page => captions(page),
        Mode::Region => vec![],
    };
    drafts
        .into_iter()
        .flat_map(|draft| split_by_captions(draft, &captions, height, words))
        .filter(|table| plausible(&table.rows, mode))
        .collect()
}

fn captions(page: &Page) -> Vec<Caption> {
    let words = &page.words;
    page.blocks
        .iter()
        .flatten()
        .filter_map(|line| {
            let (&first, &last) = (line.first()?, line.last()?);
            let label = words[*line.get(1)?].text.trim_end_matches([':', '.']);
            let numbered = !label.is_empty()
                && (label.chars().all(|c| c.is_ascii_digit())
                    || label.chars().all(|c| "IVXLC".contains(c)));
            (matches!(words[first].text.as_str(), "Table" | "TABLE") && numbered).then(|| Caption {
                x0: words[first].x0,
                x1: words[last].x1,
                y0: words[first].y0,
            })
        })
        .collect()
}

/// A grid can take in text from beside its table: a second table set side by
/// side in the other text column, or a heading level with its header. Captions
/// mark where each table is: columns under a caption form that table and the
/// rest form a leftover part that must pass [`plausible`] on its own. Each part
/// is then cut at its own vertical gaps, since rows from the other column can
/// bridge a gap that ends this part's table.
fn split_by_captions(
    draft: Draft,
    captions: &[Caption],
    height: f64,
    words: &[Word],
) -> Vec<Table> {
    let left = draft.columns[0].0;
    let right = draft.columns[draft.columns.len() - 1].1;
    let reach = CAPTION_REACH * height;
    let mut near: Vec<&Caption> = captions
        .iter()
        .filter(|c| c.x1 > left && c.x0 < right)
        .filter(|c| {
            (c.y0 < draft.top && draft.top - c.y0 < reach)
                || (c.y0 > draft.bottom && c.y0 - draft.bottom < reach)
        })
        .collect();
    near.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    if !near.windows(2).all(|p| p[0].x1 < p[1].x0) {
        near.clear();
    }
    let slack = CAPTION_SLACK * height;
    // Part `i` is under caption `i`; the last part is everything else.
    let part_of = |x: f64| {
        near.iter()
            .position(|c| x >= c.x0 - slack && x <= c.x1 + slack)
            .unwrap_or(near.len())
    };
    let mut tables = vec![];
    for part in 0..=near.len() {
        let cols: Vec<usize> = (0..draft.columns.len())
            .filter(|&c| part_of((draft.columns[c].0 + draft.columns[c].1) / 2.0) == part)
            .collect();
        if cols.is_empty() {
            continue;
        }
        let mut segment: Vec<(Vec<String>, Vec<usize>)> = vec![];
        let mut gaps: Vec<f64> = vec![];
        let mut bottom = f64::MIN;
        for (row, ids) in draft.rows.iter().zip(&draft.cell_words) {
            let ids: Vec<usize> = cols.iter().flat_map(|&c| ids[c].iter().copied()).collect();
            if ids.is_empty() {
                continue;
            }
            let top = ids.iter().map(|&i| words[i].y0).fold(f64::MAX, f64::min);
            let gap = top - bottom;
            let usual = median(&gaps);
            let jump = usual.is_some_and(|usual| gap > PITCH_JUMP * usual.max(0.25 * height));
            if !segment.is_empty() && (gap > ROW_GAP * height || jump) {
                tables.push(to_table(std::mem::take(&mut segment)));
                gaps.clear();
            } else if !segment.is_empty() {
                gaps.push(gap);
            }
            bottom = ids.iter().map(|&i| words[i].y1).fold(f64::MIN, f64::max);
            segment.push((cols.iter().map(|&c| row[c].clone()).collect(), ids));
        }
        if !segment.is_empty() {
            tables.push(to_table(segment));
        }
    }
    tables
}

/// The whole draft as one table.
fn draft_table(draft: Draft) -> Table {
    to_table(
        draft
            .rows
            .into_iter()
            .zip(draft.cell_words)
            .map(|(row, ids)| (row, ids.into_iter().flatten().collect()))
            .collect(),
    )
}

fn median(values: &[f64]) -> Option<f64> {
    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied()
}

fn to_table(segment: Vec<(Vec<String>, Vec<usize>)>) -> Table {
    let mut words = HashSet::new();
    let rows = segment
        .into_iter()
        .map(|(row, ids)| {
            words.extend(ids);
            row
        })
        .collect();
    Table { rows, words }
}

/// Final checks that separate tables from grids of other text: a header of
/// words, cells mostly filled, and not an author block of email addresses.
fn plausible(rows: &[Vec<String>], mode: Mode) -> bool {
    let Some(header) = rows.first() else {
        return false;
    };
    let width = header.len();
    let filled = rows.iter().flatten().filter(|c| !c.is_empty()).count();
    let non_text = header
        .iter()
        .map(|c| c.trim())
        .filter(|c| {
            c.is_empty()
                || c.starts_with('(')
                || (c.starts_with(|ch: char| ch.is_ascii_digit()) && !is_year(c))
        })
        .count();
    let emails = rows.iter().any(|row| {
        let count = row
            .iter()
            .filter(|c| c.contains('@') && c.contains('.'))
            .count();
        count >= 2 && count * 2 >= row.len()
    });
    let grid = rows.len() >= 2 && width >= 2 && !emails;
    match mode {
        Mode::Page => {
            grid && filled * 2 >= rows.len() * width
                && (non_text as f64) <= MAX_NON_TEXT_HEADER * width as f64
        }
        Mode::Region => grid && filled * 5 >= rows.len() * width * 2,
    }
}

/// Years are common column headers, unlike other bare numbers.
fn is_year(text: &str) -> bool {
    text.len() == 4
        && (text.starts_with("19") || text.starts_with("20"))
        && text.chars().all(|c| c.is_ascii_digit())
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

fn build(run: &[&Row], words: &[Word], mode: Mode) -> Option<Draft> {
    let candidates: Vec<&&Row> = run.iter().filter(|r| r.is_candidate(mode)).collect();
    let min_rows = if mode == Mode::Region { 2 } else { MIN_ROWS };
    if candidates.len() < min_rows {
        return None;
    }
    let cells: usize = candidates.iter().map(|r| r.cells.len()).sum();
    let words_in: usize = candidates.iter().flat_map(|r| &r.cells).map(Vec::len).sum();
    if mode == Mode::Page && words_in as f64 / cells as f64 > MAX_MEAN_WORDS {
        return None;
    }
    let columns = columns(&candidates, words);
    if columns.len() < 2 {
        return None;
    }
    let run = trim_trailing(run, words, columns[0].1, mode);
    let mut grid: Vec<Vec<String>> = vec![];
    let mut cell_words: Vec<Vec<Vec<usize>>> = vec![];
    for row in run {
        let header = grid.len() <= 1;
        let mut line = vec![String::new(); columns.len()];
        let mut line_words = vec![vec![]; columns.len()];
        for (c, cell) in row.cells.iter().enumerate() {
            let (x0, x1) = row.span(words, c);
            let text = cell
                .iter()
                .map(|&i| words[i].text.as_str())
                .collect::<Vec<_>>();
            // A header cell spanning columns labels each of them; elsewhere a
            // spanning cell fills its leftmost column, like a merged cell.
            let spanned = spanned(&columns, x0, x1);
            if spanned.is_empty() {
                continue;
            }
            let targets = if header {
                spanned
            } else {
                spanned[..1].to_vec()
            };
            for col in targets {
                push_text(&mut line[col], &text.join(" "));
                line_words[col].extend(cell.iter().copied());
            }
        }
        let filled = line.iter().filter(|c| !c.is_empty()).count();
        if filled == 0 {
            continue;
        }
        match grid.last_mut() {
            // An empty first column continues the row above when it is a
            // second header line or a single wrapped cell. With more cells it
            // is a new row under a first-column value spanning several rows.
            Some(previous) if line[0].is_empty() && (header || filled == 1) => {
                for (cell, text) in previous.iter_mut().zip(line) {
                    push_text(cell, &text);
                }
                if let Some(previous) = cell_words.last_mut() {
                    for (cell, ids) in previous.iter_mut().zip(line_words) {
                        cell.extend(ids);
                    }
                }
            }
            _ => {
                grid.push(line);
                cell_words.push(line_words);
            }
        }
    }
    let top = run.iter().map(|r| r.y0).fold(f64::MAX, f64::min);
    let bottom = run.iter().map(|r| r.y1).fold(f64::MIN, f64::max);
    Some(Draft {
        rows: grid,
        cell_words,
        columns,
        top,
        bottom,
    })
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
fn trim_trailing<'a>(
    run: &'a [&'a Row],
    words: &[Word],
    first_column_end: f64,
    mode: Mode,
) -> &'a [&'a Row] {
    let last = run
        .iter()
        .rposition(|r| r.is_candidate(mode))
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

/// Columns a cell overlaps; the nearest one for a cell in a gap between
/// columns; none for a cell wholly beside the table, such as a heading or text
/// from the other text column on the same line.
fn spanned(columns: &[(f64, f64)], x0: f64, x1: f64) -> Vec<usize> {
    let hit: Vec<usize> = (0..columns.len())
        .filter(|&c| x1.min(columns[c].1) - x0.max(columns[c].0) > 0.0)
        .collect();
    if !hit.is_empty() {
        return hit;
    }
    if x1 < columns[0].0 || x0 > columns[columns.len() - 1].1 {
        return vec![];
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

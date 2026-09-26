//! Tables from Table Transformer structure: the model's rows and columns form
//! the grid, and the page words fill its cells.

use crate::layout::{Page, Word};
use crate::tables::{REGION_MARGIN, Table, plausible_region, sideways};
use rait_vision::Label;
use std::collections::HashSet;

/// A structure element in page points: label, box and score.
pub type Element = (Label, [f64; 4], f32);

/// Two rows (or columns) overlapping by more than this share of the smaller
/// one are the same row; the lower score is dropped.
const SAME_LINE: f64 = 0.5;

/// The table inside `region`, or `None` when the structure gives no grid of at
/// least two rows and two columns, or the table is turned sideways.
pub fn table(page: &Page, region: [f64; 4], elements: &[Element]) -> Option<Table> {
    let [x0, y0, x1, y1] = region;
    let m = REGION_MARGIN;
    let inside: Vec<usize> = (0..page.words.len())
        .filter(|&i| {
            let w = &page.words[i];
            let cx = (w.x0 + w.x1) / 2.0;
            cx >= x0 - m && cx <= x1 + m && w.center_y() >= y0 - m && w.center_y() <= y1 + m
        })
        .collect();
    let words: Vec<Word> = inside.iter().map(|&i| page.words[i].clone()).collect();
    if words.is_empty() || sideways(&words) {
        return None;
    }
    let of = |label: Label| elements.iter().filter(move |e| e.0 == label);
    let rows = distinct(of(Label::Row), 1);
    let cols = distinct(of(Label::Column), 0);
    if rows.len() < 2 || cols.len() < 2 {
        return None;
    }
    let spans = |b: &[f64; 4], lines: &[[f64; 4]], axis: usize| -> Vec<usize> {
        (0..lines.len())
            .filter(|&i| {
                let c = (lines[i][axis] + lines[i][axis + 2]) / 2.0;
                c >= b[axis] && c <= b[axis + 2]
            })
            .collect()
    };
    let mut cells: Vec<Vec<Vec<usize>>> = vec![vec![vec![]; cols.len()]; rows.len()];
    let mut placed = vec![false; words.len()];
    let headers: HashSet<usize> = of(Label::ColumnHeader)
        .flat_map(|h| spans(&h.1, &rows, 1))
        .collect();
    let header_rows = (0..rows.len())
        .take_while(|r| headers.contains(r))
        .count()
        .max(1);
    // A spanning cell's words go to its top-left cell, or to every column it
    // covers in the header, so each column name carries the spanning text.
    for span in of(Label::SpanningCell) {
        let (rs, cs) = (spans(&span.1, &rows, 1), spans(&span.1, &cols, 0));
        if rs.len() * cs.len() < 2 {
            continue;
        }
        let ids: Vec<usize> = (0..words.len())
            .filter(|&i| !placed[i] && contains(&span.1, &words[i]))
            .collect();
        let targets = if rs[0] < header_rows {
            cs.clone()
        } else {
            vec![cs[0]]
        };
        for c in targets {
            cells[rs[0]][c].extend(&ids);
        }
        for i in ids {
            placed[i] = true;
        }
    }
    for i in 0..words.len() {
        if !placed[i] {
            let (r, c) = (nearest(&rows, &words[i], 1), nearest(&cols, &words[i], 0));
            cells[r][c].push(i);
        }
    }
    let text = |ids: &Vec<usize>| {
        let mut ids = ids.clone();
        ids.sort_by(|&a, &b| {
            (words[a].line, words[a].x0)
                .partial_cmp(&(words[b].line, words[b].x0))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ids.dedup();
        ids.iter()
            .map(|&i| words[i].text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    };
    let grid: Vec<Vec<String>> = cells
        .iter()
        .map(|row| row.iter().map(text).collect())
        .collect();
    let used: Vec<usize> = (0..cols.len())
        .filter(|&c| grid.iter().any(|row| !row[c].is_empty()))
        .collect();
    let pick = |row: &Vec<String>| used.iter().map(|&c| row[c].clone()).collect::<Vec<_>>();
    let header: Vec<String> = used
        .iter()
        .map(|&c| {
            let parts: Vec<&str> = grid[..header_rows]
                .iter()
                .map(|row| row[c].as_str())
                .filter(|t| !t.is_empty())
                .collect();
            // A spanning header repeats in the rows below it only once.
            let mut joined: Vec<&str> = vec![];
            for p in parts {
                if joined.last() != Some(&p) {
                    joined.push(p);
                }
            }
            joined.join(" ")
        })
        .collect();
    let mut out = vec![header];
    out.extend(
        grid[header_rows..]
            .iter()
            .filter(|row| row.iter().any(|t| !t.is_empty()))
            .map(pick),
    );
    if !plausible_region(&out) {
        return None;
    }
    Some(Table {
        rows: out,
        words: inside.into_iter().collect(),
    })
}

/// Boxes along one axis (1 for rows, 0 for columns), best first, without
/// near-duplicates, then in page order.
fn distinct<'a>(found: impl Iterator<Item = &'a Element>, axis: usize) -> Vec<[f64; 4]> {
    let mut found: Vec<&Element> = found.collect();
    found.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut kept: Vec<[f64; 4]> = vec![];
    for e in found {
        let b = e.1;
        let dup = kept.iter().any(|k| {
            let inter = (k[axis + 2].min(b[axis + 2]) - k[axis].max(b[axis])).max(0.0);
            let smaller = (k[axis + 2] - k[axis])
                .min(b[axis + 2] - b[axis])
                .max(f64::EPSILON);
            inter / smaller > SAME_LINE
        });
        if !dup {
            kept.push(b);
        }
    }
    kept.sort_by(|a, b| (a[axis] + a[axis + 2]).total_cmp(&(b[axis] + b[axis + 2])));
    kept
}

fn contains(b: &[f64; 4], w: &Word) -> bool {
    let (cx, cy) = ((w.x0 + w.x1) / 2.0, w.center_y());
    cx >= b[0] && cx <= b[2] && cy >= b[1] && cy <= b[3]
}

/// The line (row or column) that overlaps the word most along `axis`, or the
/// closest one when none overlaps.
fn nearest(lines: &[[f64; 4]], w: &Word, axis: usize) -> usize {
    let (lo, hi) = if axis == 0 {
        (w.x0, w.x1)
    } else {
        (w.y0, w.y1)
    };
    let centre = (lo + hi) / 2.0;
    let score = |l: &[f64; 4]| {
        let inter = (l[axis + 2].min(hi) - l[axis].max(lo)).max(0.0);
        if inter > 0.0 {
            inter
        } else {
            -(centre - (l[axis] + l[axis + 2]) / 2.0).abs()
        }
    };
    (0..lines.len())
        .max_by(|&a, &b| score(&lines[a]).total_cmp(&score(&lines[b])))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(x0: f64, y0: f64, text: &str, line: usize) -> Word {
        Word {
            x0,
            y0,
            x1: x0 + 20.0,
            y1: y0 + 8.0,
            text: text.into(),
            line,
        }
    }

    #[test]
    fn fills_grid_with_spanning_header() {
        let page = Page {
            width: 200.0,
            height: 200.0,
            words: vec![
                word(60.0, 10.0, "Latency", 0),
                word(10.0, 30.0, "Name", 1),
                word(50.0, 30.0, "p50", 1),
                word(90.0, 30.0, "p99", 1),
                word(10.0, 50.0, "a", 2),
                word(50.0, 50.0, "1", 2),
                word(90.0, 50.0, "2", 2),
                word(10.0, 70.0, "b", 3),
                word(50.0, 70.0, "3", 3),
                word(90.0, 70.0, "4", 3),
            ],
            blocks: vec![],
        };
        let row = |y: f64| (Label::Row, [5.0, y - 2.0, 115.0, y + 10.0], 0.9);
        let col = |x: f64| (Label::Column, [x - 4.0, 5.0, x + 24.0, 85.0], 0.9);
        let mut els = vec![row(10.0), row(30.0), row(50.0), row(70.0), row(50.5)];
        els.extend([col(10.0), col(50.0), col(90.0)]);
        els.push((Label::ColumnHeader, [5.0, 6.0, 115.0, 40.0], 0.9));
        els.push((Label::SpanningCell, [45.0, 8.0, 115.0, 20.0], 0.9));
        let t = table(&page, [0.0, 0.0, 120.0, 90.0], &els).unwrap();
        assert_eq!(t.rows[0], ["Name", "Latency p50", "Latency p99"]);
        assert_eq!(t.rows[1], ["a", "1", "2"]);
        assert_eq!(t.rows.len(), 3);
        assert_eq!(t.words.len(), 10);
    }
}

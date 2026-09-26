//! Figures: the area above each `Figure N:` caption, up to the nearest
//! paragraph, cropped from the rendered page. Works for vector plots as well
//! as embedded images.

use crate::layout::{Page, Word};
use crate::tables;

/// Lines with at least this many words, averaging at least `PROSE_CHARS`
/// characters and spanning `PROSE_WIDTH` of the text width, are text that
/// bounds a figure; a row of axis ticks and labels is not.
const PROSE_WORDS: usize = 6;
const PROSE_CHARS: usize = 3;
const PROSE_WIDTH: f64 = 0.35;
/// A caption line closer than this many text heights (the page's median word
/// height) below the previous one continues the caption. Caption fonts can
/// report short word boxes, so their own height is no measure.
const CAPTION_GAP: f64 = 1.2;
/// Figures shorter than this, in points, are false captions.
const MIN_HEIGHT: f64 = 30.0;
/// The topmost and bottom-most lines, when inside this share of the page from
/// its edge, are the running header and the page number.
const HEADER_ZONE: f64 = 0.15;
/// Share of the page kept as margin where nothing else bounds a figure. A
/// figure may be wider or taller than the text, so the text edges do not.
const PAGE_MARGIN: f64 = 0.04;
/// Points kept around the figure, and kept clear above the caption.
const PAD: f64 = 2.0;
const CAPTION_CLEAR: f64 = 4.0;

/// A figure on one page: its label (`Figure 3`), caption text and box in
/// page points (x0, y0, x1, y1 from the top-left corner).
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub label: String,
    pub caption: String,
    pub bounds: [f64; 4],
}

/// Figures on a page. A caption usually sits below its figure; when the space
/// above it is too small for one, the figure is taken from below it.
pub fn find(page: &Page) -> Vec<Found> {
    let words = &page.words;
    if words.is_empty() {
        return vec![];
    }
    let bbox = |ids: &mut dyn Iterator<Item = usize>| {
        ids.fold([f64::MAX, f64::MAX, f64::MIN, f64::MIN], |b, i| {
            let w = &words[i];
            [
                b[0].min(w.x0),
                b[1].min(w.y0),
                b[2].max(w.x1),
                b[3].max(w.y1),
            ]
        })
    };
    let text_top = words.iter().map(|w| w.y0).fold(f64::MAX, f64::min);
    let text_bottom = words.iter().map(|w| w.y1).fold(f64::MIN, f64::max);
    let text_left = words.iter().map(|w| w.x0).fold(f64::MAX, f64::min);
    let text_right = words.iter().map(|w| w.x1).fold(f64::MIN, f64::max);
    // What bounds a figure from above: prose, tables and the running header.
    let lines: Vec<&Vec<usize>> = page.blocks.iter().flatten().collect();
    let mut prose: Vec<[f64; 4]> = page
        .blocks
        .iter()
        .flatten()
        .filter(|line| {
            let chars: usize = line.iter().map(|&i| words[i].text.chars().count()).sum();
            line.len() >= PROSE_WORDS && chars >= PROSE_CHARS * line.len()
        })
        .map(|line| bbox(&mut line.iter().copied()))
        .filter(|b| b[2] - b[0] >= PROSE_WIDTH * (text_right - text_left))
        .collect();
    prose.extend(
        tables::detect(page)
            .iter()
            .map(|t| bbox(&mut t.words.iter().copied())),
    );
    prose.extend(
        page.blocks
            .iter()
            .flatten()
            .map(|line| bbox(&mut line.iter().copied()))
            .filter(|b| {
                let header = (b[1] - text_top).abs() < 1.0 && b[3] < HEADER_ZONE * page.height;
                let footer =
                    (b[3] - text_bottom).abs() < 1.0 && b[1] > (1.0 - HEADER_ZONE) * page.height;
                header || footer
            }),
    );
    let captions: Vec<[f64; 4]> = lines
        .iter()
        .filter(|line| label_of(words, line).is_some())
        .map(|line| bbox(&mut line.iter().copied()))
        .collect();
    let mid = (text_left + text_right) / 2.0;
    let mut heights: Vec<f64> = words.iter().map(Word::height).collect();
    heights.sort_by(f64::total_cmp);
    let text_height = heights[heights.len() / 2];
    let mut found = vec![];
    // A caption opens its block, or follows a short line such as axis ticks
    // that Poppler put in the same block; not a line inside a paragraph.
    let openers: Vec<&Vec<usize>> = page
        .blocks
        .iter()
        .flat_map(|b| {
            b.iter()
                .enumerate()
                .filter(|(i, _)| *i == 0 || b[i - 1].len() <= 3)
                .map(|(_, l)| l)
        })
        .collect();
    for first in &openers {
        let Some(label) = label(&first.iter().map(|&i| &words[i]).collect::<Vec<_>>()) else {
            continue;
        };
        // Poppler may split one caption into several blocks; take the lines
        // that follow closely below and overlap it.
        let mut caption_lines = vec![*first];
        loop {
            let prev = bbox(&mut caption_lines.last().unwrap().iter().copied());
            let next = lines.iter().find(|line| {
                let next = bbox(&mut line.iter().copied());
                next[1] > prev[3] - 0.5 * text_height
                    && next[1] > prev[1]
                    && next[1] - prev[3] < CAPTION_GAP * text_height
                    && next[0] < prev[2]
                    && next[2] > prev[0]
                    && label_of(words, line).is_none()
            });
            match next {
                Some(line) => caption_lines.push(line),
                None => break,
            }
        }
        let caption_box = bbox(&mut caption_lines.iter().flat_map(|l| l.iter()).copied());
        let caption = caption_lines
            .iter()
            .flat_map(|l| l.iter())
            .map(|&i| words[i].text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let spans = caption_box[0] < mid - 10.0 && caption_box[2] > mid + 10.0;
        let (x0, x1) = if spans {
            (PAGE_MARGIN * page.width, (1.0 - PAGE_MARGIN) * page.width)
        } else {
            let left = (caption_box[0] + caption_box[2]) / 2.0 < mid;
            let side: Vec<&[f64; 4]> = prose
                .iter()
                .filter(|p| {
                    ((p[0] + p[2]) / 2.0 < mid) == left
                        && (p[2] - p[0]) < 0.6 * (text_right - text_left)
                })
                .collect();
            if side.is_empty() {
                (caption_box[0], caption_box[2])
            } else {
                (
                    side.iter().map(|p| p[0]).fold(f64::MAX, f64::min),
                    side.iter().map(|p| p[2]).fold(f64::MIN, f64::max),
                )
            }
        };
        let across = |p: &&[f64; 4]| p[2] > x0 && p[0] < x1;
        let bottom = caption_box[1];
        let top = prose
            .iter()
            .chain(found.iter().map(|f: &Found| &f.bounds))
            .filter(across)
            .filter(|p| p[3] <= bottom)
            .map(|p| p[3])
            .fold(PAGE_MARGIN * page.height, f64::max);
        let above = [x0 - PAD, top + PAD, x1 + PAD, bottom - CAPTION_CLEAR];
        let below_top = caption_box[3];
        let below_bottom = prose
            .iter()
            .chain(&captions)
            .filter(across)
            .filter(|p| p[1] >= below_top)
            .map(|p| p[1])
            .fold((1.0 - PAGE_MARGIN) * page.height, f64::min);
        let below = [
            x0 - PAD,
            below_top + CAPTION_CLEAR,
            x1 + PAD,
            below_bottom - PAD,
        ];
        let holds_text = |b: &[f64; 4]| {
            words.iter().any(|w| {
                let (cx, cy) = ((w.x0 + w.x1) / 2.0, w.center_y());
                cx > b[0] && cx < b[2] && cy > b[1] && cy < b[3]
            })
        };
        let tall = |b: &[f64; 4]| b[3] - b[1] >= MIN_HEIGHT;
        // Below only when the space above is too small or blank and the space
        // below holds something.
        // ponytail: a figure below its caption ends at the next prose, table
        // or caption, so a short heading right under it is included.
        let bounds = if tall(&above) && (holds_text(&above) || !tall(&below) || !holds_text(&below))
        {
            above
        } else if tall(&below) {
            below
        } else {
            continue;
        };
        found.push(Found {
            label,
            caption,
            bounds,
        });
    }
    found
}

fn label_of(words: &[Word], line: &[usize]) -> Option<String> {
    label(&line.iter().map(|&i| &words[i]).collect::<Vec<_>>())
}

/// `Figure 3` from a caption line that opens `Figure 3:`, `Fig. 3.` or
/// `FIGURE 3.`; a sentence such as `Figure 3 shows` is not a caption.
fn label(line: &[&Word]) -> Option<String> {
    let first = line.first()?.text.as_str();
    if !matches!(first, "Figure" | "FIGURE" | "Fig." | "FIG.") {
        return None;
    }
    let number = line.get(1)?.text.as_str();
    let bare = number.trim_end_matches([':', '.', '|']);
    let marked = number.len() > bare.len()
        || line
            .get(2)
            .is_some_and(|w| matches!(w.text.as_str(), ":" | "." | "|"));
    let numeric = !bare.is_empty()
        && bare.starts_with(|c: char| c.is_ascii_digit())
        && bare.chars().all(|c| c.is_ascii_alphanumeric() || c == '.');
    (marked && numeric).then(|| format!("Figure {bare}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(words: &mut Vec<Word>, y: f64, x: f64, text: &str) -> Vec<usize> {
        let mut ids = vec![];
        for (i, t) in text.split(' ').enumerate() {
            ids.push(words.len());
            words.push(Word {
                x0: x + i as f64 * 30.0,
                y0: y,
                x1: x + i as f64 * 30.0 + 25.0,
                y1: y + 9.0,
                text: t.into(),
                line: words.len(),
            });
        }
        ids
    }

    #[test]
    fn crops_between_paragraph_and_caption() {
        let mut words = vec![];
        let para = line(
            &mut words,
            100.0,
            50.0,
            "some prose words that fill a line of text",
        );
        let axis = line(&mut words, 200.0, 80.0, "0 10 20");
        let cap = line(&mut words, 300.0, 50.0, "Figure 2: Latency over time.");
        let refer = line(
            &mut words,
            400.0,
            50.0,
            "Figure 2 shows that latency drops over time here",
        );
        let page = Page {
            width: 600.0,
            height: 800.0,
            words,
            blocks: vec![vec![para], vec![axis], vec![cap], vec![refer]],
        };
        let found = find(&page);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "Figure 2");
        assert_eq!(found[0].caption, "Figure 2: Latency over time.");
        let [_, y0, _, y1] = found[0].bounds;
        assert_eq!((y0, y1), (109.0 + PAD, 300.0 - CAPTION_CLEAR));
    }
}

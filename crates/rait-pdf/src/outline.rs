//! A PDF's outline: its bookmarks, or headings found from text size and
//! section numbers when it has none.

use crate::layout::{Page, Word};
use serde::Serialize;
use std::collections::HashMap;

/// Lines at least this many times the body text height are headings.
const HEADING_SCALE: f64 = 1.15;
/// Headings are short.
const MAX_HEADING_WORDS: usize = 12;
const MAX_NUMBERED_WORDS: usize = 10;
/// A line repeated on more pages than this is a running header.
const MAX_REPEATS: usize = 2;
/// Lines with at least this many words are prose, which sets the body size.
const PROSE_WORDS: usize = 6;
/// A paper with this many numbered headings is numbered: its unnumbered large
/// lines must be at least as large as those headings to count.
const NUMBERED_PAPER: usize = 3;
/// More large lines than this of one size on one page are table cells or a
/// list of names, not headings.
const MAX_LARGE_PER_PAGE: usize = 3;
/// First words of figure and table captions.
const CAPTIONS: &[&str] = &[
    "Figure",
    "FIGURE",
    "Fig.",
    "Table",
    "TABLE",
    "Listing",
    "Algorithm",
];
/// Unnumbered headings that are headings at any size.
const NAMED: &[&str] = &[
    "abstract",
    "introduction",
    "related work",
    "background",
    "conclusion",
    "conclusions",
    "discussion",
    "acknowledgments",
    "acknowledgements",
    "acknowledgment",
    "references",
    "bibliography",
    "appendix",
    "contents",
];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OutlineEntry {
    pub level: u32,
    pub title: String,
    pub page: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutlineSource {
    Bookmarks,
    Headings,
}

enum Kind {
    Numbered(u32),
    Named,
    Large,
}

struct Line {
    page: u32,
    text: String,
    height: f64,
    kind: Kind,
}

/// A section found by name: its outline entry, its pages and the title of the
/// entry that ends it.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    pub first: u32,
    pub last: u32,
    pub next: Option<String>,
}

/// Letters and digits only, lower case: `I NTRODUCTION` and `1 Introduction`
/// both hold `introduction`.
pub fn compact(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// The entry named `name`, running to the next entry at the same or a higher
/// level. Exact titles win over titles that only contain `name`, and a
/// number such as `3.1` matches a numbered title.
pub fn section(entries: &[OutlineEntry], name: &str, pages: u32) -> Option<Section> {
    let want = compact(name);
    if want.is_empty() {
        return None;
    }
    let bare = |t: &str| compact(unnumbered(t));
    let found = entries
        .iter()
        .position(|e| bare(&e.title) == want || compact(&e.title) == want)
        .or_else(|| {
            entries
                .iter()
                .position(|e| compact(&e.title).starts_with(&want))
        })
        .or_else(|| {
            entries
                .iter()
                .position(|e| compact(&e.title).contains(&want))
        })?;
    let entry = &entries[found];
    let next = entries[found + 1..].iter().find(|e| e.level <= entry.level);
    Some(Section {
        title: entry.title.clone(),
        first: entry.page,
        last: next.map_or(pages, |n| n.page.max(entry.page)),
        next: next.map(|n| n.title.clone()),
    })
}

/// `title` without a leading section number such as `2.1`, `IV.` or `A.`.
fn unnumbered(title: &str) -> &str {
    let title = title.trim_start();
    let Some((first, rest)) = title.split_once(' ') else {
        return title;
    };
    let label = first.trim_end_matches('.');
    let numeric = !label.is_empty()
        && label
            .split('.')
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()));
    let roman = !label.is_empty() && label.chars().all(|c| "IVX".contains(c));
    let letter =
        first.len() == 2 && first.ends_with('.') && label.chars().all(|c| c.is_ascii_uppercase());
    if numeric || roman || letter {
        rest.trim_start()
    } else {
        title
    }
}

/// Index of the line that is the heading `title`: its letters and digits end
/// with the title's, with at most a section number before them.
pub fn heading_line(lines: &[&str], title: &str) -> Option<usize> {
    let want = compact(unnumbered(title));
    if want.is_empty() {
        return None;
    }
    let i = lines.iter().position(|line| {
        let have = compact(line);
        have.ends_with(&want) && have.len() <= want.len() + 8
    })?;
    // Poppler may put the section number on its own line just before, with a
    // blank line between.
    let prev = (0..i).rev().find(|&j| !lines[j].trim().is_empty());
    let number =
        prev.filter(|&j| i - j <= 2 && unnumbered(&format!("{} x", lines[j].trim())) == "x");
    Some(number.unwrap_or(i))
}

/// Bookmarks worth trusting: at least two, not mostly page labels such as
/// `Page 3` or bare numbers, and not all on one page of a longer PDF.
pub fn useful(bookmarks: &[OutlineEntry], pages: u32) -> bool {
    let labels = bookmarks
        .iter()
        .filter(|b| {
            let t = b.title.trim().to_lowercase();
            let t = t.strip_prefix("page").unwrap_or(&t).trim();
            t.chars()
                .all(|c| c.is_ascii_digit() || c.is_whitespace() || c == '.')
        })
        .count();
    let one_page = bookmarks.windows(2).all(|w| w[0].page == w[1].page);
    bookmarks.len() >= 2 && labels * 2 < bookmarks.len() && !(one_page && pages > 1)
}

/// Headings in `layouts`, which start at page `first`: short lines that open
/// with a section number such as `3.2`, standard section names, and lines in
/// larger text than the body. Numbered headings take their level from the
/// number, the others from their size, largest first.
pub fn headings(first: u32, layouts: &[Page]) -> Vec<OutlineEntry> {
    let Some(body) = body_height(layouts) else {
        return vec![];
    };
    let mut lines: Vec<Line> = vec![];
    for (page, layout) in (first..).zip(layouts) {
        let blocks = attach_numbers(layout);
        for block in &blocks {
            let mut previous: Option<usize> = None;
            for (i, line) in block.iter().enumerate() {
                let words: Vec<&Word> = line.iter().map(|&w| &layout.words[w]).collect();
                let height = median(words.iter().map(|w| w.height()));
                let text = join(&words);
                let letters = text.chars().filter(|c| c.is_alphabetic()).count();
                let upright = words
                    .iter()
                    .filter(|w| w.height() > 1.5 * (w.x1 - w.x0))
                    .count()
                    * 2
                    <= words.len();
                if letters * 2 < text.chars().filter(|c| !c.is_whitespace()).count() || !upright {
                    previous = None;
                    continue;
                }
                let bare = text.trim_end_matches([':', '.']).to_lowercase();
                // A heading stands alone: a line that runs on into a paragraph
                // in the same block is a run-in heading or prose.
                let alone = block.get(i + 1).is_none_or(|next| {
                    (median(next.iter().map(|&w| layout.words[w].height())) - height).abs() >= 0.5
                });
                if !alone || !title_text(&text) {
                    previous = None;
                    continue;
                }
                let kind = if i > 0 {
                    None
                } else if let Some(level) = numbered(&words) {
                    Some(Kind::Numbered(level))
                } else if NAMED.contains(&bare.as_str()) {
                    Some(Kind::Named)
                } else {
                    None
                };
                let large = height >= HEADING_SCALE * body && words.len() <= MAX_HEADING_WORDS;
                // A large heading wrapped over two lines is one heading.
                if kind.is_none()
                    && large
                    && let Some(p) = previous
                    && (lines[p].height - height).abs() < 0.5
                {
                    lines[p].text.push(' ');
                    lines[p].text.push_str(&text);
                    previous = None;
                    continue;
                }
                let kind = match kind {
                    Some(kind) => kind,
                    None if large && i == 0 => Kind::Large,
                    None => {
                        previous = None;
                        continue;
                    }
                };
                previous = matches!(kind, Kind::Large).then_some(lines.len());
                lines.push(Line {
                    page,
                    text,
                    height,
                    kind,
                });
            }
        }
    }
    let mut pages_of: HashMap<String, usize> = HashMap::new();
    for line in &lines {
        *pages_of.entry(line.text.clone()).or_default() += 1;
    }
    lines.retain(|l| pages_of[&l.text] <= MAX_REPEATS);
    let mut per_page: HashMap<(u32, i64), usize> = HashMap::new();
    for l in lines.iter().filter(|l| matches!(l.kind, Kind::Large)) {
        *per_page
            .entry((l.page, (l.height * 2.0).round() as i64))
            .or_default() += 1;
    }
    lines.retain(|l| {
        !matches!(l.kind, Kind::Large)
            || per_page[&(l.page, (l.height * 2.0).round() as i64)] <= MAX_LARGE_PER_PAGE
    });
    let numbered: Vec<f64> = lines
        .iter()
        .filter(|l| matches!(l.kind, Kind::Numbered(_)))
        .map(|l| l.height)
        .collect();
    if numbered.len() >= NUMBERED_PAPER {
        let section = numbered.iter().copied().fold(f64::MIN, f64::max) - 0.25;
        lines.retain(|l| !matches!(l.kind, Kind::Large) || l.height >= section);
    }
    let size = |h: f64| (h * 2.0).round() / 2.0;
    let mut sizes: Vec<f64> = lines
        .iter()
        .filter(|l| matches!(l.kind, Kind::Large))
        .map(|l| size(l.height))
        .collect();
    sizes.sort_by(|a, b| b.total_cmp(a));
    sizes.dedup();
    lines
        .into_iter()
        .map(|line| OutlineEntry {
            level: match line.kind {
                Kind::Numbered(level) => level,
                Kind::Named => 1,
                Kind::Large => {
                    sizes
                        .iter()
                        .position(|&s| s == size(line.height))
                        .unwrap_or(0) as u32
                        + 1
                }
            },
            title: line.text,
            page: line.page,
        })
        .collect()
}

/// Text that can be a heading: not a caption, a link or a sentence, and it
/// starts like a title.
fn title_text(text: &str) -> bool {
    let first = text.split(' ').next().unwrap_or("");
    let caption = CAPTIONS.contains(&first)
        && text
            .split(' ')
            .nth(1)
            .is_some_and(|w| w.starts_with(|c: char| c.is_ascii_digit() || "IVX".contains(c)));
    let sentence = text.char_indices().any(|(i, c)| {
        ".?!".contains(c)
            && text[i + 1..].starts_with(' ')
            && text[i + 1..].trim_start().starts_with(char::is_uppercase)
    }) && !first.ends_with('.');
    !caption
        && !sentence
        && !text.contains("://")
        && !text.contains('@')
        && !text.ends_with([',', ';', '-'])
        && text.starts_with(|c: char| c.is_uppercase() || c.is_ascii_digit())
}

/// Blocks with each lone section number (LaTeX sets `1` and `Introduction` a
/// quad apart, and Poppler splits them) moved onto the line beside it.
fn attach_numbers(layout: &Page) -> Vec<Vec<Vec<usize>>> {
    let words = &layout.words;
    let lone = |line: &Vec<usize>| {
        line.len() == 1 && {
            let t = words[line[0]].text.trim_end_matches('.');
            !t.is_empty()
                && t.split('.')
                    .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
        }
    };
    let mut blocks = layout.blocks.clone();
    let mut moved = vec![];
    for (b, block) in layout.blocks.iter().enumerate() {
        if block.len() != 1 || !lone(&block[0]) {
            continue;
        }
        let n = &words[block[0][0]];
        let target = layout.blocks.iter().enumerate().find(|(o, other)| {
            *o != b
                && other.first().and_then(|l| l.first()).is_some_and(|&i| {
                    let w = &words[i];
                    (w.center_y() - n.center_y()).abs() < 0.3 * n.height()
                        && w.x0 > n.x1
                        && w.x0 - n.x1 < 4.0 * n.height()
                })
        });
        if let Some((o, _)) = target {
            blocks[o][0].insert(0, block[0][0]);
            moved.push(b);
        }
    }
    for b in moved.into_iter().rev() {
        blocks.remove(b);
    }
    blocks
}

/// The text size that most characters of prose lines use.
fn body_height(layouts: &[Page]) -> Option<f64> {
    let mut chars: HashMap<i64, usize> = HashMap::new();
    for layout in layouts {
        for line in layout
            .blocks
            .iter()
            .flatten()
            .filter(|l| l.len() >= PROSE_WORDS)
        {
            for &w in line {
                let word = &layout.words[w];
                *chars
                    .entry((word.height() * 4.0).round() as i64)
                    .or_default() += word.text.len();
            }
        }
    }
    chars
        .into_iter()
        .max_by_key(|&(h, n)| (n, -h))
        .map(|(h, _)| h as f64 / 4.0)
}

fn median(values: impl Iterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = values.collect();
    v.sort_by(f64::total_cmp);
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

/// Words joined with spaces, except where the PDF split one word into two, as
/// small capitals do (`I NTRODUCTION`).
fn join(words: &[&Word]) -> String {
    let mut text = String::new();
    for (i, w) in words.iter().enumerate() {
        if i > 0 && w.x0 - words[i - 1].x1 > 0.15 * w.height() {
            text.push(' ');
        }
        text.push_str(&w.text);
    }
    text
}

/// The level of a line that opens with a section number (`2`, `2.1.`, `IV.`,
/// `A.`) followed by a short title starting with a capital, like `3.1 Method`.
/// The number must be full size, not a superscript footnote mark.
fn numbered(words: &[&Word]) -> Option<u32> {
    let (first, rest) = words.split_first()?;
    let label = first.text.trim_end_matches('.');
    let parts: Vec<&str> = label.split('.').collect();
    let arabic = parts.len() <= 4
        && parts.iter().all(|p| {
            (1..=2).contains(&p.len())
                && !p.starts_with('0')
                && p.chars().all(|c| c.is_ascii_digit())
        });
    let roman = parts.len() == 1
        && (1..=4).contains(&label.len())
        && label.chars().all(|c| "IVX".contains(c));
    let letter = first.text.len() == 2
        && first.text.ends_with('.')
        && label.chars().all(|c| c.is_ascii_uppercase());
    let last = rest.last()?.text.as_str();
    let title_like = (1..=MAX_NUMBERED_WORDS).contains(&rest.len())
        && rest[0].text.starts_with(|c: char| c.is_uppercase())
        && !last.ends_with(['.', ',', ';', ':'])
        && first.height() >= 0.8 * median(rest.iter().map(|w| w.height()));
    if !title_like {
        return None;
    }
    if arabic {
        Some(parts.len() as u32)
    } else if roman {
        Some(1)
    } else if letter {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(lines: &[(&str, f64)]) -> Page {
        let mut words = vec![];
        let mut blocks = vec![];
        let mut y = 0.0;
        for (n, (text, height)) in lines.iter().enumerate() {
            let mut line = vec![];
            for (i, w) in text.split(' ').enumerate() {
                line.push(words.len());
                words.push(Word {
                    x0: i as f64 * 30.0,
                    y0: y,
                    x1: i as f64 * 30.0 + 25.0,
                    y1: y + height,
                    text: w.into(),
                    line: n,
                });
            }
            y += height + 4.0;
            blocks.push(vec![line]);
        }
        Page {
            width: 600.0,
            height: 800.0,
            words,
            blocks,
        }
    }

    #[test]
    fn section_runs_to_next_entry_at_same_level() {
        let entry = |level, title: &str, page| OutlineEntry {
            level,
            title: title.into(),
            page,
        };
        let entries = [
            entry(1, "1 Introduction", 1),
            entry(1, "2 Method", 2),
            entry(2, "2.1 Setup", 3),
            entry(1, "3 Results", 5),
        ];
        let method = section(&entries, "method", 9).unwrap();
        assert_eq!((method.first, method.last), (2, 5));
        assert_eq!(method.next.as_deref(), Some("3 Results"));
        let setup = section(&entries, "2.1", 9).unwrap();
        assert_eq!((setup.title.as_str(), setup.last), ("2.1 Setup", 5));
        assert_eq!(section(&entries, "Results", 9).unwrap().last, 9);
        assert!(section(&entries, "Discussion", 9).is_none());
        let lines = ["text", "I. I NTRODUCTION", "more"];
        assert_eq!(heading_line(&lines, "Introduction"), Some(1));
        assert_eq!(
            heading_line(&["text", "1.1", "Setup"], "2.1 Setup"),
            Some(1)
        );
        assert_eq!(unnumbered("IV. Validation"), "Validation");
        assert_eq!(unnumbered("Validation"), "Validation");
        assert_eq!(unnumbered("2.1 Setup"), "Setup");
    }

    #[test]
    fn page_label_bookmarks_are_not_useful() {
        let entry = |title: &str, page| OutlineEntry {
            level: 1,
            title: title.into(),
            page,
        };
        let labels = [entry("Page 1", 1), entry("Page 2", 2), entry("3", 3)];
        assert!(!useful(&labels, 3));
        assert!(!useful(&[entry("Title", 1)], 3));
        assert!(!useful(&[entry("Title", 1), entry("Abstract", 1)], 9));
        assert!(useful(&[entry("Intro", 1), entry("Method", 2)], 9));
    }

    #[test]
    fn finds_large_and_numbered_headings() {
        let body = "the quick brown fox jumps over the lazy dog again and again";
        let layout = page(&[
            ("A Study of Things", 18.0),
            (body, 10.0),
            ("1 Introduction", 10.0),
            (body, 10.0),
            ("2.1 Method details", 10.0),
            (body, 10.0),
            ("1 We propose a method.", 10.0),
            (body, 10.0),
        ]);
        let found = headings(3, &[layout]);
        let got: Vec<(u32, &str, u32)> = found
            .iter()
            .map(|e| (e.level, e.title.as_str(), e.page))
            .collect();
        assert_eq!(
            got,
            [
                (1, "A Study of Things", 3),
                (1, "1 Introduction", 3),
                (2, "2.1 Method details", 3)
            ]
        );
    }
}

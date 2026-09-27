//! A paper's bibliography: entries split by their labels (`[12]`, `12.`) or
//! hanging indent, each parsed into authors, title, year and identifiers.

use crate::layout::{Page, Word};
use crate::outline::compact;
use serde::Serialize;

/// Headings that open a bibliography, compared as letters only.
pub const HEADINGS: &[&str] = &[
    "references",
    "bibliography",
    "literaturecited",
    "workscited",
];
/// A labelled style needs at least this many labelled lines.
const MIN_LABELS: usize = 3;
/// The topmost and bottom-most lines of a page within this share of its
/// height from the edge, set apart by a gap of `EDGE_GAP` line heights, are
/// the running header and page number.
const EDGE: f64 = 0.12;
const EDGE_GAP: f64 = 1.8;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reference {
    /// The entry's number: its label when numbered, else its position.
    pub n: u32,
    pub authors: Option<String>,
    pub title: Option<String>,
    pub year: Option<String>,
    /// `doi:…`, `arXiv:…` or a URL, in that order of preference.
    pub id: Option<String>,
    pub text: String,
}

struct Line {
    text: String,
    x0: f64,
    right: bool,
    block: usize,
    page: usize,
}

/// Entries in the reference lines of `layouts`, from the line after the
/// heading `start` on the first page up to, not including, the heading `end`
/// on the last page.
pub fn parse(layouts: &[Page], start: Option<&str>, end: Option<&str>) -> Vec<Reference> {
    let mut lines = vec![];
    let mut block = 0;
    for (page, layout) in layouts.iter().enumerate() {
        let mid = layout.width / 2.0;
        for b in &layout.blocks {
            block += 1;
            for line in b {
                let words: Vec<&Word> = line.iter().map(|&i| &layout.words[i]).collect();
                let Some(first) = words.first() else { continue };
                if edge_line(layout, first) {
                    continue;
                }
                let text = words
                    .iter()
                    .map(|w| w.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                lines.push(Line {
                    text,
                    x0: first.x0,
                    right: first.x0 > mid - 10.0,
                    block,
                    page,
                });
            }
        }
    }
    let is_heading = |l: &Line, name: &str| {
        let have = compact(&l.text);
        have.ends_with(&compact(name)) && have.len() <= compact(name).len() + 8
    };
    let from = start
        .and_then(|s| lines.iter().position(|l| l.page == 0 && is_heading(l, s)))
        .map_or(0, |i| i + 1);
    let last_page = layouts.len().saturating_sub(1);
    let to = end
        .and_then(|e| {
            lines
                .iter()
                .rposition(|l| l.page == last_page && is_heading(l, e))
        })
        .unwrap_or(lines.len());
    let lines = &lines[from.min(to)..to];
    let starts = entry_starts(lines);
    let mut refs = vec![];
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(lines.len());
        let text = join_lines(lines[s..end].iter().map(|l| l.text.as_str()));
        refs.push(entry(refs.len() as u32 + 1, &text));
    }
    refs
}

fn edge_line(page: &Page, word: &Word) -> bool {
    let gap = EDGE_GAP * word.height();
    let top = page.words.iter().map(|w| w.y0).fold(f64::MAX, f64::min);
    let bottom = page.words.iter().map(|w| w.y1).fold(f64::MIN, f64::max);
    // Set apart from the body, and with a body on the other side of the gap.
    let header = (word.y0 - top).abs() < 1.0
        && word.y1 < EDGE * page.height
        && page
            .words
            .iter()
            .all(|w| w.y0 <= word.y1 || w.y0 - word.y1 >= gap)
        && page.words.iter().any(|w| w.y0 - word.y1 >= gap);
    let footer = (word.y1 - bottom).abs() < 1.0
        && word.y0 > (1.0 - EDGE) * page.height
        && page
            .words
            .iter()
            .all(|w| w.y1 >= word.y0 || word.y0 - w.y1 >= gap)
        && page.words.iter().any(|w| word.y0 - w.y1 >= gap);
    header || footer
}

/// Where each entry begins: at labelled lines when the style has labels,
/// else at lines set out to the left of indented continuation lines, else at
/// each text block.
fn entry_starts(lines: &[Line]) -> Vec<usize> {
    // Labels count up, maybe skipping one the PDF split oddly; a line that
    // starts with `2008.` inside an entry is not one.
    let mut labelled = vec![];
    let mut next: Option<u32> = None;
    for (i, line) in lines.iter().enumerate() {
        if let Some(n) = label(&line.text)
            && next.is_none_or(|want| (want..want + 3).contains(&n))
        {
            labelled.push(i);
            next = Some(n + 1);
        }
    }
    if labelled.len() >= MIN_LABELS {
        return labelled;
    }
    let mut starts = vec![];
    for (i, line) in lines.iter().enumerate() {
        let group: Vec<&Line> = lines
            .iter()
            .filter(|l| l.page == line.page && l.right == line.right)
            .collect();
        let left = group.iter().map(|l| l.x0).fold(f64::MAX, f64::min);
        let hanging = group.iter().any(|l| l.x0 > left + 4.0);
        let new_block = i == 0 || lines[i - 1].block != line.block;
        let start = if hanging {
            line.x0 < left + 2.0
        } else {
            new_block
        };
        if start && !line.text.trim().is_empty() {
            starts.push(i);
        }
    }
    starts
}

/// The number of a line that opens with `[12]` or `12.`.
fn label(text: &str) -> Option<u32> {
    let first = text.split_whitespace().next()?;
    let inner = first
        .strip_prefix('[')
        .and_then(|t| t.strip_suffix(']'))
        .or_else(|| first.strip_suffix('.'))?;
    // A year such as `2021.` is not a label.
    inner.parse().ok().filter(|n| *n < 1000)
}

/// Lines joined with spaces, rejoining words and URLs broken across a line:
/// `stor-` + `age` is `storage`, `High-` + `Performance` keeps its hyphen,
/// and a URL ending in `/` runs on.
fn join_lines<'a>(lines: impl Iterator<Item = &'a str>) -> String {
    let mut text = String::new();
    for line in lines {
        let line = line.trim();
        let last = text.rsplit(' ').next().unwrap_or("");
        let url = last.contains("://") && last.ends_with(['/', '-', '.', '_', '=', '?', '&', '#']);
        let hyphen = text.ends_with('-')
            && !text.ends_with("--")
            && !last.chars().all(|c| c.is_ascii_digit() || c == '-');
        let joined = url || hyphen && line.starts_with(|c: char| c.is_uppercase());
        if hyphen && line.starts_with(|c: char| c.is_lowercase()) {
            text.pop();
        } else if !joined && !text.is_empty() {
            text.push(' ');
        }
        text.push_str(line);
    }
    text
}

fn entry(position: u32, text: &str) -> Reference {
    let n = label(text).unwrap_or(position);
    let body = match text.split_once(char::is_whitespace) {
        Some((first, rest)) if label(first).is_some() => rest.trim(),
        _ => text.trim(),
    };
    let doi = find_after(body, "10.", |id| {
        id.split_once('/').is_some_and(|(prefix, _)| {
            prefix[3..].chars().all(|c| c.is_ascii_digit()) && prefix.len() >= 7
        })
    });
    let arxiv = find_arxiv(body);
    let url = body
        .split_whitespace()
        .find(|w| w.starts_with("http://") || w.starts_with("https://"))
        .map(|w| w.trim_end_matches(['.', ',', ';', ')']).to_string());
    let id = doi
        .map(|d| format!("doi:{d}"))
        .or(arxiv.map(|a| format!("arXiv:{a}")))
        .or(url);
    let (authors, title) = authors_title(body);
    Reference {
        n,
        authors,
        title,
        year: year(body),
        id,
        text: text.trim().to_string(),
    }
}

/// The first whitespace-free token starting with `prefix` that passes `ok`,
/// without trailing punctuation.
fn find_after(text: &str, prefix: &str, ok: impl Fn(&str) -> bool) -> Option<String> {
    text.match_indices(prefix).find_map(|(i, _)| {
        let token: String = text[i..]
            .chars()
            .take_while(|c| !c.is_whitespace())
            .collect();
        let token = token
            .trim_end_matches(['.', ',', ';', ')', ']'])
            .to_string();
        ok(&token).then_some(token)
    })
}

fn find_arxiv(text: &str) -> Option<String> {
    for marker in [
        "arXiv:",
        "arxiv:",
        "arxiv.org/abs/",
        "arXiv.org/abs/",
        "arXiv ",
    ] {
        if let Some(id) = find_after(text, marker, |t| {
            let id = &t[marker.len()..];
            id.len() >= 9
                && id.chars().next().is_some_and(|c| c.is_ascii_digit())
                && id.contains('.')
        }) {
            return Some(id[marker.len()..].to_string());
        }
    }
    None
}

/// The first year from 1900 to 2099, as in `2020`, `(2020)` or `2020a`.
fn year(text: &str) -> Option<String> {
    let chars: Vec<char> = text.chars().collect();
    (0..chars.len().saturating_sub(3)).find_map(|i| {
        let digits = &chars[i..i + 4];
        let before = i.checked_sub(1).map(|j| chars[j]);
        let after = chars.get(i + 4).copied();
        let century = matches!((digits[0], digits[1]), ('1', '9') | ('2', '0'));
        let bounded = before.is_none_or(|c| !c.is_alphanumeric() && c != '.' && c != '/')
            && after.is_none_or(|c| {
                !c.is_ascii_digit() && c != '.'
                    || c == '.' && chars.get(i + 5).is_none_or(|c| c.is_whitespace())
            });
        (century && digits.iter().all(char::is_ascii_digit) && bounded)
            .then(|| digits.iter().collect())
    })
}

/// Authors and title. A quoted title (IEEE) splits the entry at the quotes;
/// otherwise the entry splits into sentences, skipping a year sentence after
/// the authors (ACM) and a year in parentheses (APA).
fn authors_title(body: &str) -> (Option<String>, Option<String>) {
    let clean = |s: &str| {
        let s = s.trim().trim_end_matches([',', '.', ':', ';']).trim();
        (!s.is_empty()).then(|| s.to_string())
    };
    for (open, close) in [('“', '”'), ('"', '"')] {
        if let Some(a) = body.find(open)
            && let Some(len) = body[a + open.len_utf8()..].find(close)
        {
            let title = &body[a + open.len_utf8()..a + open.len_utf8() + len];
            return (clean(&body[..a]), clean(title));
        }
    }
    let sentences = sentences(body);
    let is_year = |s: &str| {
        let s = s.trim().trim_start_matches('(').trim_end_matches(')');
        s.len() <= 6 && year(s).is_some()
    };
    // A web page may have no authors: the entry opens with its year.
    let (mut authors, mut rest) = match sentences.first() {
        Some(s) if is_year(s) => (None, &sentences[1..]),
        Some(s) => (Some(s.to_string()), &sentences[1..]),
        None => (None, &sentences[..]),
    };
    if let Some(a) = &authors
        && let Some(open) = a.rfind('(')
        && year(&a[open..]).is_some()
    {
        authors = Some(a[..open].to_string());
    }
    if rest.first().is_some_and(|s| is_year(s)) {
        rest = &rest[1..];
    }
    // Physics styles print no title: `A, B, and C, Phys. Rev. A 95, 1 (2017)`.
    // The authors end at the first comma after the last `and` that is not
    // followed by an initial, and the rest is the venue.
    if let Some(a) = &authors
        && let Some(and) = a.rfind(" and ")
        && let Some(comma) = a[and..].find(", ")
    {
        let after = a[and + comma + 2..].split(' ').next().unwrap_or("");
        let initial = after.trim_end_matches(['.', ',']).chars().count() <= 2;
        let editors = ["editor", "eds", "ed."]
            .iter()
            .any(|e| after.starts_with(e));
        if !initial && !editors {
            return (clean(&a[..and + comma]), None);
        }
    }
    (
        authors.as_deref().and_then(clean),
        rest.first().and_then(|s| clean(s)),
    )
}

/// Sentences split at `. `, `? ` and `! `, but not after a short
/// abbreviation such as `Proc.` or an initial inside a name (`J. Smith`). An
/// initial after `Surname,` that a capitalised word follows ends the author
/// list (`Abbeel, P. Variational ...`), and so does `et al.`.
fn sentences(text: &str) -> Vec<&str> {
    let mut out = vec![];
    let mut start = 0;
    let bytes = text.as_bytes();
    for (i, c) in text.char_indices() {
        if !".?!".contains(c) || bytes.get(i + 1) != Some(&b' ') {
            continue;
        }
        let before: Vec<&str> = text[start..i].rsplit(' ').take(2).collect();
        let word = before.first().copied().unwrap_or("");
        let next = text[i + 2..].split(' ').next().unwrap_or("");
        let initial_like = |w: &str| {
            let w = w.trim_end_matches(['.', ',']);
            w.chars().count() <= 2 && w.chars().next().is_some_and(char::is_uppercase)
        };
        let initial = initial_like(word);
        let ends_names = initial
            && before.get(1).is_some_and(|w| w.ends_with(','))
            && next.starts_with(char::is_uppercase)
            && !initial_like(next)
            && !next.ends_with(',');
        let abbrev = c == '.'
            && (matches!(
                word,
                "et" | "Proc" | "Conf" | "Int" | "vol" | "no" | "pp" | "Vol" | "No"
            ) || initial && !ends_names
                || word.contains('.'));
        if abbrev {
            continue;
        }
        out.push(&text[start..=i]);
        start = i + 2;
    }
    if start < text.len() {
        out.push(&text[start..]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_styles() {
        let ieee = entry(
            1,
            "[3] J. Smith and K. Lee, “Fast storage for things,” in Proc. FAST, 2020, pp. 1–10. doi: 10.1145/12345.678",
        );
        assert_eq!(ieee.n, 3);
        assert_eq!(ieee.authors.as_deref(), Some("J. Smith and K. Lee"));
        assert_eq!(ieee.title.as_deref(), Some("Fast storage for things"));
        assert_eq!(ieee.year.as_deref(), Some("2020"));
        assert_eq!(ieee.id.as_deref(), Some("doi:10.1145/12345.678"));
        let acm = entry(
            2,
            "Jane Doe and John Roe. 2021. A Study of Caches. In Proceedings of SOSP. ACM, 1–12. https://doi.org/10.1145/3477132.3483550",
        );
        assert_eq!(acm.authors.as_deref(), Some("Jane Doe and John Roe"));
        assert_eq!(acm.title.as_deref(), Some("A Study of Caches"));
        assert_eq!(acm.year.as_deref(), Some("2021"));
        assert_eq!(acm.id.as_deref(), Some("doi:10.1145/3477132.3483550"));
        let apa = entry(
            5,
            "Smith, J. and Lee, K. (2019). Learning to rank. arXiv preprint arXiv:1901.01234.",
        );
        assert_eq!(apa.authors.as_deref(), Some("Smith, J. and Lee, K"));
        assert_eq!(apa.title.as_deref(), Some("Learning to rank"));
        assert_eq!(apa.year.as_deref(), Some("2019"));
        assert_eq!(apa.id.as_deref(), Some("arXiv:1901.01234"));
        let natbib = entry(
            1,
            "Achiam, J., Edwards, H., and Abbeel, P. Variational option discovery algorithms. arXiv preprint arXiv:1807.10299, 2018.",
        );
        assert_eq!(
            natbib.authors.as_deref(),
            Some("Achiam, J., Edwards, H., and Abbeel, P")
        );
        assert_eq!(
            natbib.title.as_deref(),
            Some("Variational option discovery algorithms")
        );
        let physics = entry(
            2,
            "M. Puchalski, D. Kedziera, and K. Pachucki, Phys. Rev. A 82, 062509 (2010).",
        );
        assert_eq!(
            physics.authors.as_deref(),
            Some("M. Puchalski, D. Kedziera, and K. Pachucki")
        );
        assert_eq!(
            (physics.title, physics.year.as_deref()),
            (None, Some("2010"))
        );
        let etal = entry(
            1,
            "Ray, A., et al. Learning dexterous manipulation. arXiv preprint arXiv:1808.00177, 2018.",
        );
        assert_eq!(
            etal.title.as_deref(),
            Some("Learning dexterous manipulation")
        );
    }

    #[test]
    fn rejoins_hyphenated_lines() {
        assert_eq!(
            join_lines(["A fast stor-", "age system"].into_iter()),
            "A fast storage system"
        );
        assert_eq!(
            join_lines(["pages 1-", "Linux"].into_iter()),
            "pages 1- Linux"
        );
        assert_eq!(
            join_lines(["High-", "Performance"].into_iter()),
            "High-Performance"
        );
        assert_eq!(
            join_lines(["see https://x.org/", "a/b"].into_iter()),
            "see https://x.org/a/b"
        );
        let web = entry(1, "2021. Lustre. https://www.lustre.org/");
        assert_eq!((web.authors, web.title.as_deref()), (None, Some("Lustre")));
    }
}

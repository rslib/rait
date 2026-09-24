use crate::markdown::{Block, push};
use crate::xml::{Ev, parse, skip};
use std::collections::HashMap;

#[derive(Default)]
struct Para {
    text: String,
    style: Option<String>,
    num_id: Option<String>,
    ilvl: usize,
    outline: Option<u8>,
}

struct Cell {
    lines: Vec<String>,
    span: usize,
    merged: bool,
}

/// Blocks of `word/document.xml`, using `styles.xml` for heading levels and
/// `numbering.xml` for list kinds when present.
pub fn blocks(
    document: &str,
    styles: Option<&str>,
    numbering: Option<&str>,
) -> Result<Vec<Block>, String> {
    let headings = styles.map(heading_levels).transpose()?.unwrap_or_default();
    let lists = numbering.map(list_kinds).transpose()?.unwrap_or_default();
    let events = parse(document)?;
    let mut blocks = vec![];
    let mut paras: Vec<Para> = vec![];
    let mut table_depth = 0;
    let mut rows: Vec<Vec<String>> = vec![];
    let mut row: Vec<String> = vec![];
    let mut cell: Option<Cell> = None;
    let mut in_run = 0;
    let mut in_text = false;
    let mut i = 0;
    while i < events.len() {
        match &events[i] {
            Ev::Start(tag) => match tag.name.as_str() {
                // Word repeats text boxes in a VML fallback, and deleted runs are not content.
                "Fallback" | "del" | "moveFrom" => {
                    i = skip(&events, i);
                    continue;
                }
                "p" => paras.push(Para::default()),
                "pStyle" => {
                    if let Some(p) = paras.last_mut() {
                        p.style = tag.attr("val").map(String::from);
                    }
                }
                "numId" => {
                    if let Some(p) = paras.last_mut() {
                        p.num_id = tag.attr("val").map(String::from);
                    }
                }
                "ilvl" => {
                    if let Some(p) = paras.last_mut() {
                        p.ilvl = tag.attr("val").and_then(|v| v.parse().ok()).unwrap_or(0);
                    }
                }
                "outlineLvl" => {
                    if let Some(p) = paras.last_mut() {
                        p.outline = outline_level(tag.attr("val"));
                    }
                }
                "r" => in_run += 1,
                "t" => in_text = true,
                "tab" if in_run > 0 => append(&mut paras, "\t"),
                "br" | "cr" if in_run > 0 => append(&mut paras, "\n"),
                "tbl" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        rows.clear();
                    }
                }
                "tr" if table_depth == 1 => row.clear(),
                "tc" if table_depth == 1 => {
                    cell = Some(Cell {
                        lines: vec![],
                        span: 1,
                        merged: false,
                    })
                }
                "gridSpan" if table_depth == 1 => {
                    if let Some(cell) = cell.as_mut() {
                        cell.span = tag.attr("val").and_then(|v| v.parse().ok()).unwrap_or(1);
                    }
                }
                "vMerge" if table_depth == 1 => {
                    if let Some(cell) = cell.as_mut() {
                        cell.merged = tag.attr("val") != Some("restart");
                    }
                }
                _ => {}
            },
            Ev::End(name) => match name.as_str() {
                "r" => in_run -= 1,
                "t" => in_text = false,
                "p" => {
                    let Some(p) = paras.pop() else {
                        i += 1;
                        continue;
                    };
                    match cell.as_mut() {
                        Some(cell) if table_depth > 0 => cell.lines.push(p.text.trim().into()),
                        _ => push(&mut blocks, classify(p, &headings, &lists)),
                    }
                }
                "tc" if table_depth == 1 => {
                    if let Some(cell) = cell.take() {
                        row.push(if cell.merged {
                            String::new()
                        } else {
                            cell.lines.join("\n")
                        });
                        row.extend((1..cell.span).map(|_| String::new()));
                    }
                }
                "tr" if table_depth == 1 => rows.push(std::mem::take(&mut row)),
                "tbl" => {
                    if table_depth == 1 && !rows.is_empty() {
                        push(&mut blocks, Block::Table(std::mem::take(&mut rows)));
                    }
                    table_depth -= 1;
                }
                _ => {}
            },
            Ev::Text(text) => {
                if in_text {
                    append(&mut paras, text);
                }
            }
        }
        i += 1;
    }
    Ok(blocks)
}

fn append(paras: &mut [Para], text: &str) {
    if let Some(p) = paras.last_mut() {
        p.text.push_str(text);
    }
}

fn classify(
    p: Para,
    headings: &HashMap<String, u8>,
    lists: &HashMap<String, HashMap<usize, bool>>,
) -> Block {
    let level = p
        .style
        .as_deref()
        .and_then(|style| {
            headings
                .get(style)
                .copied()
                .or_else(|| style_id_level(style))
        })
        .or(p.outline);
    if let Some(level) = level {
        return Block::Heading(level, p.text);
    }
    match p.num_id.as_deref() {
        Some(num_id) if num_id != "0" => Block::Item {
            depth: p.ilvl,
            ordered: lists
                .get(num_id)
                .and_then(|levels| levels.get(&p.ilvl))
                .copied()
                .unwrap_or(false),
            text: p.text,
        },
        _ => Block::Paragraph(p.text),
    }
}

/// Heading level by paragraph style id, from each style's name or outline level.
fn heading_levels(xml: &str) -> Result<HashMap<String, u8>, String> {
    let mut levels = HashMap::new();
    let mut id = None;
    let mut level = None;
    for ev in parse(xml)? {
        match ev {
            Ev::Start(tag) => match tag.name.as_str() {
                "style" => {
                    id = tag.attr("styleId").map(String::from);
                    level = None;
                }
                "name" => level = level.or_else(|| name_level(tag.attr("val")?)),
                "outlineLvl" => level = level.or(outline_level(tag.attr("val"))),
                _ => {}
            },
            Ev::End(name) if name == "style" => {
                if let (Some(id), Some(level)) = (id.take(), level.take()) {
                    levels.insert(id, level);
                }
            }
            _ => {}
        }
    }
    Ok(levels)
}

fn name_level(name: &str) -> Option<u8> {
    let name = name.to_lowercase();
    if name == "title" {
        return Some(1);
    }
    name.strip_prefix("heading ")?
        .parse()
        .ok()
        .filter(|n| (1..=9).contains(n))
}

fn style_id_level(id: &str) -> Option<u8> {
    if id == "Title" {
        return Some(1);
    }
    id.strip_prefix("Heading")?
        .parse()
        .ok()
        .filter(|n| (1..=9).contains(n))
}

fn outline_level(value: Option<&str>) -> Option<u8> {
    value?.parse::<u8>().ok().filter(|n| *n < 9).map(|n| n + 1)
}

/// Whether each list level is ordered, by `numId` then `ilvl`.
fn list_kinds(xml: &str) -> Result<HashMap<String, HashMap<usize, bool>>, String> {
    let mut abstracts: HashMap<String, HashMap<usize, bool>> = HashMap::new();
    let mut nums: HashMap<String, String> = HashMap::new();
    let mut abstract_id = None;
    let mut ilvl = 0;
    let mut num_id = None;
    for ev in parse(xml)? {
        let Ev::Start(tag) = ev else { continue };
        match tag.name.as_str() {
            "abstractNum" => abstract_id = tag.attr("abstractNumId").map(String::from),
            "lvl" => ilvl = tag.attr("ilvl").and_then(|v| v.parse().ok()).unwrap_or(0),
            "numFmt" => {
                if let Some(id) = &abstract_id {
                    let ordered = !matches!(tag.attr("val"), Some("bullet" | "none") | None);
                    abstracts
                        .entry(id.clone())
                        .or_default()
                        .insert(ilvl, ordered);
                }
            }
            "num" => {
                abstract_id = None;
                num_id = tag.attr("numId").map(String::from);
            }
            "abstractNumId" => {
                if let (Some(num), Some(target)) = (num_id.take(), tag.attr("val")) {
                    nums.insert(num, target.into());
                }
            }
            _ => {}
        }
    }
    Ok(nums
        .into_iter()
        .filter_map(|(num, target)| Some((num, abstracts.get(&target)?.clone())))
        .collect())
}

use crate::markdown::{Block, push};
use crate::resolve;
use crate::xml::{Ev, parse};
use std::collections::HashMap;

/// Slide part paths in presentation order, which can differ from file name order.
pub fn slide_paths(presentation: &str, rels: &str) -> Result<Vec<String>, String> {
    let mut targets = HashMap::new();
    for ev in parse(rels)? {
        if let Ev::Start(tag) = ev
            && tag.name == "Relationship"
            && let (Some(id), Some(target)) = (tag.attr("Id"), tag.attr("Target"))
        {
            targets.insert(id.to_string(), resolve("ppt/presentation.xml", target));
        }
    }
    let mut paths = vec![];
    for ev in parse(presentation)? {
        if let Ev::Start(tag) = ev
            && tag.name == "sldId"
            && let Some(path) = tag.prefixed_attr("id").and_then(|id| targets.get(id))
        {
            paths.push(path.clone());
        }
    }
    Ok(paths)
}

/// Bullet set on a paragraph; `Inherit` defers to the shape's placeholder.
#[derive(Clone, Copy)]
enum Bullet {
    Inherit,
    None,
    Char,
    Number,
}

/// Placeholder kind of the shape being read.
enum Shape {
    Plain,
    Title,
    Body,
    /// Date, footer and slide number repeat on every slide.
    Chrome,
}

pub fn slide_blocks(xml: &str) -> Result<Vec<Block>, String> {
    let mut blocks = vec![];
    let mut shape = Shape::Plain;
    let mut para: Option<(String, usize, Bullet)> = None;
    let mut in_text = false;
    let mut table_depth = 0;
    let mut rows: Vec<Vec<String>> = vec![];
    let mut row: Vec<String> = vec![];
    let mut cell: Option<(Vec<String>, bool)> = None;
    for ev in parse(xml)? {
        match ev {
            Ev::Start(tag) => match tag.name.as_str() {
                "sp" => shape = Shape::Plain,
                "ph" => {
                    shape = match tag.attr("type") {
                        Some("title" | "ctrTitle") => Shape::Title,
                        Some("dt" | "ftr" | "sldNum") => Shape::Chrome,
                        None | Some("body" | "obj") => Shape::Body,
                        Some(_) => Shape::Plain,
                    }
                }
                "p" => para = Some((String::new(), 0, Bullet::Inherit)),
                "pPr" => {
                    if let Some((_, lvl, _)) = para.as_mut() {
                        *lvl = tag.attr("lvl").and_then(|v| v.parse().ok()).unwrap_or(0);
                    }
                }
                "buNone" | "buChar" | "buAutoNum" => {
                    if let Some((_, _, bullet)) = para.as_mut() {
                        *bullet = match tag.name.as_str() {
                            "buNone" => Bullet::None,
                            "buChar" => Bullet::Char,
                            _ => Bullet::Number,
                        };
                    }
                }
                "t" => in_text = true,
                "br" => {
                    if let Some((text, _, _)) = para.as_mut() {
                        text.push('\n');
                    }
                }
                "tbl" => {
                    table_depth += 1;
                    if table_depth == 1 {
                        rows.clear();
                    }
                }
                "tr" if table_depth == 1 => row.clear(),
                "tc" if table_depth == 1 => {
                    // Cells covered by a merge are still present, flagged hMerge or vMerge.
                    let merged = tag.attr("hMerge") == Some("1") || tag.attr("vMerge") == Some("1");
                    cell = Some((vec![], merged));
                }
                _ => {}
            },
            Ev::End(name) => match name.as_str() {
                "t" => in_text = false,
                "p" => {
                    let Some((text, lvl, bullet)) = para.take() else {
                        continue;
                    };
                    let item = |ordered| Block::Item {
                        depth: lvl,
                        ordered,
                        text: text.clone(),
                    };
                    match cell.as_mut() {
                        Some((lines, _)) if table_depth > 0 => lines.push(text.trim().into()),
                        _ => push(
                            &mut blocks,
                            match (&shape, bullet) {
                                (Shape::Chrome, _) => continue,
                                (Shape::Title, _) => Block::Heading(1, text),
                                (_, Bullet::Number) => item(true),
                                (_, Bullet::Char) | (Shape::Body, Bullet::Inherit) => item(false),
                                _ => Block::Paragraph(text),
                            },
                        ),
                    }
                }
                "tc" if table_depth == 1 => {
                    if let Some((lines, merged)) = cell.take() {
                        row.push(if merged {
                            String::new()
                        } else {
                            lines.join("\n")
                        });
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
                if in_text && let Some((para, _, _)) = para.as_mut() {
                    para.push_str(&text);
                }
            }
        }
    }
    // Shapes are stored in drawing order; lead with the title.
    if let Some(title) = blocks.iter().position(|b| matches!(b, Block::Heading(..))) {
        let heading = blocks.remove(title);
        blocks.insert(0, heading);
    }
    Ok(blocks)
}

use rait_core::cells::markdown_table;

/// Document structure that survives extraction, rendered as GitHub-flavoured Markdown.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    Heading(u8, String),
    Paragraph(String),
    Item {
        depth: usize,
        ordered: bool,
        text: String,
    },
    Table(Vec<Vec<String>>),
    /// A fenced code block, with an optional info-string language.
    Code(Option<String>, String),
}

pub fn render(blocks: &[Block]) -> String {
    let mut out = String::new();
    let mut previous: Option<&Block> = None;
    for block in blocks {
        if !out.is_empty() {
            out.push_str(if same_list(previous, block) {
                "\n"
            } else {
                "\n\n"
            });
        }
        previous = Some(block);
        match block {
            Block::Heading(level, text) => {
                out.push_str(&"#".repeat((*level).clamp(1, 6) as usize));
                out.push(' ');
                out.push_str(&one_line(text));
            }
            Block::Paragraph(text) => {
                let lines: Vec<&str> = text.trim().lines().map(str::trim_end).collect();
                out.push_str(&lines.join("\n"));
            }
            Block::Item {
                depth,
                ordered,
                text,
            } => {
                out.push_str(&"  ".repeat(*depth));
                out.push_str(if *ordered { "1. " } else { "- " });
                out.push_str(&one_line(text));
            }
            Block::Table(rows) => out.push_str(&markdown_table(rows)),
            Block::Code(lang, text) => {
                out.push_str("```");
                out.push_str(lang.as_deref().unwrap_or(""));
                out.push('\n');
                out.push_str(text.trim_end_matches('\n'));
                out.push_str("\n```");
            }
        }
    }
    out
}

/// Items continue one list unless a top-level item switches between bullets and numbers.
fn same_list(previous: Option<&Block>, block: &Block) -> bool {
    match (previous, block) {
        (
            Some(Block::Item { ordered: a, .. }),
            Block::Item {
                depth, ordered: b, ..
            },
        ) => *depth > 0 || a == b,
        _ => false,
    }
}

/// Split into parts at each level-1 heading, keeping text before the first one.
pub fn split_at_h1(blocks: Vec<Block>) -> Vec<Vec<Block>> {
    let mut parts: Vec<Vec<Block>> = vec![];
    for block in blocks {
        let starts_part = matches!(block, Block::Heading(1, _));
        match parts.last_mut() {
            Some(part) if !starts_part || part.is_empty() => part.push(block),
            _ => parts.push(vec![block]),
        }
    }
    parts
}

/// A block is kept only when it carries text.
pub fn push(blocks: &mut Vec<Block>, block: Block) {
    let empty = match &block {
        Block::Heading(_, text)
        | Block::Paragraph(text)
        | Block::Item { text, .. }
        | Block::Code(_, text) => text.trim().is_empty(),
        Block::Table(rows) => rows
            .iter()
            .all(|row| row.iter().all(|cell| cell.is_empty())),
    };
    if !empty {
        blocks.push(block);
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_tables_lists_and_headings() {
        let blocks = vec![
            Block::Heading(1, "Title".into()),
            Block::Item {
                depth: 0,
                ordered: false,
                text: "a".into(),
            },
            Block::Item {
                depth: 1,
                ordered: true,
                text: "b".into(),
            },
            Block::Table(vec![
                vec!["h1".into(), "h2".into()],
                vec!["x|y".into(), "one\ntwo".into()],
                vec!["short".into()],
            ]),
        ];
        assert_eq!(
            render(&blocks),
            "# Title\n\n- a\n  1. b\n\n| h1 | h2 |\n| --- | --- |\n| x\\|y | one<br>two |\n| short |  |"
        );
    }

    #[test]
    fn splits_at_level_one_headings() {
        let blocks = vec![
            Block::Paragraph("intro".into()),
            Block::Heading(1, "A".into()),
            Block::Heading(2, "A.1".into()),
            Block::Heading(1, "B".into()),
        ];
        let parts = split_at_h1(blocks);
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1].len(), 2);
    }
}

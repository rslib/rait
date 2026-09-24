use crate::markdown::{Block, push};
use scraper::{ElementRef, Html, Node};

pub fn blocks(html: &str) -> Vec<Block> {
    let doc = Html::parse_document(html);
    let mut walker = Walker::default();
    walker.walk(doc.root_element());
    walker.flush();
    walker.blocks
}

#[derive(Default)]
struct Walker {
    blocks: Vec<Block>,
    inline: String,
    lists: Vec<bool>,
    item: Option<(usize, bool)>,
}

impl Walker {
    fn walk(&mut self, element: ElementRef) {
        for child in element.children() {
            match child.value() {
                // Source newlines are just whitespace; only <br> breaks a line.
                Node::Text(text) => self.inline.push_str(&text.replace(['\n', '\r'], " ")),
                Node::Element(_) => {
                    let Some(child) = ElementRef::wrap(child) else {
                        continue;
                    };
                    self.element(child);
                }
                _ => {}
            }
        }
    }

    fn element(&mut self, element: ElementRef) {
        let name = element.value().name();
        match name {
            "head" | "script" | "style" | "noscript" | "template" | "svg" => {}
            "br" => self.inline.push('\n'),
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => {
                self.flush();
                let level = name[1..].parse().unwrap_or(1);
                push(
                    &mut self.blocks,
                    Block::Heading(level, inline_text(element)),
                );
            }
            "ul" | "ol" => {
                self.flush();
                self.lists.push(name == "ol");
                self.walk(element);
                self.flush();
                self.lists.pop();
            }
            "li" => {
                self.flush();
                let ordered = self.lists.last().copied().unwrap_or(false);
                self.item = Some((self.lists.len().saturating_sub(1), ordered));
                self.walk(element);
                self.flush();
            }
            "table" => {
                self.flush();
                push(&mut self.blocks, Block::Table(table(element)));
            }
            "pre" => {
                self.flush();
                push(&mut self.blocks, Block::Code(element.text().collect()));
            }
            "p" | "div" | "section" | "article" | "main" | "header" | "footer" | "aside"
            | "nav" | "blockquote" | "figure" | "figcaption" | "address" | "body" | "dl" | "dt"
            | "dd" | "details" | "summary" | "hr" => {
                self.flush();
                self.walk(element);
                self.flush();
            }
            _ => self.walk(element),
        }
    }

    /// Emit pending inline text as the current list item or a paragraph.
    fn flush(&mut self) {
        let text = collapse(&std::mem::take(&mut self.inline));
        let block = match self.item.take() {
            Some((depth, ordered)) => Block::Item {
                depth,
                ordered,
                text,
            },
            None => Block::Paragraph(text),
        };
        push(&mut self.blocks, block);
    }
}

/// Rows of `table`, excluding nested tables' rows. Spanned cells are padded with
/// empty cells so every row keeps its columns.
fn table(table: ElementRef) -> Vec<Vec<String>> {
    let mut rows = vec![];
    // Column -> rows still covered by a rowspan from above.
    let mut carried: Vec<usize> = vec![];
    for tr in own_rows(table) {
        let mut row = vec![];
        let mut col = 0;
        for cell in tr
            .child_elements()
            .filter(|c| matches!(c.value().name(), "td" | "th"))
        {
            fill_carried(&mut carried, &mut row, &mut col);
            let colspan = span(cell, "colspan");
            let rowspan = span(cell, "rowspan");
            row.push(inline_text(cell));
            row.extend((1..colspan).map(|_| String::new()));
            for c in col..col + colspan {
                if carried.len() <= c {
                    carried.resize(c + 1, 0);
                }
                carried[c] = rowspan - 1;
            }
            col += colspan;
        }
        fill_carried(&mut carried, &mut row, &mut col);
        rows.push(row);
    }
    rows
}

fn fill_carried(carried: &mut [usize], row: &mut Vec<String>, col: &mut usize) {
    while carried.get(*col).is_some_and(|n| *n > 0) {
        carried[*col] -= 1;
        row.push(String::new());
        *col += 1;
    }
}

fn own_rows(table: ElementRef) -> Vec<ElementRef> {
    let mut rows = vec![];
    for child in table.child_elements() {
        match child.value().name() {
            "tr" => rows.push(child),
            "thead" | "tbody" | "tfoot" => {
                rows.extend(child.child_elements().filter(|c| c.value().name() == "tr"))
            }
            _ => {}
        }
    }
    rows
}

fn span(cell: ElementRef, attr: &str) -> usize {
    cell.value()
        .attr(attr)
        .and_then(|v| v.trim().parse().ok())
        .filter(|n| (1..=1000).contains(n))
        .unwrap_or(1)
}

fn inline_text(element: ElementRef) -> String {
    element
        .text()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn collapse(text: &str) -> String {
    text.split('\n')
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

use quick_xml::Reader;
use quick_xml::events::{BytesStart, Event};

/// A word and its box, in PDF points from the top-left corner.
#[derive(Clone, Debug)]
pub struct Word {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    pub text: String,
    /// Poppler's line, numbered within the page. Poppler keeps a cell's words
    /// on one line even when justification stretches the gaps.
    pub line: usize,
}

impl Word {
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }

    pub fn center_y(&self) -> f64 {
        (self.y0 + self.y1) / 2.0
    }
}

/// Words of one page, plus Poppler's reading-order grouping of them into
/// blocks of lines (indexes into `words`).
#[derive(Clone, Debug, Default)]
pub struct Page {
    pub words: Vec<Word>,
    pub blocks: Vec<Vec<Vec<usize>>>,
}

/// Parse `pdftotext -bbox-layout` output.
pub fn parse(xml: &str) -> Result<Vec<Page>, String> {
    let mut reader = Reader::from_str(xml);
    let mut pages: Vec<Page> = vec![];
    let mut word: Option<Word> = None;
    let mut line = 0;
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) => match e.local_name().as_ref() {
                "page" => pages.push(Page::default()),
                "block" => {
                    if let Some(page) = pages.last_mut() {
                        page.blocks.push(vec![]);
                    }
                }
                "line" => {
                    line += 1;
                    if let Some(block) = pages.last_mut().and_then(|p| p.blocks.last_mut()) {
                        block.push(vec![]);
                    }
                }
                "word" => {
                    word = Some(Word {
                        x0: coord(&e, "xMin")?,
                        y0: coord(&e, "yMin")?,
                        x1: coord(&e, "xMax")?,
                        y1: coord(&e, "yMax")?,
                        text: String::new(),
                        line,
                    })
                }
                _ => {}
            },
            Event::Text(e) => {
                if let Some(word) = word.as_mut() {
                    word.text.push_str(&e.xml10_content());
                }
            }
            Event::GeneralRef(e) => {
                if let Some(word) = word.as_mut() {
                    let text = match e.resolve_char_ref().map_err(|e| e.to_string())? {
                        Some(c) => c.to_string(),
                        None => quick_xml::escape::resolve_predefined_entity(&e.xml10_content())
                            .unwrap_or_default()
                            .to_string(),
                    };
                    word.text.push_str(&text);
                }
            }
            Event::End(e) if e.local_name().as_ref() == "word" => {
                let (Some(word), Some(page)) = (word.take(), pages.last_mut()) else {
                    continue;
                };
                let index = page.words.len();
                page.words.push(word);
                if let Some(line) = page.blocks.last_mut().and_then(|b| b.last_mut()) {
                    line.push(index);
                }
            }
            Event::Eof => return Ok(pages),
            _ => {}
        }
    }
}

fn coord(e: &BytesStart, name: &str) -> Result<f64, String> {
    let attr = e
        .try_get_attribute(name)
        .map_err(|err| err.to_string())?
        .ok_or_else(|| format!("word without {name}"))?;
    attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
        .map_err(|err| err.to_string())?
        .parse()
        .map_err(|_| format!("bad {name}"))
}

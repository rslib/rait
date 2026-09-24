use quick_xml::escape::resolve_predefined_entity;
use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, XmlVersion};

/// A flattened XML event. Self-closing tags produce `Start` then `End`.
pub enum Ev {
    Start(Tag),
    End(String),
    Text(String),
}

pub struct Tag {
    /// Local name, without namespace prefix.
    pub name: String,
    /// Attributes by qualified name.
    attrs: Vec<(String, String)>,
}

impl Tag {
    /// Value of the attribute whose local name is `local`, preferring an unprefixed one.
    pub fn attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name == local)
            .or_else(|| {
                self.attrs
                    .iter()
                    .find(|(name, _)| name.rsplit(':').next() == Some(local))
            })
            .map(|(_, value)| value.as_str())
    }

    /// Value of a namespaced attribute such as `r:id`, whatever the prefix is.
    pub fn prefixed_attr(&self, local: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(name, _)| name.contains(':') && name.rsplit(':').next() == Some(local))
            .map(|(_, value)| value.as_str())
    }
}

pub fn parse(xml: &str) -> Result<Vec<Ev>, String> {
    let mut reader = Reader::from_str(xml);
    let mut events = vec![];
    loop {
        match reader.read_event().map_err(|err| err.to_string())? {
            Event::Start(e) => events.push(Ev::Start(tag(&e)?)),
            Event::Empty(e) => {
                let tag = tag(&e)?;
                let name = tag.name.clone();
                events.push(Ev::Start(tag));
                events.push(Ev::End(name));
            }
            Event::End(e) => events.push(Ev::End(e.local_name().as_ref().to_string())),
            Event::Text(e) => events.push(Ev::Text(e.xml10_content().into_owned())),
            Event::CData(e) => events.push(Ev::Text(e.into_inner().into_owned())),
            Event::GeneralRef(e) => {
                let text = match e.resolve_char_ref().map_err(|err| err.to_string())? {
                    Some(c) => c.to_string(),
                    None => {
                        let name = e.xml10_content();
                        resolve_predefined_entity(&name)
                            .unwrap_or_default()
                            .to_string()
                    }
                };
                events.push(Ev::Text(text));
            }
            Event::Eof => return Ok(events),
            _ => {}
        }
    }
}

fn tag(e: &BytesStart) -> Result<Tag, String> {
    let mut attrs = vec![];
    for attr in e.attributes() {
        let attr = attr.map_err(|err| err.to_string())?;
        let value = attr
            .normalized_value(XmlVersion::Implicit1_0)
            .map_err(|err| err.to_string())?;
        attrs.push((attr.key.as_ref().to_string(), value.into_owned()));
    }
    Ok(Tag {
        name: e.local_name().as_ref().to_string(),
        attrs,
    })
}

/// Index just past the `End` matching the `Start` at `start`.
pub fn skip(events: &[Ev], start: usize) -> usize {
    let mut depth = 0;
    for (i, ev) in events.iter().enumerate().skip(start) {
        match ev {
            Ev::Start(_) => depth += 1,
            Ev::End(_) => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            Ev::Text(_) => {}
        }
    }
    events.len()
}

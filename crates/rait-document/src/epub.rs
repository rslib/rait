use crate::resolve;
use crate::xml::{Ev, parse};
use std::collections::HashMap;

/// Path of the OPF package file named by `META-INF/container.xml`.
pub fn package_path(container: &str) -> Result<String, String> {
    parse(container)?
        .into_iter()
        .find_map(|ev| match ev {
            Ev::Start(tag) if tag.name == "rootfile" => tag.attr("full-path").map(String::from),
            _ => None,
        })
        .ok_or_else(|| "container.xml names no rootfile".into())
}

/// Content documents in reading order.
pub fn spine(opf: &str, opf_path: &str) -> Result<Vec<String>, String> {
    let mut manifest = HashMap::new();
    let mut order = vec![];
    for ev in parse(opf)? {
        let Ev::Start(tag) = ev else { continue };
        match tag.name.as_str() {
            "item" => {
                let is_html = matches!(
                    tag.attr("media-type"),
                    Some("application/xhtml+xml" | "text/html")
                );
                if let (true, Some(id), Some(href)) = (is_html, tag.attr("id"), tag.attr("href")) {
                    manifest.insert(id.to_string(), resolve(opf_path, &percent_decode(href)));
                }
            }
            "itemref" => {
                if tag.attr("linear") != Some("no")
                    && let Some(idref) = tag.attr("idref")
                {
                    order.push(idref.to_string());
                }
            }
            _ => {}
        }
    }
    Ok(order
        .into_iter()
        .filter_map(|id| manifest.get(&id).cloned())
        .collect())
}

fn percent_decode(href: &str) -> String {
    let bytes = href.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = href
                .get(i + 1..i + 3)
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

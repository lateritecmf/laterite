//! What the rich editor stores: HTML constrained to the editor's own schema.
//!
//! The editor can only produce what its schema allows, and this module makes
//! the server keep the same promise for a request the editor did not send. The
//! allow-list below is the schema, element for element, so a document that
//! comes back from storage is one the editor could have written.

use std::collections::{HashMap, HashSet};

/// Cleans submitted rich text to the editor's schema: paragraphs, headings,
/// inline marks, lists, quotes, code, links, images from the library, rules
/// and tables. Scripts, event handlers, styles, unknown elements and unknown
/// attributes are dropped; relative URLs (the library's own) pass through, and
/// a link that opens a new tab gains `rel="noopener noreferrer"`.
pub(crate) fn clean(html: &str) -> String {
    let mut builder = ammonia::Builder::default();
    builder
        .tags(HashSet::from([
            "p",
            "br",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "strong",
            "b",
            "em",
            "i",
            "u",
            "s",
            "code",
            "pre",
            "blockquote",
            "ul",
            "ol",
            "li",
            "a",
            "img",
            "hr",
            "table",
            "thead",
            "tbody",
            "tr",
            "th",
            "td",
        ]))
        .tag_attributes(HashMap::from([
            ("a", HashSet::from(["href", "title", "target"])),
            (
                "img",
                HashSet::from(["src", "alt", "title", "width", "height", "data-media"]),
            ),
            ("ol", HashSet::from(["start", "type"])),
            ("th", HashSet::from(["colspan", "rowspan", "colwidth"])),
            ("td", HashSet::from(["colspan", "rowspan", "colwidth"])),
            ("code", HashSet::from(["class"])),
        ]))
        .generic_attributes(HashSet::new())
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(Some("noopener noreferrer"))
        .attribute_filter(|element, attribute, value| {
            // A code block's class names its language and nothing else.
            if element == "code" && attribute == "class" {
                return value.starts_with("language-").then(|| value.into());
            }
            // A library reference is a record id.
            if attribute == "data-media" && value.parse::<i64>().is_err() {
                return None;
            }
            Some(value.into())
        });
    builder.clean(html).to_string()
}

/// Whether cleaned rich text holds nothing an author would call content: no
/// text, no image, no rule, no table. An empty editor submits `<p></p>`, which
/// a `required` rule has to see as empty.
pub(crate) fn is_blank(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    if lower.contains("<img") || lower.contains("<hr") || lower.contains("<table") {
        return false;
    }
    let text = html.replace("&nbsp;", " ").replace("&#160;", " ");
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag && !c.is_whitespace() && c != '\u{a0}' => return false,
            _ => {}
        }
    }
    true
}

/// The library records a document refers to, from the `data-media` attribute
/// the editor writes on an image it inserted from the library.
pub(crate) fn media_ids(html: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let mut rest = html;
    while let Some(at) = rest.find("data-media=\"") {
        rest = &rest[at + "data-media=\"".len()..];
        let Some(end) = rest.find('"') else { break };
        if let Ok(id) = rest[..end].parse::<i64>() {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        rest = &rest[end..];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_the_schema_and_drops_the_rest() {
        let dirty = r#"<h2 style="color:red" onclick="x()">Title</h2><p>Hi <strong>there</strong> <span class="x">span</span><script>alert(1)</script></p><img src="/storage/media/abc/a.jpg" data-media="12" alt="A"><a href="https://acme.com" target="_blank">out</a><iframe src="x"></iframe>"#;
        let clean = clean(dirty);
        assert_eq!(
            clean,
            r#"<h2>Title</h2><p>Hi <strong>there</strong> span</p><img src="/storage/media/abc/a.jpg" data-media="12" alt="A"><a href="https://acme.com" target="_blank" rel="noopener noreferrer">out</a>"#
        );
    }

    #[test]
    fn code_class_only_names_a_language_and_media_is_an_id() {
        let clean = clean(
            r#"<pre><code class="language-rust">fn</code></pre><code class="evil">x</code><img src="a.png" data-media="abc">"#,
        );
        assert_eq!(
            clean,
            r#"<pre><code class="language-rust">fn</code></pre><code>x</code><img src="a.png">"#
        );
    }

    #[test]
    fn javascript_urls_go_and_relative_ones_stay() {
        let clean = clean(r#"<a href="javascript:alert(1)">a</a><a href="/about">b</a>"#);
        assert_eq!(
            clean,
            r#"<a rel="noopener noreferrer">a</a><a href="/about" rel="noopener noreferrer">b</a>"#
        );
    }

    #[test]
    fn blank_is_what_an_empty_editor_sends() {
        assert!(is_blank("<p></p>"));
        assert!(is_blank("<p>&nbsp;</p>\n<p> </p>"));
        assert!(!is_blank("<p>a</p>"));
        assert!(!is_blank("<hr>"));
        assert!(!is_blank(r#"<img src="x">"#));
    }

    #[test]
    fn media_ids_are_collected_once_each() {
        let html =
            r#"<img data-media="3"><img data-media="7"><img data-media="3"><img data-media="x">"#;
        assert_eq!(media_ids(html), vec![3, 7]);
    }
}

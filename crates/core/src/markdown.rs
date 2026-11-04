//! Markdown to HTML, one renderer for the admin's preview, the theme's
//! `markdown` filter and content blocks, so what an author previews is what
//! the site shows.
//!
//! The dialect is CommonMark plus tables, strikethrough, footnotes, task
//! lists and heading attributes (`{#id}`). A heading without an id gets one
//! from its text (`## Release 0.9` becomes `id="release-0-9"`), made unique
//! within the text, so any heading can be linked to. Raw HTML in the text
//! passes through, as it does on the site; a surface that shows untrusted
//! text cleans the result itself.

use pulldown_cmark::{CowStr, Event, Tag, TagEnd};

/// Renders markdown to an HTML fragment.
pub fn render(text: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_FOOTNOTES);
    options.insert(Options::ENABLE_TASKLISTS);
    options.insert(Options::ENABLE_HEADING_ATTRIBUTES);
    options.insert(Options::ENABLE_SMART_PUNCTUATION);
    let events = with_heading_ids(Parser::new_ext(text, options).collect());
    let mut out = String::with_capacity(text.len() * 2);
    html::push_html(&mut out, events.into_iter());
    out
}

/// Renders markdown as plain text: the words of headings and paragraphs on
/// their own lines, list items marked, a link's address kept beside its
/// text when the two differ, emphasis and raw HTML dropped. The plain-text
/// part of a message is made this way from its one body.
pub fn to_text(text: &str) -> String {
    use pulldown_cmark::{Options, Parser};
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);
    let mut out = String::with_capacity(text.len());
    let mut link: Option<(String, String)> = None;
    let mut ordered: Vec<Option<u64>> = Vec::new();
    for event in Parser::new_ext(text, options) {
        match event {
            Event::Text(t) | Event::Code(t) => match &mut link {
                Some((_, label)) => label.push_str(&t),
                None => out.push_str(&t),
            },
            Event::SoftBreak => out.push(' '),
            Event::HardBreak => out.push('\n'),
            Event::Start(Tag::Link { dest_url, .. }) => {
                link = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Link) => {
                if let Some((url, label)) = link.take() {
                    let label = label.trim().to_string();
                    if label.is_empty() || label == url {
                        out.push_str(&url);
                    } else {
                        out.push_str(&label);
                        out.push_str(" (");
                        out.push_str(&url);
                        out.push(')');
                    }
                }
            }
            Event::Start(Tag::Image { dest_url, .. }) => {
                link = Some((dest_url.to_string(), String::new()));
            }
            Event::End(TagEnd::Image) => {
                if let Some((_, alt)) = link.take() {
                    out.push_str(alt.trim());
                }
            }
            Event::Start(Tag::List(first)) => ordered.push(first),
            Event::End(TagEnd::List(_)) => {
                ordered.pop();
                out.push('\n');
            }
            Event::Start(Tag::Item) => {
                let depth = ordered.len().saturating_sub(1);
                out.push_str(&"  ".repeat(depth));
                match ordered.last_mut() {
                    Some(Some(n)) => {
                        out.push_str(&format!("{n}. "));
                        *n += 1;
                    }
                    _ => out.push_str("- "),
                }
            }
            Event::End(TagEnd::Item) => out.push('\n'),
            Event::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::BlockQuote(_)) => {
                out.push_str("\n\n");
            }
            Event::End(TagEnd::CodeBlock) => out.push_str("\n\n"),
            Event::End(TagEnd::TableRow | TagEnd::TableHead) => out.push('\n'),
            Event::End(TagEnd::TableCell) => out.push_str("  "),
            Event::End(TagEnd::Table) => out.push('\n'),
            Event::Rule => out.push_str("\n----\n\n"),
            Event::Html(html) | Event::InlineHtml(html) => out.push_str(&strip_tags(&html)),
            Event::TaskListMarker(done) => out.push_str(if done { "[x] " } else { "[ ] " }),
            _ => {}
        }
    }
    let mut tidy = String::with_capacity(out.len());
    let mut blank = 0;
    for line in out.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank += 1;
            if blank > 1 {
                continue;
            }
        } else {
            blank = 0;
        }
        tidy.push_str(line);
        tidy.push('\n');
    }
    tidy.trim().to_string()
}

/// Raw HTML inside markdown, reduced to its text.
fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

/// Gives every heading without an id one derived from its text, unique
/// within the document (`-2`, `-3` on a repeat).
fn with_heading_ids(mut events: Vec<Event<'_>>) -> Vec<Event<'_>> {
    let mut seen: std::collections::HashSet<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::Start(Tag::Heading { id: Some(id), .. }) => Some(id.to_string()),
            _ => None,
        })
        .collect();
    let mut i = 0;
    while i < events.len() {
        if let Event::Start(Tag::Heading { id: None, .. }) = &events[i] {
            let mut text = String::new();
            for e in &events[i + 1..] {
                match e {
                    Event::End(TagEnd::Heading(_)) => break,
                    Event::Text(t) | Event::Code(t) => text.push_str(t),
                    _ => {}
                }
            }
            let base = slug(&text);
            let mut id = base.clone();
            let mut n = 2;
            while seen.contains(&id) {
                id = format!("{base}-{n}");
                n += 1;
            }
            seen.insert(id.clone());
            if let Event::Start(Tag::Heading { id: slot, .. }) = &mut events[i] {
                *slot = Some(CowStr::from(id));
            }
        }
        i += 1;
    }
    events
}

/// A heading's text as an id: lowercase letters and digits, runs of anything
/// else as one hyphen; `heading` when nothing is left.
fn slug(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut gap = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('-');
            }
            gap = false;
            out.extend(c.to_lowercase());
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        "heading".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn plain_text_keeps_the_words_and_the_links() {
        let text = super::to_text(
            "# Hello Ada\n\nYour account on **Acme** is ready.\n\n- [Sign in](https://acme.example/in)\n- <https://acme.example>\n\n<p>Raw &amp; <b>bold</b></p>",
        );
        assert_eq!(
            text,
            "Hello Ada\n\nYour account on Acme is ready.\n\n- Sign in (https://acme.example/in)\n- https://acme.example\n\nRaw & bold"
        );
    }

    use super::render;

    #[test]
    fn renders_the_dialect() {
        let html = render(
            "# Title {#top}\n\nSome ~~old~~ *new* text[^1].\n\n- [x] done\n- [ ] open\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n[^1]: A note.\n",
        );
        assert!(html.contains(r##"<h1 id="top">Title</h1>"##), "{html}");
        assert!(html.contains("<del>old</del>"), "{html}");
        assert!(html.contains("<em>new</em>"), "{html}");
        assert!(html.contains(r#"type="checkbox""#), "{html}");
        assert!(html.contains("<table>"), "{html}");
        assert!(html.contains("footnote"), "{html}");
    }

    #[test]
    fn headings_without_an_id_get_one_from_their_text() {
        let html =
            render("## Release 0.9.25\n\n## Fixed\n\n## Fixed\n\n## `code` Heading!\n\n## ---\n");
        assert!(html.contains(r##"<h2 id="release-0-9-25">"##), "{html}");
        assert!(html.contains(r##"<h2 id="fixed">"##), "{html}");
        assert!(html.contains(r##"<h2 id="fixed-2">"##), "{html}");
        assert!(html.contains(r##"<h2 id="code-heading">"##), "{html}");
        assert!(html.contains(r##"<h2 id="heading">"##), "{html}");
    }

    #[test]
    fn empty_text_renders_nothing() {
        assert_eq!(render(""), "");
        assert_eq!(render("   \n"), "");
    }
}

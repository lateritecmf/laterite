//! Markdown to HTML, one renderer for the admin's preview, the theme's
//! `markdown` filter and content blocks, so what an author previews is what
//! the site shows.
//!
//! The dialect is CommonMark plus tables, strikethrough, footnotes, task
//! lists and heading attributes (`{#id}`). Raw HTML in the text passes
//! through, as it does on the site; a surface that shows untrusted text cleans
//! the result itself.

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
    let parser = Parser::new_ext(text, options);
    let mut out = String::with_capacity(text.len() * 2);
    html::push_html(&mut out, parser);
    out
}

#[cfg(test)]
mod tests {
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
    fn empty_text_renders_nothing() {
        assert_eq!(render(""), "");
        assert_eq!(render("   \n"), "");
    }
}

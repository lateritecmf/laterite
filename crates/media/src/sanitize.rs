//! What an upload may carry: the extension allow-list, and the cleaning of
//! the two text formats a browser would run as code.
//!
//! A file's bytes decide its type, never its name; but the name's extension
//! is still the one signal an operator reads, so a name outside the allow-list
//! is refused before the bytes are stored. An SVG is a document the browser
//! parses, with script and event handlers of its own, so it is cleaned to a
//! drawing on the way in; a stylesheet is cleaned of what cannot be allowed
//! in a `<style>`.

use std::collections::{HashMap, HashSet};

/// The extensions accepted when the deployment lists none: images, documents,
/// audio, video, fonts and archives. Scripts, markup and stylesheets are not
/// among them; a deployment that wants them lists them.
pub const DEFAULT_EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "gif", "webp", "avif", "bmp", "svg", "ico", "pdf", "txt", "csv", "md",
    "rtf", "odt", "doc", "docx", "ods", "xls", "xlsx", "odp", "ppt", "pptx", "ics", "epub", "mp3",
    "wav", "ogg", "m4a", "flac", "aac", "mp4", "webm", "mov", "m4v", "mpeg", "mpg", "mkv", "avi",
    "woff", "woff2", "ttf", "otf", "eot", "zip", "rar", "7z", "gz", "tar",
];

/// The extension of a file name, lowercased, or `None` when it has none.
pub fn extension(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (_, ext) = name.rsplit_once('.')?;
    if ext.is_empty() || ext.contains(char::is_whitespace) {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// Whether a file name's extension is in `allowed` (compared lowercased). A
/// name with no extension is never allowed.
pub fn extension_allowed<S: AsRef<str>>(name: &str, allowed: &[S]) -> bool {
    let Some(ext) = extension(name) else {
        return false;
    };
    allowed.iter().any(|a| {
        a.as_ref()
            .trim()
            .trim_start_matches('.')
            .eq_ignore_ascii_case(&ext)
    })
}

/// Whether the leading bytes are an SVG document: optional BOM, XML prolog,
/// comments and a doctype, then an `<svg` root.
pub fn is_svg(head: &[u8]) -> bool {
    // A chunk may end mid-character; the root element sits well before that.
    let text = String::from_utf8_lossy(head);
    let mut rest = text.trim_start_matches('\u{feff}').trim_start();
    loop {
        if rest.starts_with("<?") {
            let Some(end) = rest.find("?>") else {
                return false;
            };
            rest = rest[end + 2..].trim_start();
        } else if rest.starts_with("<!--") {
            let Some(end) = rest.find("-->") else {
                return false;
            };
            rest = rest[end + 3..].trim_start();
        } else if rest.starts_with("<!") {
            let Some(end) = rest.find('>') else {
                return false;
            };
            rest = rest[end + 1..].trim_start();
        } else {
            break;
        }
    }
    rest.len() >= 4
        && rest[..4].eq_ignore_ascii_case("<svg")
        && rest[4..]
            .chars()
            .next()
            .is_none_or(|c| c.is_whitespace() || c == '>' || c == '/')
}

const SVG_NS: &str = "http://www.w3.org/2000/svg";

/// Cleans an SVG to a drawing: the shape, paint, text, gradient, filter,
/// mask, clip, marker, pattern, symbol and animation elements stay; script,
/// foreign content, event handlers, external references and `javascript:`
/// URLs go. A `style` element or attribute keeps only what a stylesheet may
/// carry. `None` for bytes that are not an SVG document.
pub fn svg(bytes: &[u8]) -> Option<String> {
    if !is_svg(&bytes[..bytes.len().min(4096)]) {
        return None;
    }
    let text = std::str::from_utf8(bytes).ok()?;
    let mut builder = ammonia::Builder::default();
    builder
        .tags(ELEMENTS.iter().copied().collect::<HashSet<_>>())
        .generic_attributes(ATTRIBUTES.iter().copied().collect::<HashSet<_>>())
        .tag_attributes(HashMap::from([
            (
                "svg",
                HashSet::from(["xmlns", "xmlns:xlink", "version", "baseProfile"]),
            ),
            ("a", HashSet::from(["href", "target"])),
            ("use", HashSet::from(["href"])),
            ("image", HashSet::from(["href", "preserveAspectRatio"])),
            ("textPath", HashSet::from(["href"])),
            ("linearGradient", HashSet::from(["href"])),
            ("radialGradient", HashSet::from(["href"])),
            ("pattern", HashSet::from(["href"])),
            ("mpath", HashSet::from(["href"])),
        ]))
        .clean_content_tags(HashSet::from(["script", "foreignObject"]))
        .url_schemes(HashSet::from(["data"]))
        .url_relative(ammonia::UrlRelative::PassThrough)
        .link_rel(None)
        .strip_comments(true)
        .attribute_filter(|_, attribute, value| {
            if attribute == "href" {
                return safe_href(value).then(|| value.into());
            }
            if attribute == "style" {
                return Some(css_line(value).into());
            }
            Some(value.into())
        });
    let cleaned = builder.clean(text).to_string();
    let start = cleaned.find("<svg")?;
    let cleaned = &cleaned[start..];
    let end = cleaned.rfind("</svg>")? + "</svg>".len();
    let mut cleaned = clean_style_elements(&cleaned[..end]);
    // A standalone file is parsed as XML, which needs the namespace the
    // fragment parser dropped or the author never wrote.
    let root_end = cleaned.find('>').unwrap_or(0);
    if !cleaned[..root_end].contains("xmlns=") {
        cleaned.insert_str(4, &format!(" xmlns=\"{SVG_NS}\""));
    }
    Some(format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n{cleaned}\n"
    ))
}

/// A reference is safe when it points inside the document or carries an
/// image inline.
fn safe_href(value: &str) -> bool {
    let v = value.trim();
    v.starts_with('#') || v.to_ascii_lowercase().starts_with("data:image/")
}

/// The text of every `style` element cleaned like a stylesheet.
fn clean_style_elements(svg: &str) -> String {
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    while let Some(open) = rest.find("<style") {
        let Some(tag_end) = rest[open..].find('>') else {
            break;
        };
        let body_start = open + tag_end + 1;
        let Some(close) = rest[body_start..].find("</style>") else {
            break;
        };
        out.push_str(&rest[..body_start]);
        out.push_str(&css(&rest[body_start..body_start + close]));
        rest = &rest[body_start + close..];
    }
    out.push_str(rest);
    out
}

/// A stylesheet with what cannot be allowed in a `<style>` removed, rule by
/// rule: imports, script URLs, legacy expressions and bindings, and a closing
/// tag that would end the element. Capped at 64 KiB.
pub fn css(css: &str) -> String {
    const CAP: usize = 65536;
    let mut out = String::with_capacity(css.len().min(CAP));
    let push = |out: &mut String, text: &str| {
        if out.len() + text.len() <= CAP {
            out.push_str(text);
        }
    };
    for chunk in css.split_inclusive('}').take(4000) {
        let (prelude, rule) = match chunk.find('{') {
            Some(i) => (&chunk[..i], &chunk[i..]),
            None => (chunk, ""),
        };
        // Statements before a block (`@import url(x);`) each end with a
        // semicolon; what remains is the block's selector.
        let mut parts: Vec<&str> = prelude.split_inclusive(';').collect();
        let selector = if prelude.ends_with(';') {
            ""
        } else {
            parts.pop().unwrap_or("")
        };
        for statement in parts {
            if css_line_allowed(statement) {
                push(&mut out, statement);
            }
        }
        if css_line_allowed(selector) && css_line_allowed(rule) {
            push(&mut out, selector);
            push(&mut out, rule);
        }
    }
    out.trim().to_string()
}

/// One declaration list (a `style` attribute) cleaned the same way.
fn css_line(value: &str) -> String {
    if css_line_allowed(value) && !value.to_ascii_lowercase().contains("url(") {
        value.to_string()
    } else {
        String::new()
    }
}

fn css_line_allowed(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    !(lower.contains("@import")
        || lower.contains("expression(")
        || lower.contains("javascript:")
        || lower.contains("-moz-binding")
        || lower.contains("behavior:")
        || lower.contains("</")
        || lower.contains("&lt;/"))
}

/// The drawing vocabulary: everything a renderer draws, nothing it runs.
const ELEMENTS: &[&str] = &[
    "svg",
    "a",
    "circle",
    "clipPath",
    "defs",
    "desc",
    "ellipse",
    "feBlend",
    "feColorMatrix",
    "feComponentTransfer",
    "feComposite",
    "feConvolveMatrix",
    "feDiffuseLighting",
    "feDisplacementMap",
    "feDistantLight",
    "feDropShadow",
    "feFlood",
    "feFuncA",
    "feFuncB",
    "feFuncG",
    "feFuncR",
    "feGaussianBlur",
    "feImage",
    "feMerge",
    "feMergeNode",
    "feMorphology",
    "feOffset",
    "fePointLight",
    "feSpecularLighting",
    "feSpotLight",
    "feTile",
    "feTurbulence",
    "filter",
    "g",
    "image",
    "line",
    "linearGradient",
    "marker",
    "mask",
    "metadata",
    "path",
    "pattern",
    "polygon",
    "polyline",
    "radialGradient",
    "rect",
    "stop",
    "style",
    "switch",
    "symbol",
    "text",
    "textPath",
    "title",
    "tspan",
    "use",
    "view",
];

/// Presentation and geometry attributes, on any element.
const ATTRIBUTES: &[&str] = &[
    "id",
    "class",
    "style",
    "lang",
    "tabindex",
    "role",
    "aria-label",
    "aria-labelledby",
    "aria-hidden",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "dx",
    "dy",
    "fx",
    "fy",
    "fr",
    "width",
    "height",
    "viewBox",
    "preserveAspectRatio",
    "d",
    "points",
    "pathLength",
    "transform",
    "transform-origin",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "color",
    "visibility",
    "display",
    "overflow",
    "clip-path",
    "clip-rule",
    "clipPathUnits",
    "mask",
    "maskUnits",
    "maskContentUnits",
    "filter",
    "filterUnits",
    "primitiveUnits",
    "marker-start",
    "marker-mid",
    "marker-end",
    "markerWidth",
    "markerHeight",
    "markerUnits",
    "refX",
    "refY",
    "orient",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "patternUnits",
    "patternContentUnits",
    "patternTransform",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "font-variant",
    "letter-spacing",
    "word-spacing",
    "text-anchor",
    "text-decoration",
    "dominant-baseline",
    "alignment-baseline",
    "baseline-shift",
    "writing-mode",
    "direction",
    "textLength",
    "lengthAdjust",
    "startOffset",
    "method",
    "spacing",
    "in",
    "in2",
    "result",
    "mode",
    "type",
    "values",
    "stdDeviation",
    "flood-color",
    "flood-opacity",
    "lighting-color",
    "operator",
    "k1",
    "k2",
    "k3",
    "k4",
    "scale",
    "radius",
    "edgeMode",
    "paint-order",
    "vector-effect",
    "shape-rendering",
    "image-rendering",
    "color-interpolation-filters",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_extension_is_matched_lowercased_and_a_bare_name_is_refused() {
        let allowed = ["png", "PDF"];
        assert!(extension_allowed("Photo.PNG", &allowed));
        assert!(extension_allowed("report.pdf", &allowed));
        assert!(!extension_allowed("script.exe", &allowed));
        assert!(!extension_allowed("README", &allowed));
        assert!(!extension_allowed("archive.tar.gz", &allowed));
        assert!(extension_allowed("archive.tar.gz", DEFAULT_EXTENSIONS));
        assert!(!extension_allowed("page.html", DEFAULT_EXTENSIONS));
        assert!(!extension_allowed("app.js", DEFAULT_EXTENSIONS));
    }

    #[test]
    fn an_svg_is_recognised_past_its_prolog_and_doctype() {
        assert!(is_svg(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"));
        assert!(is_svg(
            b"\xEF\xBB\xBF<?xml version=\"1.0\"?>\n<!-- made -->\n<!DOCTYPE svg PUBLIC \"-//W3C//DTD SVG 1.1//EN\" \"x.dtd\">\n<SVG viewBox=\"0 0 1 1\"/>"
        ));
        assert!(!is_svg(b"<svgfoo>"));
        assert!(!is_svg(b"<html><svg></svg></html>"));
        assert!(!is_svg(b"\x89PNG"));
    }

    #[test]
    fn a_script_bearing_svg_becomes_a_drawing() {
        let dirty = r##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" viewBox="0 0 10 10" onload="alert(1)">
  <script>alert(1)</script>
  <style>.a{fill:red} @import url(evil.css); .b{background:url(javascript:alert(1))}</style>
  <defs><linearGradient id="g"><stop offset="0" stop-color="#fff"/></linearGradient></defs>
  <foreignObject><body xmlns="http://www.w3.org/1999/xhtml"><iframe src="x"></iframe></body></foreignObject>
  <a href="javascript:alert(1)"><rect width="10" height="10" fill="url(#g)" onclick="x()" style="fill:url(http://x/y);stroke:red"/></a>
  <use xlink:href="http://evil/x.svg#a"/><use href="#ok"/>
  <image href="data:image/png;base64,AAAA" width="1" height="1"/>
  <image href="https://evil/track.png" width="1" height="1"/>
  <text x="1" y="1">Hi &amp; bye</text>
</svg>"##;
        let clean = svg(dirty.as_bytes()).expect("an svg");
        assert!(clean.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg"));
        for gone in [
            "<script",
            "alert(",
            "onload",
            "onclick",
            "foreignObject",
            "iframe",
            "@import",
            "javascript:",
            "http://evil",
            "https://evil",
            "url(http",
        ] {
            assert!(!clean.contains(gone), "{gone} survived: {clean}");
        }
        for kept in [
            "xmlns=\"http://www.w3.org/2000/svg\"",
            "viewBox=\"0 0 10 10\"",
            "<linearGradient id=\"g\"",
            "stop-color=\"#fff\"",
            "fill=\"url(#g)\"",
            "href=\"#ok\"",
            "href=\"data:image/png;base64,AAAA\"",
            "Hi &amp; bye",
            ".a{fill:red}",
        ] {
            assert!(clean.contains(kept), "{kept} lost: {clean}");
        }
        assert!(!clean.contains("<style>.a{fill:red} @import"));
    }

    #[test]
    fn a_namespace_is_added_when_the_author_left_it_out() {
        let clean = svg(b"<svg viewBox=\"0 0 1 1\"><circle r=\"1\"/></svg>").unwrap();
        assert!(
            clean.contains("<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox"),
            "{clean}"
        );
        assert!(clean.contains("<circle r=\"1\"></circle>"));
        assert!(svg(b"<html></html>").is_none());
        assert!(svg(b"\x89PNG").is_none());
    }

    #[test]
    fn a_stylesheet_loses_imports_scripts_and_closing_tags() {
        let dirty = ".lat-topnav{background:#123}\n@import url(evil.css);\na{behavior:url(x.htc)}\n</style><script>1</script>\n.x{background:url(javascript:alert(1))}\n.ok{color:red}";
        assert_eq!(css(dirty), ".lat-topnav{background:#123}\n.ok{color:red}");
    }
}

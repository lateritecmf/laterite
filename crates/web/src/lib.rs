//! The Laterite public web layer.
//!
//! Where [`laterite_admin`](https://docs.rs/laterite-admin) renders the private
//! admin, this crate renders the public face of an application. Its first
//! capability is **static-site generation**: an application renders its pages to
//! HTML strings and hands them to a [`StaticSite`], which writes them as files,
//! and copies static assets. The output is a plain directory suitable for any
//! static host or CDN.
//!
//! It writes **no `robots.txt` and no `sitemap.xml`**. How a site wants to be
//! crawled, and which of its URLs it advertises at what priority, are the
//! application's decisions, not the framework's. What this crate offers is the
//! material for them: [`StaticSite::paths`] reports every page written, and
//! [`StaticSite::file`] writes whatever the site decides to publish.
//!
//! Page templates live in the application (Askama, or any renderer that produces
//! a `String`); this crate owns the file layout, the sitemap, and the shared
//! [`Meta`] tags (title, description, canonical URL, Open Graph) so pages stay
//! consistent and shareable.
//!
//! ```no_run
//! use laterite_web::{Meta, StaticSite};
//!
//! # fn render_home(head: &str) -> String { String::new() }
//! # fn main() -> std::io::Result<()> {
//! let meta = Meta::new("Acme", "The Acme website.").canonical("https://acme.example/");
//! let home = render_home(&meta.head_tags());
//!
//! let mut site = StaticSite::new("dist", "https://acme.example")?;
//! site.page("/", &home)?;
//! site.assets("static", "static")?;
//!
//! // What the site publishes about itself is its own decision.
//! site.file("robots.txt", "User-agent: *\nAllow: /\n")?;
//! # Ok(()) }
//! ```

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Page metadata rendered into a document's `<head>`: the title, description,
/// optional canonical URL, and optional social-share image. [`Meta::head_tags`]
/// renders the standard `<title>`, `<meta>`, and Open Graph tags so every page is
/// described and shareable the same way.
#[derive(Debug, Clone)]
pub struct Meta {
    title: String,
    description: String,
    canonical: Option<String>,
    image: Option<String>,
}

impl Meta {
    /// A page's title and description. Both appear in search results and social
    /// cards, so keep them human and specific.
    pub fn new(title: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            description: description.into(),
            canonical: None,
            image: None,
        }
    }

    /// The canonical absolute URL for this page (also used as the Open Graph URL).
    pub fn canonical(mut self, url: impl Into<String>) -> Self {
        self.canonical = Some(url.into());
        self
    }

    /// The absolute URL of the social-share image (Open Graph / Twitter card).
    pub fn image(mut self, url: impl Into<String>) -> Self {
        self.image = Some(url.into());
        self
    }

    /// Renders the `<head>` tags for this page: title, description, canonical
    /// link, and Open Graph / Twitter card metadata. Embed the result inside the
    /// document `<head>`. All values are HTML-attribute escaped.
    pub fn head_tags(&self) -> String {
        let title = escape(&self.title);
        let description = escape(&self.description);
        let mut out = format!(
            "<title>{title}</title>\n\
             <meta name=\"description\" content=\"{description}\">\n\
             <meta property=\"og:title\" content=\"{title}\">\n\
             <meta property=\"og:description\" content=\"{description}\">\n\
             <meta property=\"og:type\" content=\"website\">\n\
             <meta name=\"twitter:card\" content=\"summary_large_image\">"
        );
        if let Some(url) = &self.canonical {
            let url = escape(url);
            out.push_str(&format!(
                "\n<link rel=\"canonical\" href=\"{url}\">\n\
                 <meta property=\"og:url\" content=\"{url}\">"
            ));
        }
        if let Some(image) = &self.image {
            let image = escape(image);
            out.push_str(&format!(
                "\n<meta property=\"og:image\" content=\"{image}\">"
            ));
        }
        out
    }
}

/// A static site being assembled into an output directory. Pages are written as
/// `index.html` under a directory per path (so URLs are clean and extensionless),
/// and static assets are copied verbatim.
pub struct StaticSite {
    out: PathBuf,
    base_url: String,
    paths: Vec<String>,
}

impl StaticSite {
    /// Prepares an output directory and records the site's base URL, for example
    /// `https://acme.example`. The base URL is what [`Meta::canonical`] and any
    /// absolute link the site builds are formed against.
    pub fn new(out_dir: impl Into<PathBuf>, base_url: impl Into<String>) -> io::Result<Self> {
        let out = out_dir.into();
        fs::create_dir_all(&out)?;
        Ok(Self {
            out,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            paths: Vec::new(),
        })
    }

    /// Writes a page's rendered `html` for the URL `path`. The path maps to a
    /// clean-URL file: `/` becomes `index.html`, `/features/` becomes
    /// `features/index.html`. The path is recorded, and [`paths`](Self::paths)
    /// reports it.
    pub fn page(&mut self, path: &str, html: &str) -> io::Result<&mut Self> {
        let file = self.out.join(path_to_file(path));
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(file, html)?;
        self.paths.push(normalise_path(path));
        Ok(self)
    }

    /// Copies a directory of static assets (CSS, fonts, images) from `from` into
    /// the output at `to`, recursively.
    pub fn assets(&self, from: impl AsRef<Path>, to: &str) -> io::Result<()> {
        copy_dir(from.as_ref(), &self.out.join(to))
    }

    /// Writes a file verbatim at `path`, relative to the output root.
    ///
    /// For the files a site decides for itself and a generator should not invent:
    /// `robots.txt`, `humans.txt`, `llms.txt`, a `CNAME`, a verification token.
    /// Unlike [`page`](Self::page) the contents are written as given, with no
    /// clean-URL mapping, and the path is not added to the sitemap.
    ///
    /// The path is confined to the output directory: a leading slash is trimmed
    /// and a `..` segment is refused, so a caller cannot write outside `dist`.
    pub fn file(&self, path: &str, contents: &str) -> io::Result<()> {
        let relative = path.trim_start_matches('/');
        if relative.split('/').any(|part| part == "..") {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("`{path}` climbs out of the output directory"),
            ));
        }
        let target = self.out.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(target, contents)
    }

    /// The site's base URL, without a trailing slash. Paired with
    /// [`paths`](Self::paths) it gives the absolute URL of every page written,
    /// which is what a sitemap is built from.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Every page path written so far, normalised, in the order they were added.
    ///
    /// This is the material a sitemap is built from, without this crate deciding
    /// what the sitemap says. Which URLs a site advertises, at what priority and
    /// change frequency, is the site's own policy.
    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

/// Maps a URL path to its output file: `/` and `""` to `index.html`, and any
/// other path to `<path>/index.html` for clean, extensionless URLs.
fn path_to_file(path: &str) -> PathBuf {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        PathBuf::from("index.html")
    } else {
        PathBuf::from(trimmed).join("index.html")
    }
}

/// Normalises a URL path for the sitemap: leading slash, trailing slash for
/// directories, and `/` for the root.
fn normalise_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{trimmed}/")
    }
}

/// Escapes a string for use in HTML text or a double-quoted attribute.
fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_map_to_clean_url_files() {
        assert_eq!(path_to_file("/"), PathBuf::from("index.html"));
        assert_eq!(path_to_file(""), PathBuf::from("index.html"));
        assert_eq!(
            path_to_file("/features/"),
            PathBuf::from("features/index.html")
        );
        assert_eq!(
            path_to_file("/get-started"),
            PathBuf::from("get-started/index.html")
        );
    }

    #[test]
    fn head_tags_include_and_escape_metadata() {
        let meta = Meta::new("Acme & Co", "A \"great\" site")
            .canonical("https://acme.example/")
            .image("https://acme.example/card.png");
        let tags = meta.head_tags();
        assert!(tags.contains("<title>Acme &amp; Co</title>"));
        assert!(tags.contains("content=\"A &quot;great&quot; site\""));
        assert!(tags.contains("rel=\"canonical\" href=\"https://acme.example/\""));
        assert!(tags.contains("og:image"));
    }

    #[test]
    fn builds_pages_and_copies_assets() {
        let tmp = tempfile::tempdir().unwrap();
        let assets = tmp.path().join("static");
        fs::create_dir_all(&assets).unwrap();
        fs::write(assets.join("site.css"), "body{}").unwrap();

        let out = tmp.path().join("dist");
        let mut site = StaticSite::new(&out, "https://acme.example/").unwrap();
        site.page("/", "<h1>Home</h1>").unwrap();
        site.page("/features/", "<h1>Features</h1>").unwrap();
        site.assets(&assets, "static").unwrap();

        assert_eq!(
            fs::read_to_string(out.join("index.html")).unwrap(),
            "<h1>Home</h1>"
        );
        assert!(out.join("features/index.html").exists());
        assert!(out.join("static/site.css").exists());

        // Neither file is invented here: what a site advertises and what it
        // allows are its own decisions.
        assert!(!out.join("sitemap.xml").exists());
        assert!(!out.join("robots.txt").exists());

        // What it does offer is the material: every page it wrote, and the base
        // URL to form them against.
        assert_eq!(site.paths(), ["/", "/features/"]);
        assert_eq!(site.base_url(), "https://acme.example");
    }

    #[test]
    fn a_site_writes_its_own_root_files() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("dist");
        let site = StaticSite::new(&out, "https://acme.example").unwrap();

        site.file("robots.txt", "User-agent: *\nDisallow: /private/\n")
            .unwrap();
        // A leading slash is the same path, and nesting creates its directory.
        site.file(
            "/.well-known/security.txt",
            "Contact: mailto:x@acme.example\n",
        )
        .unwrap();

        let robots = fs::read_to_string(out.join("robots.txt")).unwrap();
        assert!(robots.contains("Disallow: /private/"));
        assert!(out.join(".well-known/security.txt").exists());

        // Written verbatim, and never counted as a page.
        assert!(site.paths().is_empty());
    }

    #[test]
    fn a_root_file_cannot_escape_the_output_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let out = tmp.path().join("dist");
        let site = StaticSite::new(&out, "https://acme.example").unwrap();
        assert!(site.file("../escaped.txt", "no").is_err());
        assert!(site.file("a/../../escaped.txt", "no").is_err());
        assert!(!tmp.path().join("escaped.txt").exists());
    }
}

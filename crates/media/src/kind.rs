//! What a file is to a browser, from its sniffed content type.

use std::fmt;

/// The broad class of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// A content type under `image/`.
    Image,
    /// A content type under `video/`.
    Video,
    /// A content type under `audio/`.
    Audio,
    /// A PDF, text, word-processing, spreadsheet, presentation, RTF or EPUB file.
    Document,
    /// A TTF, OTF, TTC, WOFF or WOFF2 font.
    Font,
    /// Any content type the other kinds do not cover.
    Other,
}

impl Kind {
    /// Every kind, in display order.
    pub const ALL: [Kind; 6] = [
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Document,
        Kind::Font,
        Kind::Other,
    ];

    /// The class of a content type.
    pub fn of(content_type: &str) -> Kind {
        let ct = content_type.trim().to_ascii_lowercase();
        if ct.starts_with("image/") {
            Kind::Image
        } else if ct.starts_with("video/") {
            Kind::Video
        } else if ct.starts_with("audio/") {
            Kind::Audio
        } else if ct == "application/pdf"
            || ct.starts_with("text/")
            || ct.contains("msword")
            || ct.contains("officedocument")
            || ct.contains("opendocument")
            || ct == "application/rtf"
            || ct == "application/epub+zip"
        {
            Kind::Document
        } else if ct.starts_with("font/")
            || ct.starts_with("application/font-")
            || ct.starts_with("application/x-font")
        {
            Kind::Font
        } else {
            Kind::Other
        }
    }

    /// The stored and query name.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Document => "document",
            Kind::Font => "font",
            Kind::Other => "other",
        }
    }

    /// The kind by its name.
    pub fn parse(name: &str) -> Option<Kind> {
        Kind::ALL.into_iter().find(|k| k.as_str() == name)
    }
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn content_types_classify() {
        assert_eq!(Kind::of("image/png"), Kind::Image);
        assert_eq!(Kind::of("IMAGE/svg+xml"), Kind::Image);
        assert_eq!(Kind::of("video/mp4"), Kind::Video);
        assert_eq!(Kind::of("audio/mpeg"), Kind::Audio);
        assert_eq!(Kind::of("application/pdf"), Kind::Document);
        assert_eq!(Kind::of("text/csv"), Kind::Document);
        assert_eq!(
            Kind::of("application/vnd.openxmlformats-officedocument.wordprocessingml.document"),
            Kind::Document
        );
        assert_eq!(Kind::of("font/ttf"), Kind::Font);
        assert_eq!(Kind::of("font/woff2"), Kind::Font);
        assert_eq!(Kind::of("application/font-sfnt"), Kind::Font);
        assert_eq!(Kind::of("application/zip"), Kind::Other);
        assert_eq!(Kind::parse("font"), Some(Kind::Font));
        assert_eq!(Kind::parse("video"), Some(Kind::Video));
        assert_eq!(Kind::parse("thing"), None);
    }
}

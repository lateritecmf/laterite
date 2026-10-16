//! What a file is to a browser, from its sniffed content type.

use std::fmt;

/// The broad class of a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Image,
    Video,
    Audio,
    Document,
    Other,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Image,
        Kind::Video,
        Kind::Audio,
        Kind::Document,
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
        assert_eq!(Kind::of("application/zip"), Kind::Other);
        assert_eq!(Kind::parse("video"), Some(Kind::Video));
        assert_eq!(Kind::parse("thing"), None);
    }
}

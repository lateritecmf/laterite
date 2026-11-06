//! Presets: the named sizes a variant may be made in. A URL names a preset,
//! never a width, so the set of derivatives a file can have is the set an
//! author declared.

use std::collections::HashMap;

/// How a source is fitted to a preset's box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Inside the box, keeping the aspect ratio; a smaller source stays its size.
    #[default]
    Fit,
    /// Filling the box, keeping the aspect ratio, cropped from the centre.
    Crop,
    /// Exactly the box, stretched when the aspect ratio differs.
    Exact,
}

/// The encoding a variant is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    /// JPEG for an opaque image, PNG for one with transparency.
    #[default]
    Auto,
    /// JPEG at the preset's quality, with any transparency flattened.
    Jpeg,
    /// Lossless PNG.
    Png,
    /// Lossless WebP.
    WebP,
}

/// A named derivative: register one on the application's `Registry` to make
/// it available.
///
/// ```ignore
/// registry.add(Preset::new("card").width(600).height(400).crop());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// The name a URL uses to ask for this preset, such as `thumb`.
    pub name: String,
    /// The box's width; unset, the height alone bounds the image.
    pub width: Option<u32>,
    /// The box's height; unset, the width alone bounds the image.
    pub height: Option<u32>,
    /// How the source is fitted to the box.
    pub mode: Mode,
    /// The encoding the variant is written in.
    pub format: Format,
    /// JPEG quality, 1 to 100.
    pub quality: u8,
}

impl Preset {
    /// Returns a preset named `name` with no box, [`Mode::Fit`], [`Format::Auto`] and quality 82.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            width: None,
            height: None,
            mode: Mode::Fit,
            format: Format::Auto,
            quality: 82,
        }
    }

    /// Sets the box's width in pixels.
    pub fn width(mut self, width: u32) -> Self {
        self.width = Some(width);
        self
    }

    /// Sets the box's height in pixels.
    pub fn height(mut self, height: u32) -> Self {
        self.height = Some(height);
        self
    }

    /// Sets the mode to [`Mode::Fit`].
    pub fn fit(mut self) -> Self {
        self.mode = Mode::Fit;
        self
    }

    /// Sets the mode to [`Mode::Crop`].
    pub fn crop(mut self) -> Self {
        self.mode = Mode::Crop;
        self
    }

    /// Sets the mode to [`Mode::Exact`].
    pub fn exact(mut self) -> Self {
        self.mode = Mode::Exact;
        self
    }

    /// Sets the encoding the variant is written in.
    pub fn format(mut self, format: Format) -> Self {
        self.format = format;
        self
    }

    /// Sets the JPEG quality, clamped to 1 to 100.
    pub fn quality(mut self, quality: u8) -> Self {
        self.quality = quality.clamp(1, 100);
        self
    }
}

/// The name the admin's own thumbnails use.
pub const THUMB: &str = "thumb";
/// The name the admin's file page uses for its large preview.
pub const PREVIEW: &str = "preview";

/// The presets the framework declares for its own screens.
pub fn builtin() -> Vec<Preset> {
    vec![
        Preset::new(THUMB).width(320).height(320),
        Preset::new(PREVIEW).width(1200).height(1200),
    ]
}

/// Every preset by name: the built-in ones, then each in `registered` (the
/// application's contributions), a registered one replacing a built-in of
/// the same name.
pub fn all(registered: impl IntoIterator<Item = Preset>) -> HashMap<String, Preset> {
    builtin()
        .into_iter()
        .chain(registered)
        .map(|p| (p.name.clone(), p))
        .collect()
}

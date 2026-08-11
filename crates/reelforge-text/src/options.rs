//! Text clip configuration.

use reelforge_core::{Duration, Rgba8, Size};

/// Special `font_path` value that selects the built-in bitmap face.
pub const BITMAP_FONT: &str = "bitmap";

/// Parameters for a title / text clip.
#[derive(Debug, Clone)]
pub struct TextClipOptions {
    /// UTF-8 text to render (`\n` starts a new line).
    pub text: String,
    /// Path to a `.ttf` / `.otf` file, or [`BITMAP_FONT`] for the built-in face.
    pub font_path: String,
    /// Font size in pixels (height of the capital box for bitmap; px for TrueType).
    pub font_size: u32,
    /// Fill color including alpha.
    pub color: Rgba8,
    /// Optional explicit raster size; otherwise content-sized with padding.
    pub size: Option<Size>,
    /// Padding around content when auto-sizing (default 4).
    pub padding: u32,
    /// Clip duration on the timeline.
    pub duration: Duration,
}

impl TextClipOptions {
    /// Build options with required fields (bitmap font by default).
    #[must_use]
    pub fn new(text: impl Into<String>, font_size: u32, duration: Duration) -> Self {
        Self {
            text: text.into(),
            font_path: BITMAP_FONT.to_string(),
            font_size,
            color: Rgba8::WHITE,
            size: None,
            padding: 4,
            duration,
        }
    }

    /// Use a TrueType/OpenType font file.
    #[must_use]
    pub fn with_font_path(mut self, font_path: impl Into<String>) -> Self {
        self.font_path = font_path.into();
        self
    }

    /// Set fill color.
    #[must_use]
    pub fn with_color(mut self, color: Rgba8) -> Self {
        self.color = color;
        self
    }

    /// Force output size.
    #[must_use]
    pub fn with_size(mut self, size: Size) -> Self {
        self.size = Some(size);
        self
    }

    /// Content padding when auto-sizing.
    #[must_use]
    pub fn with_padding(mut self, padding: u32) -> Self {
        self.padding = padding;
        self
    }
}

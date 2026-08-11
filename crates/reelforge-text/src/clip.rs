//! [`TextClip`] — static rasterized title as a video source.

use crate::error::{Result, TextError};
use crate::font::load_face;
use crate::layout::{layout_text, resolve_canvas};
use crate::options::TextClipOptions;
use crate::raster::rasterize_rgba;
use reelforge_core::{CoreError, Duration, Frame, Mask, Size, Time, VideoClip};

/// Video clip that shows a rendered text frame for its full duration.
#[derive(Debug, Clone)]
pub struct TextClip {
    frame: Frame,
    mask: Mask,
    duration: Duration,
    fps: Option<f64>,
}

impl TextClip {
    /// Build and rasterize a text clip from options.
    ///
    /// # Errors
    ///
    /// Returns font, layout, or timing errors.
    pub fn new(options: &TextClipOptions) -> Result<Self> {
        if !options.duration.is_positive() {
            return Err(TextError::message("text clip duration must be > 0"));
        }
        let face = load_face(&options.font_path, options.font_size)?;
        let layout = layout_text(&face, &options.text)?;
        let canvas = resolve_canvas(&layout, options.size, options.padding)?;
        let (frame, mask) = rasterize_rgba(&layout, canvas, options.padding, options.color)?;
        Ok(Self {
            frame,
            mask,
            duration: options.duration,
            fps: None,
        })
    }

    /// Attach nominal FPS for writers.
    #[must_use]
    pub fn with_fps(mut self, fps: f64) -> Self {
        self.fps = Some(fps);
        self
    }

    /// Borrow the rendered frame.
    #[must_use]
    pub fn frame(&self) -> &Frame {
        &self.frame
    }

    /// Borrow the coverage mask.
    #[must_use]
    pub fn mask(&self) -> &Mask {
        &self.mask
    }
}

impl VideoClip for TextClip {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.frame.size()
    }

    fn fps(&self) -> Option<f64> {
        self.fps
    }

    fn frame_at(&self, t: Time) -> reelforge_core::Result<Frame> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        Ok(self.frame.clone())
    }

    fn mask_at(&self, t: Time) -> reelforge_core::Result<Option<Mask>> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        Ok(Some(self.mask.clone()))
    }
}

/// Construct a [`TextClip`] from options (convenience).
///
/// # Errors
///
/// Propagates [`TextClip::new`] errors.
pub fn text_clip(options: &TextClipOptions) -> Result<TextClip> {
    TextClip::new(options)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::options::{BITMAP_FONT, TextClipOptions};
    use reelforge_core::{Rgb8, Rgba8};

    #[test]
    fn text_clip_samples() {
        let opts = TextClipOptions::new("Hi", 14, Duration::from_secs(1.0))
            .with_font_path(BITMAP_FONT)
            .with_color(Rgba8::from(Rgb8::WHITE));
        let clip = TextClip::new(&opts).unwrap();
        assert!(clip.size().width > 0);
        let f = clip.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.size(), clip.size());
        let m = clip.mask_at(Time::ZERO).unwrap().unwrap();
        assert!(m.data().iter().any(|&c| c > 0.0));
    }

    #[test]
    fn rejects_empty_text() {
        let opts = TextClipOptions::new("", 10, Duration::from_secs(1.0));
        assert!(TextClip::new(&opts).is_err());
    }
}

//! Add a solid margin around frames.

use reelforge_core::{
    CoreError, Duration, Frame, FrameFormat, Result, Rgb8, Size, Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Pad the clip with a uniform margin.
#[derive(Debug, Clone, Copy)]
pub struct Margin {
    /// Pixels on every side.
    pub size: u32,
    /// Fill color of the margin.
    pub color: Rgb8,
}

impl Margin {
    /// Uniform margin filled with black.
    #[must_use]
    pub fn new(size: u32) -> Self {
        Self {
            size,
            color: Rgb8::BLACK,
        }
    }

    /// Uniform margin with custom color.
    #[must_use]
    pub fn with_color(size: u32, color: Rgb8) -> Self {
        Self { size, color }
    }
}

impl VideoEffect for Margin {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if self.size == 0 {
            return Err(CoreError::invalid_frame("margin size must be > 0"));
        }
        Ok(Arc::new(MarginedVideo {
            inner: clip,
            pad: self.size,
            color: self.color,
        }))
    }
}

struct MarginedVideo {
    inner: Arc<dyn VideoClip>,
    pad: u32,
    color: Rgb8,
}

impl VideoClip for MarginedVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        let s = self.inner.size();
        Size::new(
            s.width.saturating_add(self.pad.saturating_mul(2)),
            s.height.saturating_add(self.pad.saturating_mul(2)),
        )
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let src = self.inner.frame_at(t)?;
        pad_frame(&src, self.pad, self.color)
    }
}

fn pad_frame(src: &Frame, pad: u32, color: Rgb8) -> Result<Frame> {
    let out_size = Size::new(
        src.size().width.saturating_add(pad.saturating_mul(2)),
        src.size().height.saturating_add(pad.saturating_mul(2)),
    );
    out_size.require_positive()?;
    // Canvas always RGB for simple margin fill; convert source if needed.
    let mut canvas = Frame::solid_rgb(out_size, color)?;
    let bpp_src = src.format().bytes_per_pixel();
    let sw = src.size().width as usize;
    let sh = src.size().height as usize;
    let dw = out_size.width as usize;
    let pad_u = pad as usize;
    let src_data = src.data();
    let dst = canvas.data_mut();

    for y in 0..sh {
        for x in 0..sw {
            let si = (y * sw + x) * bpp_src;
            let di = ((y + pad_u) * dw + (x + pad_u)) * 3;
            match src.format() {
                FrameFormat::Rgb8 | FrameFormat::Rgba8 => {
                    dst[di] = src_data[si];
                    dst[di + 1] = src_data[si + 1];
                    dst[di + 2] = src_data[si + 2];
                }
            }
        }
    }
    Ok(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::ColorClip;

    #[test]
    fn margin_grows_size() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(10, 8),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let out = Margin::new(2).apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(14, 12));
    }
}

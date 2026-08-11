//! Posterize / painting-style color quantization.

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use rayon::prelude::*;
use std::sync::Arc;

/// Reduce color detail toward a painted look (quantize + optional saturation boost).
#[derive(Debug, Clone, Copy)]
pub struct Painting {
    /// Saturation multiplier (`1.0` = unchanged).
    pub saturation: f32,
    /// Quantization levels per channel (`2`–`32` typical; lower = more posterized).
    pub levels: u8,
}

impl Painting {
    /// Default painting look.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            saturation: 1.4,
            levels: 8,
        }
    }

    /// Custom saturation and level count.
    #[must_use]
    pub const fn with(saturation: f32, levels: u8) -> Self {
        Self { saturation, levels }
    }
}

impl Default for Painting {
    fn default() -> Self {
        Self::new()
    }
}

impl VideoEffect for Painting {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        let levels = self.levels.max(2);
        Ok(Arc::new(PaintVideo {
            inner: clip,
            saturation: self.saturation.max(0.0),
            levels,
        }))
    }
}

struct PaintVideo {
    inner: Arc<dyn VideoClip>,
    saturation: f32,
    levels: u8,
}

impl VideoClip for PaintVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    #[allow(clippy::many_single_char_names)]
    fn frame_at(&self, t: Time) -> Result<Frame> {
        let mut frame = self.inner.frame_at(t)?;
        let sat = self.saturation;
        let levels = u32::from(self.levels);
        let bpp = frame.format().bytes_per_pixel();
        frame.data_mut().par_chunks_mut(bpp).for_each(|px| {
            let mut red = f32::from(px[0]);
            let mut green = f32::from(px[1]);
            let mut blue = f32::from(px[2]);
            // Boost saturation around luma.
            let luma = 0.299 * red + 0.587 * green + 0.114 * blue;
            red = luma + (red - luma) * sat;
            green = luma + (green - luma) * sat;
            blue = luma + (blue - luma) * sat;
            px[0] = quantize(red, levels);
            px[1] = quantize(green, levels);
            px[2] = quantize(blue, levels);
        });
        Ok(frame)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn quantize(v: f32, levels: u32) -> u8 {
    let v = v.clamp(0.0, 255.0);
    let step = 255.0 / f32::from(levels.saturating_sub(1).max(1) as u16);
    let q = (v / step).round() * step;
    q.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn paints() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::new(100, 50, 200),
            Duration::from_secs(0.5),
        ));
        let out = Painting::new().apply(clip).unwrap();
        assert_eq!(out.frame_at(Time::ZERO).unwrap().size(), Size::new(4, 4));
    }
}

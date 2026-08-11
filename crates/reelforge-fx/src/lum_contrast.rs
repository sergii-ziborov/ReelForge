//! Luminosity / contrast correction.

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use rayon::prelude::*;
use std::sync::Arc;

/// Adjust luminosity and contrast: `out = (in - 128) * contrast + 128 + lum`.
#[derive(Debug, Clone, Copy)]
pub struct LumContrast {
    /// Additive luminosity offset in ~[-255, 255].
    pub lum: f32,
    /// Contrast multiplier (`1.0` = unchanged).
    pub contrast: f32,
}

impl LumContrast {
    /// Construct with luminosity and contrast.
    #[must_use]
    pub const fn new(lum: f32, contrast: f32) -> Self {
        Self { lum, contrast }
    }
}

impl VideoEffect for LumContrast {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(LcVideo {
            inner: clip,
            lum: self.lum,
            contrast: self.contrast,
        }))
    }
}

struct LcVideo {
    inner: Arc<dyn VideoClip>,
    lum: f32,
    contrast: f32,
}

impl VideoClip for LcVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let mut frame = self.inner.frame_at(t)?;
        let lum = self.lum;
        let contrast = self.contrast;
        let bpp = frame.format().bytes_per_pixel();
        frame.data_mut().par_chunks_mut(bpp).for_each(|px| {
            for c in px.iter_mut().take(3) {
                let v = (f32::from(*c) - 128.0) * contrast + 128.0 + lum;
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                {
                    *c = v.round().clamp(0.0, 255.0) as u8;
                }
            }
        });
        Ok(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn identity_defaults() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::new(80, 80, 80),
            Duration::from_secs(0.5),
        ));
        let out = LumContrast::new(0.0, 1.0).apply(clip).unwrap();
        assert_eq!(out.frame_at(Time::ZERO).unwrap().data()[0], 80);
    }
}

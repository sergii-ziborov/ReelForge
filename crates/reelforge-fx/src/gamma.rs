//! Gamma correction effect.

use rayon::prelude::*;
use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Apply `out = in^gamma` per channel (linear-ish 0..1 space on 8-bit).
#[derive(Debug, Clone, Copy)]
pub struct GammaCorrection {
    /// Gamma exponent (`1.0` = unchanged; `<1` brightens midtones).
    pub gamma: f32,
}

impl GammaCorrection {
    /// Construct with the given gamma.
    #[must_use]
    pub const fn new(gamma: f32) -> Self {
        Self { gamma }
    }
}

impl VideoEffect for GammaCorrection {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(GammaVideo {
            inner: clip,
            gamma: self.gamma.max(1e-4),
        }))
    }
}

struct GammaVideo {
    inner: Arc<dyn VideoClip>,
    gamma: f32,
}

impl VideoClip for GammaVideo {
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
        let g = self.gamma;
        let bpp = frame.format().bytes_per_pixel();
        frame.data_mut().par_chunks_mut(bpp).for_each(|px| {
            for c in px.iter_mut().take(3) {
                let v = f32::from(*c) / 255.0;
                #[allow(
                    clippy::cast_possible_truncation,
                    clippy::cast_sign_loss,
                    clippy::cast_precision_loss
                )]
                {
                    *c = (v.powf(g) * 255.0).round().clamp(0.0, 255.0) as u8;
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
    fn gamma_one_identity() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::new(100, 100, 100),
            Duration::from_secs(0.5),
        ));
        let out = GammaCorrection::new(1.0).apply(clip).unwrap();
        let f = out.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.data()[0], 100);
    }
}

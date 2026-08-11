//! Color-domain video effects.

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Desaturate to grayscale (luma).
#[derive(Debug, Clone, Copy, Default)]
pub struct BlackAndWhite;

/// Invert RGB channels.
#[derive(Debug, Clone, Copy, Default)]
pub struct InvertColors;

/// Multiply RGB by a factor (`1.0` = unchanged).
#[derive(Debug, Clone, Copy)]
pub struct MultiplyColor {
    /// Linear scale factor applied to each channel.
    pub factor: f32,
}

impl MultiplyColor {
    /// Construct a color multiply effect.
    #[must_use]
    pub const fn new(factor: f32) -> Self {
        Self { factor }
    }
}

impl VideoEffect for BlackAndWhite {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MappedVideo {
            inner: clip,
            kind: ColorKind::Gray,
        }))
    }
}

impl VideoEffect for InvertColors {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MappedVideo {
            inner: clip,
            kind: ColorKind::Invert,
        }))
    }
}

impl VideoEffect for MultiplyColor {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MappedVideo {
            inner: clip,
            kind: ColorKind::Multiply(self.factor),
        }))
    }
}

#[derive(Clone, Copy)]
enum ColorKind {
    Gray,
    Invert,
    Multiply(f32),
}

struct MappedVideo {
    inner: Arc<dyn VideoClip>,
    kind: ColorKind,
}

impl VideoClip for MappedVideo {
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
        map_pixels(&mut frame, self.kind);
        Ok(frame)
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn map_pixels(frame: &mut Frame, kind: ColorKind) {
    let bpp = frame.format().bytes_per_pixel();
    for px in frame.data_mut().chunks_exact_mut(bpp) {
        match kind {
            ColorKind::Gray => {
                let y = (0.299 * f32::from(px[0])
                    + 0.587 * f32::from(px[1])
                    + 0.114 * f32::from(px[2]))
                .round()
                .clamp(0.0, 255.0) as u8;
                px[0] = y;
                px[1] = y;
                px[2] = y;
            }
            ColorKind::Invert => {
                px[0] = 255 - px[0];
                px[1] = 255 - px[1];
                px[2] = 255 - px[2];
            }
            ColorKind::Multiply(f) => {
                for c in px.iter_mut().take(3) {
                    *c = (f32::from(*c) * f).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn gray_equal_channels() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let out = BlackAndWhite.apply(clip).unwrap();
        let f = out.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.data()[0], f.data()[1]);
        assert_eq!(f.data()[1], f.data()[2]);
    }
}

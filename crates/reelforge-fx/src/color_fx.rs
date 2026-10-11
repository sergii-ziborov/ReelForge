//! Color-domain video effects.

use crate::raster::grayscale_in_place;
use rayon::prelude::*;
use reelforge_core::{
    AlphaMode, CoreError, Duration, Frame, Mask, Result, Size, Time, VideoClip, VideoEffect,
};
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
        if !self.factor.is_finite() {
            return Err(CoreError::invalid_frame("color multiplier must be finite"));
        }
        if self.factor.to_bits() == 1.0_f32.to_bits() {
            return Ok(clip);
        }
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

    fn sample_times(&self) -> Option<Vec<Time>> {
        self.inner.sample_times()
    }

    fn requires_frame_grid(&self) -> bool {
        self.inner.requires_frame_grid()
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        self.inner.mask_at(t)
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
    let premultiplied = frame.alpha_mode() == AlphaMode::Premultiplied;
    match kind {
        ColorKind::Gray => grayscale_in_place(frame),
        ColorKind::Invert => {
            let bpp = frame.format().bytes_per_pixel();
            frame.data_mut().par_chunks_mut(bpp).for_each(|px| {
                let ceiling = if premultiplied { px[3] } else { 255 };
                for channel in px.iter_mut().take(3) {
                    *channel = ceiling.saturating_sub(*channel);
                }
            });
        }
        ColorKind::Multiply(f) => {
            // Fixed-point scale: factor * 256
            let scale = (f.clamp(0.0, 16.0) * 256.0).round() as u32;
            let bpp = frame.format().bytes_per_pixel();
            frame.data_mut().par_chunks_mut(bpp).for_each(|px| {
                let ceiling = if premultiplied { u32::from(px[3]) } else { 255 };
                for c in px.iter_mut().take(3) {
                    let v = (u32::from(*c) * scale) >> 8;
                    *c = v.min(ceiling) as u8;
                }
            });
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

    #[test]
    fn invert_preserves_premultiplied_edges() {
        let mut frame = Frame::from_raw(
            Size::new(3, 1),
            reelforge_core::FrameFormat::Rgba8,
            vec![128, 0, 0, 128, 0, 0, 0, 0, 12, 64, 128, 255],
        )
        .unwrap()
        .with_alpha_mode(reelforge_core::AlphaMode::Premultiplied)
        .unwrap();
        map_pixels(&mut frame, ColorKind::Invert);
        assert_eq!(
            frame.data(),
            &[0, 128, 128, 128, 0, 0, 0, 0, 243, 191, 127, 255]
        );
        assert_eq!(frame.alpha_mode(), reelforge_core::AlphaMode::Premultiplied);
        map_pixels(&mut frame, ColorKind::Invert);
        assert_eq!(
            frame.data(),
            &[128, 0, 0, 128, 0, 0, 0, 0, 12, 64, 128, 255]
        );
    }

    #[test]
    fn multiply_clamps_to_premultiplied_alpha() {
        let mut frame =
            Frame::solid_rgba(Size::new(1, 1), reelforge_core::Rgba8::new(255, 64, 0, 128))
                .unwrap()
                .premultiply()
                .unwrap();
        map_pixels(&mut frame, ColorKind::Multiply(4.0));
        assert_eq!(frame.data(), &[128, 128, 0, 128]);
    }

    #[test]
    fn multiply_refuses_nonfinite_factors() {
        for factor in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let clip = Arc::new(ColorClip::new(
                Size::new(1, 1),
                Rgb8::RED,
                Duration::from_secs(1.0),
            ));
            assert!(MultiplyColor::new(factor).apply(clip).is_err());
        }
    }
}

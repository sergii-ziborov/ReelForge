//! Color-alpha semantics for packed RGBA (`Frame` / [`crate::VideoSurface`]).
//!
//! [`crate::Mask`] is **coverage**, not a color channel. Do not treat mask
//! samples as straight or premultiplied RGB alpha.

use crate::error::{CoreError, Result};
use crate::frame::{Frame, FrameFormat};
use crate::surface::PixelFormat;

/// How RGB samples relate to the alpha channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum AlphaMode {
    /// No alpha channel (`Rgb8`, YUV, …). RGB is display-referred as-is.
    #[default]
    Opaque,
    /// Straight (unassociated): `out = rgb * a + dst * (1 - a)`.
    Straight,
    /// Premultiplied (associated): RGB already scaled by `a`.
    Premultiplied,
}

impl AlphaMode {
    /// Default tag for a clip-graph [`FrameFormat`].
    #[must_use]
    pub const fn for_frame_format(format: FrameFormat) -> Self {
        match format {
            FrameFormat::Rgb8 => Self::Opaque,
            FrameFormat::Rgba8 => Self::Straight,
        }
    }

    /// Default tag for a [`PixelFormat`] (file surfaces: YUV is opaque).
    #[must_use]
    pub const fn for_pixel_format(format: PixelFormat) -> Self {
        match format {
            PixelFormat::Rgba8 | PixelFormat::Bgra8 => Self::Straight,
            PixelFormat::Rgb8 | PixelFormat::Yuv420p | PixelFormat::Nv12 => Self::Opaque,
        }
    }

    /// Whether this tag is valid for `format`.
    #[must_use]
    pub const fn is_valid_for_frame(self, format: FrameFormat) -> bool {
        match format {
            FrameFormat::Rgb8 => matches!(self, Self::Opaque),
            FrameFormat::Rgba8 => true,
        }
    }
}

impl Frame {
    /// Color-alpha tag. [`FrameFormat::Rgba8`] defaults to [`AlphaMode::Straight`].
    #[must_use]
    pub const fn alpha_mode(&self) -> AlphaMode {
        self.alpha
    }

    /// Relabel without converting pixels.
    ///
    /// # Errors
    ///
    /// [`AlphaMode::Straight`] / [`Premultiplied`][AlphaMode::Premultiplied] on RGB8.
    pub fn with_alpha_mode(mut self, mode: AlphaMode) -> Result<Self> {
        if !mode.is_valid_for_frame(self.format()) {
            return Err(CoreError::invalid_frame(format!(
                "alpha mode {mode:?} is invalid for {:?}",
                self.format()
            )));
        }
        self.alpha = mode;
        Ok(self)
    }

    /// Convert straight RGBA to premultiplied. No-op if already that mode.
    ///
    /// # Errors
    ///
    /// RGB8 cannot be premultiplied.
    pub fn premultiply(self) -> Result<Self> {
        match (self.format(), self.alpha) {
            (FrameFormat::Rgb8, _) => Err(CoreError::invalid_frame(
                "Rgb8 has no alpha channel to premultiply",
            )),
            (FrameFormat::Rgba8, AlphaMode::Premultiplied) => Ok(self),
            (FrameFormat::Rgba8, _) => {
                let mut frame = self;
                scale_rgba_by_alpha(frame.data_mut(), true);
                frame.alpha = AlphaMode::Premultiplied;
                Ok(frame)
            }
        }
    }

    /// Convert premultiplied RGBA to straight. No-op if already straight.
    ///
    /// # Errors
    ///
    /// RGB8 cannot be unpremultiplied.
    pub fn unpremultiply(self) -> Result<Self> {
        match (self.format(), self.alpha) {
            (FrameFormat::Rgb8, _) => Err(CoreError::invalid_frame(
                "Rgb8 has no alpha channel to unpremultiply",
            )),
            (FrameFormat::Rgba8, AlphaMode::Straight) => Ok(self),
            (FrameFormat::Rgba8, _) => {
                let mut frame = self;
                scale_rgba_by_alpha(frame.data_mut(), false);
                frame.alpha = AlphaMode::Straight;
                Ok(frame)
            }
        }
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss
)]
fn scale_rgba_by_alpha(data: &mut [u8], premultiply: bool) {
    for px in data.chunks_exact_mut(4) {
        let alpha = px[3];
        if premultiply {
            let a = u16::from(alpha);
            px[0] = ((u16::from(px[0]) * a + 127) / 255) as u8;
            px[1] = ((u16::from(px[1]) * a + 127) / 255) as u8;
            px[2] = ((u16::from(px[2]) * a + 127) / 255) as u8;
        } else if alpha == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        } else {
            let a = f32::from(alpha);
            px[0] = ((f32::from(px[0]) * 255.0 / a).round()).clamp(0.0, 255.0) as u8;
            px[1] = ((f32::from(px[1]) * 255.0 / a).round()).clamp(0.0, 255.0) as u8;
            px[2] = ((f32::from(px[2]) * 255.0 / a).round()).clamp(0.0, 255.0) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::Rgba8;
    use crate::layout::Size;

    #[test]
    fn rgba_defaults_to_straight() {
        let f = Frame::solid_rgba(Size::new(1, 1), Rgba8::new(255, 0, 0, 128)).unwrap();
        assert_eq!(f.alpha_mode(), AlphaMode::Straight);
    }

    #[test]
    fn premultiply_scales_rgb() {
        let f = Frame::solid_rgba(Size::new(1, 1), Rgba8::new(255, 0, 0, 128))
            .unwrap()
            .premultiply()
            .unwrap();
        assert_eq!(f.alpha_mode(), AlphaMode::Premultiplied);
        assert_eq!(f.data()[0], 128);
        assert_eq!(f.data()[3], 128);
    }

    #[test]
    fn unpremultiply_restores() {
        let f = Frame::solid_rgba(Size::new(1, 1), Rgba8::new(255, 0, 0, 128))
            .unwrap()
            .premultiply()
            .unwrap()
            .unpremultiply()
            .unwrap();
        assert_eq!(f.alpha_mode(), AlphaMode::Straight);
        assert_eq!(f.data()[0], 255);
    }

    #[test]
    fn rgb8_rejects_straight() {
        let f = Frame::zeros(Size::new(1, 1), FrameFormat::Rgb8).unwrap();
        assert!(f.with_alpha_mode(AlphaMode::Straight).is_err());
    }
}

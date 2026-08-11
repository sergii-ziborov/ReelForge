//! Orthogonal and free-angle rotate effects.

use crate::raster::{rotate_180, rotate_270_cw, rotate_90_cw, rotate_degrees};
use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Clockwise rotation: multiples of 90° or an arbitrary angle in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rotate {
    /// 90° clockwise.
    Cw90,
    /// 180°.
    Cw180,
    /// 270° clockwise (90° counter-clockwise).
    Cw270,
    /// Arbitrary clockwise degrees (canvas size unchanged; exterior filled black).
    Degrees(f32),
}

impl Rotate {
    /// 90° clockwise.
    #[must_use]
    pub const fn cw90() -> Self {
        Self::Cw90
    }

    /// 180°.
    #[must_use]
    pub const fn half() -> Self {
        Self::Cw180
    }

    /// 270° clockwise.
    #[must_use]
    pub const fn cw270() -> Self {
        Self::Cw270
    }

    /// Free rotation by `degrees` clockwise (nearest-neighbor sample).
    #[must_use]
    pub const fn degrees(degrees: f32) -> Self {
        Self::Degrees(degrees)
    }
}

impl VideoEffect for Rotate {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(RotatedVideo {
            inner: clip,
            rotate: *self,
        }))
    }
}

struct RotatedVideo {
    inner: Arc<dyn VideoClip>,
    rotate: Rotate,
}

impl VideoClip for RotatedVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        let s = self.inner.size();
        match self.rotate {
            Rotate::Cw180 | Rotate::Degrees(_) => s,
            Rotate::Cw90 | Rotate::Cw270 => Size::new(s.height, s.width),
        }
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let frame = self.inner.frame_at(t)?;
        match self.rotate {
            Rotate::Cw90 => rotate_90_cw(&frame),
            Rotate::Cw180 => rotate_180(&frame),
            Rotate::Cw270 => rotate_270_cw(&frame),
            Rotate::Degrees(d) => rotate_degrees(&frame, d),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn rotate_swaps_dims() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(6, 2),
            Rgb8::RED,
            Duration::from_secs(0.5),
        ));
        let out = Rotate::cw90().apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(2, 6));
    }

    #[test]
    fn free_rotate_keeps_size() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 6),
            Rgb8::BLUE,
            Duration::from_secs(0.5),
        ));
        let out = Rotate::degrees(45.0).apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(8, 6));
        let f = out.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.size(), Size::new(8, 6));
    }
}

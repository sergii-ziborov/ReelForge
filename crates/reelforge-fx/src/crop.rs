//! Crop effect.

use crate::raster::crop_frame;
use reelforge_core::{
    CoreError, Duration, Frame, Mask, Result, Size, Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Keep a rectangular subregion of each frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crop {
    /// Left edge in source pixels.
    pub x: u32,
    /// Top edge in source pixels.
    pub y: u32,
    /// Output width.
    pub width: u32,
    /// Output height.
    pub height: u32,
}

impl Crop {
    /// Construct a crop rectangle.
    #[must_use]
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

impl VideoEffect for Crop {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if self.width == 0 || self.height == 0 {
            return Err(CoreError::invalid_frame("crop size must be positive"));
        }
        let src = clip.size();
        if self.x.saturating_add(self.width) > src.width
            || self.y.saturating_add(self.height) > src.height
        {
            return Err(CoreError::invalid_frame(format!(
                "crop {self:?} exceeds source {src:?}"
            )));
        }
        Ok(Arc::new(CroppedVideo {
            inner: clip,
            crop: *self,
        }))
    }
}

struct CroppedVideo {
    inner: Arc<dyn VideoClip>,
    crop: Crop,
}

impl VideoClip for CroppedVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        Size::new(self.crop.width, self.crop.height)
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let frame = self.inner.frame_at(t)?;
        crop_frame(
            &frame,
            self.crop.x,
            self.crop.y,
            self.crop.width,
            self.crop.height,
        )
    }

    fn mask_at(&self, t: Time) -> Result<Option<Mask>> {
        match self.inner.mask_at(t)? {
            None => Ok(None),
            Some(mask) => {
                // Spatial crop drops inherited masks; re-derive if needed.
                let _ = mask;
                Ok(None)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn crop_applies() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(10, 10),
            Rgb8::GREEN,
            Duration::from_secs(1.0),
        ));
        let out = Crop::new(2, 2, 4, 4).apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(4, 4));
        let f = out.frame_at(Time::ZERO).unwrap();
        assert_eq!(&f.data()[0..3], &[0, 255, 0]);
    }
}

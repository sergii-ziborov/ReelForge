//! Force even width/height (codec-friendly).

use crate::raster::crop_frame;
use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Crop one pixel from odd dimensions so both sides are even.
#[derive(Debug, Clone, Copy, Default)]
pub struct EvenSize;

impl VideoEffect for EvenSize {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(EvenSizedVideo { inner: clip }))
    }
}

struct EvenSizedVideo {
    inner: Arc<dyn VideoClip>,
}

fn even_dims(size: Size) -> Size {
    Size::new(
        size.width - (size.width % 2),
        size.height - (size.height % 2),
    )
}

impl VideoClip for EvenSizedVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        even_dims(self.inner.size())
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let frame = self.inner.frame_at(t)?;
        let target = even_dims(frame.size());
        if target == frame.size() {
            return Ok(frame);
        }
        if !target.is_positive() {
            return Err(reelforge_core::CoreError::InvalidSize(target));
        }
        crop_frame(&frame, 0, 0, target.width, target.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn makes_even() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(5, 7),
            Rgb8::RED,
            Duration::from_secs(0.5),
        ));
        let out = EvenSize.apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(4, 6));
        assert!(out.size().is_even());
    }
}

//! Resize effect (nearest-neighbor).

use crate::raster::{resize_nearest, resolve_resize_size};
use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Resize frames to a target size.
///
/// Provide both dimensions, or one dimension to preserve aspect ratio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Resize {
    /// Target width, if set.
    pub width: Option<u32>,
    /// Target height, if set.
    pub height: Option<u32>,
}

impl Resize {
    /// Exact size.
    #[must_use]
    pub const fn to(size: Size) -> Self {
        Self {
            width: Some(size.width),
            height: Some(size.height),
        }
    }

    /// Width only (height from aspect).
    #[must_use]
    pub const fn width(width: u32) -> Self {
        Self {
            width: Some(width),
            height: None,
        }
    }

    /// Height only (width from aspect).
    #[must_use]
    pub const fn height(height: u32) -> Self {
        Self {
            width: None,
            height: Some(height),
        }
    }
}

impl VideoEffect for Resize {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        let target = resolve_resize_size(clip.size(), self.width, self.height)?;
        Ok(Arc::new(ResizedVideo {
            inner: clip,
            target,
        }))
    }
}

struct ResizedVideo {
    inner: Arc<dyn VideoClip>,
    target: Size,
}

impl VideoClip for ResizedVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        self.target
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let frame = self.inner.frame_at(t)?;
        resize_nearest(&frame, self.target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn resize_exact() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 4),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        ));
        let out = Resize::to(Size::new(4, 2)).apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(4, 2));
        assert_eq!(out.frame_at(Time::ZERO).unwrap().size(), Size::new(4, 2));
    }
}

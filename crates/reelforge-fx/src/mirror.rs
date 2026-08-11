//! Mirror (flip) effects.

use crate::raster::{mirror_x, mirror_y};
use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Flip horizontally.
#[derive(Debug, Clone, Copy, Default)]
pub struct MirrorX;

/// Flip vertically.
#[derive(Debug, Clone, Copy, Default)]
pub struct MirrorY;

impl VideoEffect for MirrorX {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MirroredVideo {
            inner: clip,
            axis: Axis::X,
        }))
    }
}

impl VideoEffect for MirrorY {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(MirroredVideo {
            inner: clip,
            axis: Axis::Y,
        }))
    }
}

#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

struct MirroredVideo {
    inner: Arc<dyn VideoClip>,
    axis: Axis,
}

impl VideoClip for MirroredVideo {
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
        let frame = self.inner.frame_at(t)?;
        match self.axis {
            Axis::X => mirror_x(&frame),
            Axis::Y => mirror_y(&frame),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn mirror_keeps_size() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(4, 4),
            Rgb8::WHITE,
            Duration::from_secs(1.0),
        ));
        let out = MirrorX.apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(4, 4));
        let _ = out.frame_at(Time::ZERO).unwrap();
    }
}

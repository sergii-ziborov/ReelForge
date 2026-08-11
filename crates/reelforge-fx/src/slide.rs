//! Slide-in / slide-out transitions (position offset + crop to canvas).

use reelforge_core::{CoreError, Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Side a slide enters or exits from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlideSide {
    /// From left edge.
    Left,
    /// From right edge.
    Right,
    /// From top edge.
    Top,
    /// From bottom edge.
    Bottom,
}

/// Slide the clip in from `side` over `duration` (starts off-canvas).
#[derive(Debug, Clone, Copy)]
pub struct SlideIn {
    /// Transition length.
    pub duration: Duration,
    /// Entry side.
    pub side: SlideSide,
}

impl SlideIn {
    /// Construct a slide-in.
    #[must_use]
    pub const fn new(duration: Duration, side: SlideSide) -> Self {
        Self { duration, side }
    }
}

impl VideoEffect for SlideIn {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing("slide-in duration must be > 0"));
        }
        Ok(Arc::new(SlideVideo {
            inner: clip,
            duration: self.duration.as_secs(),
            side: self.side,
            kind: SlideKind::In,
        }))
    }
}

/// Slide the clip out toward `side` over `duration` at the end.
#[derive(Debug, Clone, Copy)]
pub struct SlideOut {
    /// Transition length.
    pub duration: Duration,
    /// Exit side.
    pub side: SlideSide,
}

impl SlideOut {
    /// Construct a slide-out.
    #[must_use]
    pub const fn new(duration: Duration, side: SlideSide) -> Self {
        Self { duration, side }
    }
}

impl VideoEffect for SlideOut {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing("slide-out duration must be > 0"));
        }
        Ok(Arc::new(SlideVideo {
            inner: clip,
            duration: self.duration.as_secs(),
            side: self.side,
            kind: SlideKind::Out,
        }))
    }
}

#[derive(Clone, Copy)]
enum SlideKind {
    In,
    Out,
}

struct SlideVideo {
    inner: Arc<dyn VideoClip>,
    duration: f64,
    side: SlideSide,
    kind: SlideKind,
}

impl VideoClip for SlideVideo {
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
        let size = frame.size();
        let total = self.inner.duration().as_secs();
        let progress = match self.kind {
            SlideKind::In => (t.as_secs() / self.duration).clamp(0.0, 1.0),
            SlideKind::Out => {
                let start = (total - self.duration).max(0.0);
                if t.as_secs() < start {
                    1.0
                } else {
                    1.0 - ((t.as_secs() - start) / self.duration).clamp(0.0, 1.0)
                }
            }
        };
        // progress 0 = fully off, 1 = fully on
        let (ox, oy) = offset_for(self.side, size, 1.0 - progress);
        blit_offset(&frame, ox, oy)
    }
}

#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
fn offset_for(edge: SlideSide, canvas: Size, off_frac: f64) -> (i32, i32) {
    let width_f = f64::from(canvas.width);
    let height_f = f64::from(canvas.height);
    match edge {
        SlideSide::Left => ((-off_frac * width_f) as i32, 0),
        SlideSide::Right => ((off_frac * width_f) as i32, 0),
        SlideSide::Top => (0, (-off_frac * height_f) as i32),
        SlideSide::Bottom => (0, (off_frac * height_f) as i32),
    }
}

#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_sign_loss
)]
fn blit_offset(src: &Frame, ox: i32, oy: i32) -> Result<Frame> {
    let size = src.size();
    let bpp = src.format().bytes_per_pixel();
    let w = size.width as i32;
    let h = size.height as i32;
    let mut out = vec![0_u8; src.data().len()];
    let sw = size.width as usize;
    let data = src.data();
    for y in 0..h {
        for x in 0..w {
            let sx = x - ox;
            let sy = y - oy;
            if sx >= 0 && sy >= 0 && sx < w && sy < h {
                let si = (sy as usize * sw + sx as usize) * bpp;
                let di = (y as usize * sw + x as usize) * bpp;
                out[di..di + bpp].copy_from_slice(&data[si..si + bpp]);
            }
        }
    }
    Frame::from_raw(size, src.format(), out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn slide_in_keeps_size() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(8, 6),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let out = SlideIn::new(Duration::from_secs(0.5), SlideSide::Left)
            .apply(clip)
            .unwrap();
        assert_eq!(out.size(), Size::new(8, 6));
        let f = out.frame_at(Time::ZERO).unwrap();
        assert_eq!(f.size(), Size::new(8, 6));
    }
}

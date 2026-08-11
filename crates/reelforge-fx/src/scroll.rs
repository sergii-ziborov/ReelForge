//! Scroll / Ken-Burns-style crop window over a larger clip.

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use crate::raster::crop_frame;
use std::sync::Arc;

/// Scroll a fixed-size viewport across the clip.
#[derive(Debug, Clone, Copy)]
pub struct Scroll {
    /// Viewport width (defaults to clip width when `None` at apply time → must set).
    pub width: u32,
    /// Viewport height.
    pub height: u32,
    /// Horizontal pixels per second (positive = content moves left).
    pub x_speed: f64,
    /// Vertical pixels per second.
    pub y_speed: f64,
    /// Initial crop origin x.
    pub x_start: f64,
    /// Initial crop origin y.
    pub y_start: f64,
}

impl Scroll {
    /// Fixed viewport scrolling at the given speeds.
    #[must_use]
    pub const fn new(width: u32, height: u32, x_speed: f64, y_speed: f64) -> Self {
        Self {
            width,
            height,
            x_speed,
            y_speed,
            x_start: 0.0,
            y_start: 0.0,
        }
    }
}

impl VideoEffect for Scroll {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        let src = clip.size();
        let w = self.width.min(src.width).max(1);
        let h = self.height.min(src.height).max(1);
        Ok(Arc::new(ScrollVideo {
            inner: clip,
            width: w,
            height: h,
            x_speed: self.x_speed,
            y_speed: self.y_speed,
            x_start: self.x_start,
            y_start: self.y_start,
        }))
    }
}

struct ScrollVideo {
    inner: Arc<dyn VideoClip>,
    width: u32,
    height: u32,
    x_speed: f64,
    y_speed: f64,
    x_start: f64,
    y_start: f64,
}

impl VideoClip for ScrollVideo {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let frame = self.inner.frame_at(t)?;
        let src = frame.size();
        let max_x = src.width.saturating_sub(self.width);
        let max_y = src.height.saturating_sub(self.height);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let x = (self.x_start + self.x_speed * t.as_secs())
            .round()
            .clamp(0.0, f64::from(max_x)) as u32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let y = (self.y_start + self.y_speed * t.as_secs())
            .round()
            .clamp(0.0, f64::from(max_y)) as u32;
        crop_frame(&frame, x, y, self.width, self.height)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn scroll_viewport() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(20, 10),
            Rgb8::BLUE,
            Duration::from_secs(1.0),
        ));
        let out = Scroll::new(8, 6, 2.0, 0.0).apply(clip).unwrap();
        assert_eq!(out.size(), Size::new(8, 6));
        assert_eq!(out.frame_at(Time::ZERO).unwrap().size(), Size::new(8, 6));
    }
}

//! Fade-in / fade-out toward a solid color.

use crate::raster::fade_towards;
use reelforge_core::{
    CoreError, Duration, Frame, Result, Rgb8, Size, Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Fade from `color` to the clip over `duration` at the start.
#[derive(Debug, Clone, Copy)]
pub struct FadeIn {
    /// Fade length.
    pub duration: Duration,
    /// Color at `t = 0` (default black).
    pub color: Rgb8,
}

impl FadeIn {
    /// Fade in from black.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            color: Rgb8::BLACK,
        }
    }

    /// Fade in from a custom color.
    #[must_use]
    pub fn from_color(duration: Duration, color: Rgb8) -> Self {
        Self { duration, color }
    }
}

impl VideoEffect for FadeIn {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing("fade-in duration must be > 0"));
        }
        Ok(Arc::new(FadedVideo {
            inner: clip,
            kind: FadeKind::In {
                duration: self.duration,
                color: self.color,
            },
        }))
    }
}

/// Fade from the clip to `color` over `duration` at the end.
#[derive(Debug, Clone, Copy)]
pub struct FadeOut {
    /// Fade length.
    pub duration: Duration,
    /// Color at the end of the clip (default black).
    pub color: Rgb8,
}

impl FadeOut {
    /// Fade out to black.
    #[must_use]
    pub fn new(duration: Duration) -> Self {
        Self {
            duration,
            color: Rgb8::BLACK,
        }
    }

    /// Fade out to a custom color.
    #[must_use]
    pub fn to_color(duration: Duration, color: Rgb8) -> Self {
        Self { duration, color }
    }
}

impl VideoEffect for FadeOut {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing("fade-out duration must be > 0"));
        }
        Ok(Arc::new(FadedVideo {
            inner: clip,
            kind: FadeKind::Out {
                duration: self.duration,
                color: self.color,
            },
        }))
    }
}

#[derive(Clone, Copy)]
enum FadeKind {
    In { duration: Duration, color: Rgb8 },
    Out { duration: Duration, color: Rgb8 },
}

struct FadedVideo {
    inner: Arc<dyn VideoClip>,
    kind: FadeKind,
}

impl VideoClip for FadedVideo {
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
        #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
        let amount = match self.kind {
            FadeKind::In { duration, color: _ } => {
                let d = duration.as_secs();
                if d <= 0.0 || t.as_secs() >= d {
                    0.0_f32
                } else {
                    // 1 at t=0 (full color), 0 at t=duration
                    (1.0 - t.as_secs() / d) as f32
                }
            }
            FadeKind::Out { duration, color: _ } => {
                let total = self.inner.duration().as_secs();
                let d = duration.as_secs();
                let start = (total - d).max(0.0);
                if d <= 0.0 || t.as_secs() <= start {
                    0.0
                } else {
                    ((t.as_secs() - start) / d) as f32
                }
            }
        }
        .clamp(0.0, 1.0);
        let color = match self.kind {
            FadeKind::In { color, .. } | FadeKind::Out { color, .. } => color,
        };
        fade_towards(&frame, color, amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, VideoClip};

    #[test]
    fn fade_in_starts_dark() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::WHITE,
            Duration::from_secs(2.0),
        ));
        let out = FadeIn::new(Duration::from_secs(1.0)).apply(clip).unwrap();
        let f0 = out.frame_at(Time::ZERO).unwrap();
        // fully black
        assert_eq!(&f0.data()[0..3], &[0, 0, 0]);
        let f1 = out.frame_at(Time::from_secs(1.0)).unwrap();
        // past fade: white
        assert_eq!(&f1.data()[0..3], &[255, 255, 255]);
    }
}

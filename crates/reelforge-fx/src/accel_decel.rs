//! Accelerate then decelerate through the source timeline (GIF-style ease).

use reelforge_core::{
    CoreError, Duration, Frame, Result, Size, Time, VideoClip, VideoEffect, VideoSurface,
};
use std::sync::Arc;

/// Remap playback so the clip eases in and out over `new_duration`.
///
/// Source time is sampled with a smoothstep-like curve over the output duration.
#[derive(Debug, Clone, Copy)]
pub struct AccelDecel {
    /// Output duration (must be > 0).
    pub new_duration: Duration,
}

impl AccelDecel {
    /// Stretch/compress the clip into `new_duration` with accel/decel timing.
    #[must_use]
    pub const fn new(new_duration: Duration) -> Self {
        Self { new_duration }
    }
}

impl VideoEffect for AccelDecel {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.new_duration.is_positive() {
            return Err(CoreError::invalid_timing(
                "accel/decel duration must be > 0",
            ));
        }
        if !clip.duration().is_positive() {
            return Err(CoreError::invalid_timing("accel/decel source must be > 0"));
        }
        Ok(Arc::new(AccelVideo {
            inner: clip,
            out_dur: self.new_duration.as_secs(),
        }))
    }
}

struct AccelVideo {
    inner: Arc<dyn VideoClip>,
    out_dur: f64,
}

impl VideoClip for AccelVideo {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.out_dur)
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.out_dur {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.out_dur)),
            });
        }
        let u = (t.as_secs() / self.out_dur).clamp(0.0, 1.0);
        // Smoothstep: 3u^2 - 2u^3 (slow-fast-slow)
        let s = u * u * (3.0 - 2.0 * u);
        let src_d = self.inner.duration().as_secs();
        let src_t = (s * src_d).min((src_d - f64::EPSILON).max(0.0));
        self.inner.frame_at(Time::from_secs(src_t))
    }

    fn surface_at(&self, t: Time) -> Result<VideoSurface> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.out_dur {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.out_dur)),
            });
        }
        let u = (t.as_secs() / self.out_dur).clamp(0.0, 1.0);
        let s = u * u * (3.0 - 2.0 * u);
        let src_d = self.inner.duration().as_secs();
        let src_t = (s * src_d).min((src_d - f64::EPSILON).max(0.0));
        self.inner.surface_at(Time::from_secs(src_t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn remaps_duration() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let out = AccelDecel::new(Duration::from_secs(2.0))
            .apply(clip)
            .unwrap();
        assert!((out.duration().as_secs() - 2.0).abs() < 1e-9);
        assert!(out.frame_at(Time::from_secs(1.0)).is_ok());
    }
}

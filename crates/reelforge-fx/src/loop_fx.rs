//! Loop a clip for a target duration or count.

use reelforge_core::{
    CoreError, Duration, Frame, Result, Size, Time, VideoClip, VideoEffect, VideoSurface,
};
use std::sync::Arc;

/// Loop the source clip.
#[derive(Debug, Clone, Copy)]
pub struct Loop {
    /// Total output duration. If `None`, uses `n` full plays.
    pub duration: Option<Duration>,
    /// Number of full plays when `duration` is `None` (default 2).
    pub n: u32,
}

impl Loop {
    /// Loop until `duration` is filled.
    #[must_use]
    pub fn until(duration: Duration) -> Self {
        Self {
            duration: Some(duration),
            n: 1,
        }
    }

    /// Play the clip `n` times (`n >= 1`).
    #[must_use]
    pub fn times(n: u32) -> Self {
        Self {
            duration: None,
            n: n.max(1),
        }
    }
}

impl VideoEffect for Loop {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        let src_d = clip.duration();
        if !src_d.is_positive() {
            return Err(CoreError::invalid_timing(
                "loop source duration must be > 0",
            ));
        }
        let out_d = if let Some(d) = self.duration {
            if !d.is_positive() {
                return Err(CoreError::invalid_timing("loop duration must be > 0"));
            }
            d
        } else {
            Duration::from_secs(src_d.as_secs() * f64::from(self.n))
        };
        Ok(Arc::new(LoopedVideo {
            inner: clip,
            duration: out_d,
            source_duration: src_d,
        }))
    }
}

struct LoopedVideo {
    inner: Arc<dyn VideoClip>,
    duration: Duration,
    source_duration: Duration,
}

impl VideoClip for LoopedVideo {
    fn duration(&self) -> Duration {
        self.duration
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let sd = self.source_duration.as_secs();
        let mut local = t.as_secs() % sd;
        // Edge: t just below duration can map to almost sd.
        if local >= sd {
            local = (sd - 1e-9).max(0.0);
        }
        self.inner.frame_at(Time::from_secs(local))
    }

    fn surface_at(&self, t: Time) -> Result<VideoSurface> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.duration.as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration.as_secs())),
            });
        }
        let sd = self.source_duration.as_secs();
        let mut local = t.as_secs() % sd;
        if local >= sd {
            local = (sd - 1e-9).max(0.0);
        }
        self.inner.surface_at(Time::from_secs(local))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn loop_extends_duration() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::GREEN,
            Duration::from_secs(1.0),
        ));
        let out = Loop::times(3).apply(clip).unwrap();
        assert!((out.duration().as_secs() - 3.0).abs() < 1e-9);
        let _ = out.frame_at(Time::from_secs(2.5)).unwrap();
    }
}

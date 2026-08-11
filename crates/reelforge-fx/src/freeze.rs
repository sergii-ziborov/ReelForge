//! Freeze a frame for a stretch of time.

use reelforge_core::{CoreError, Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Hold the frame at `t` for `hold` seconds, inserted at `t` in the timeline.
///
/// Output duration = source duration + hold.
#[derive(Debug, Clone, Copy)]
pub struct Freeze {
    /// Source time of the frozen frame.
    pub t: Time,
    /// How long to hold that frame.
    pub hold: Duration,
}

impl Freeze {
    /// Freeze frame at `t` for `hold` seconds.
    #[must_use]
    pub fn new(t: Time, hold: Duration) -> Self {
        Self { t, hold }
    }
}

impl VideoEffect for Freeze {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        if !self.hold.is_positive() {
            return Err(CoreError::invalid_timing("freeze hold must be > 0"));
        }
        if self.t.as_secs() < 0.0 || self.t.as_secs() >= clip.duration().as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: self.t,
                range: (Time::ZERO, Time::from_secs(clip.duration().as_secs())),
            });
        }
        Ok(Arc::new(FrozenVideo {
            inner: clip,
            freeze_at: self.t,
            hold: self.hold,
        }))
    }
}

struct FrozenVideo {
    inner: Arc<dyn VideoClip>,
    freeze_at: Time,
    hold: Duration,
}

impl VideoClip for FrozenVideo {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() + self.hold.as_secs())
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        if t.as_secs() < 0.0 || t.as_secs() >= self.duration().as_secs() {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        let freeze = self.freeze_at.as_secs();
        let hold = self.hold.as_secs();
        let src_t = if t.as_secs() < freeze {
            t.as_secs()
        } else if t.as_secs() < freeze + hold {
            freeze
        } else {
            t.as_secs() - hold
        };
        let max = (self.inner.duration().as_secs() - 1e-9).max(0.0);
        self.inner.frame_at(Time::from_secs(src_t.min(max)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn freeze_extends() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::BLUE,
            Duration::from_secs(2.0),
        ));
        let out = Freeze::new(Time::from_secs(0.5), Duration::from_secs(1.0))
            .apply(clip)
            .unwrap();
        assert!((out.duration().as_secs() - 3.0).abs() < 1e-9);
        let _ = out.frame_at(Time::from_secs(1.0)).unwrap();
    }
}

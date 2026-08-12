//! Play clip forward then backward (time symmetrize).

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Plays the source once forward, then once in reverse (2× duration).
#[derive(Debug, Clone, Copy, Default)]
pub struct TimeSymmetrize;

impl VideoEffect for TimeSymmetrize {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(SymVideo { inner: clip }))
    }
}

struct SymVideo {
    inner: Arc<dyn VideoClip>,
}

impl VideoClip for SymVideo {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() * 2.0)
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps()
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        let d = self.inner.duration().as_secs();
        if d <= 0.0 {
            return self.inner.frame_at(Time::ZERO);
        }
        let tt = t.as_secs();
        let src = if tt < d {
            tt
        } else {
            // reverse half: at t=d sample last; at t=2d-eps sample start
            let back = tt - d;
            (d - back - f64::EPSILON).max(0.0)
        };
        self.inner
            .frame_at(Time::from_secs(src.min(d - f64::EPSILON).max(0.0)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn doubles_duration() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(1.0),
        ));
        let out = TimeSymmetrize.apply(clip).unwrap();
        assert!((out.duration().as_secs() - 2.0).abs() < 1e-9);
        assert!(out.frame_at(Time::from_secs(1.5)).is_ok());
    }
}

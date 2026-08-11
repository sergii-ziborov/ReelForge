//! Periodic blink (on/off visibility via black frames).

use reelforge_core::{Duration, Frame, Result, Rgb8, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Alternates between showing the clip and a solid black frame.
#[derive(Debug, Clone, Copy)]
pub struct Blink {
    /// Visible duration per cycle.
    pub duration_on: Duration,
    /// Hidden duration per cycle.
    pub duration_off: Duration,
}

impl Blink {
    /// Construct on/off periods.
    #[must_use]
    pub const fn new(duration_on: Duration, duration_off: Duration) -> Self {
        Self {
            duration_on,
            duration_off,
        }
    }
}

impl VideoEffect for Blink {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(BlinkVideo {
            inner: clip,
            on: self.duration_on.as_secs().max(0.0),
            off: self.duration_off.as_secs().max(0.0),
        }))
    }
}

struct BlinkVideo {
    inner: Arc<dyn VideoClip>,
    on: f64,
    off: f64,
}

impl VideoClip for BlinkVideo {
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
        let cycle = self.on + self.off;
        if cycle > 0.0 {
            let phase = t.as_secs().rem_euclid(cycle);
            if phase >= self.on {
                return Frame::solid_rgb(self.inner.size(), Rgb8::BLACK);
            }
        }
        self.inner.frame_at(t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8 as C};

    #[test]
    fn blink_hides() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            C::WHITE,
            Duration::from_secs(2.0),
        ));
        let out = Blink::new(Duration::from_secs(0.5), Duration::from_secs(0.5))
            .apply(clip)
            .unwrap();
        let on = out.frame_at(Time::from_secs(0.1)).unwrap();
        assert_eq!(on.data()[0], 255);
        let off = out.frame_at(Time::from_secs(0.6)).unwrap();
        assert_eq!(off.data()[0], 0);
    }
}

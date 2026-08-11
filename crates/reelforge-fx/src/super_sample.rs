//! Temporal super-sampling (average neighboring frames).

use reelforge_core::{Duration, Frame, Result, Size, Time, VideoClip, VideoEffect};
use std::sync::Arc;

/// Replace each frame with the mean of `n_frames` samples in `[t-d, t+d]`.
#[derive(Debug, Clone, Copy)]
pub struct SuperSample {
    /// Half-window duration (seconds) around `t`.
    pub d: f64,
    /// Number of samples in the window (`>= 1`).
    pub n_frames: u32,
}

impl SuperSample {
    /// Average `n_frames` frames over ±`d` seconds.
    #[must_use]
    pub const fn new(d: f64, n_frames: u32) -> Self {
        Self { d, n_frames }
    }
}

impl VideoEffect for SuperSample {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        Ok(Arc::new(SsVideo {
            inner: clip,
            d: self.d.max(0.0),
            n: self.n_frames.max(1),
        }))
    }
}

struct SsVideo {
    inner: Arc<dyn VideoClip>,
    d: f64,
    n: u32,
}

impl VideoClip for SsVideo {
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
        if self.n == 1 || self.d <= 0.0 {
            return self.inner.frame_at(t);
        }
        let dur = self.inner.duration().as_secs();
        let max_t = (dur - f64::EPSILON).max(0.0);
        let mut acc: Option<Vec<u32>> = None;
        let mut count = 0_u32;
        let n = self.n;
        for i in 0..n {
            #[allow(clippy::cast_precision_loss)]
            let u = if n == 1 {
                0.5
            } else {
                f64::from(i) / f64::from(n - 1)
            };
            let sample_t = (t.as_secs() - self.d + 2.0 * self.d * u).clamp(0.0, max_t);
            let f = self.inner.frame_at(Time::from_secs(sample_t))?;
            let data = f.data();
            let bucket = acc.get_or_insert_with(|| vec![0_u32; data.len()]);
            for (a, &b) in bucket.iter_mut().zip(data.iter()) {
                *a += u32::from(b);
            }
            count += 1;
        }
        let bucket = acc.unwrap_or_default();
        let out: Vec<u8> = bucket
            .into_iter()
            .map(|s| {
                #[allow(clippy::cast_possible_truncation)]
                {
                    (s / count.max(1)) as u8
                }
            })
            .collect();
        Frame::from_raw(self.inner.size(), self.inner.frame_at(t)?.format(), out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn averages_solid() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::new(100, 100, 100),
            Duration::from_secs(1.0),
        ));
        let out = SuperSample::new(0.1, 3).apply(clip).unwrap();
        let f = out.frame_at(Time::from_secs(0.5)).unwrap();
        assert_eq!(f.data()[0], 100);
    }
}

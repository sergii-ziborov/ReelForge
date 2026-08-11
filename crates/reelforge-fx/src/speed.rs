//! Playback speed / duration scale.

use reelforge_core::{
    AudioBuffer, AudioClip, AudioEffect, AudioFormat, CoreError, Duration, Frame, Result, Size,
    Time, VideoClip, VideoEffect,
};
use std::sync::Arc;

/// Multiply playback speed by `factor` (`2.0` = twice as fast, half duration).
///
/// Applies to video (time remap) or audio (time remap of sample windows).
#[derive(Debug, Clone, Copy)]
pub struct Speed {
    /// Speed factor; must be finite and `> 0`.
    pub factor: f64,
}

impl Speed {
    /// Construct a speed effect.
    #[must_use]
    pub const fn new(factor: f64) -> Self {
        Self { factor }
    }
}

fn validate_factor(factor: f64) -> Result<()> {
    if factor.is_finite() && factor > 0.0 {
        Ok(())
    } else {
        Err(CoreError::invalid_timing(format!(
            "speed factor must be finite and > 0, got {factor}"
        )))
    }
}

impl VideoEffect for Speed {
    fn apply(&self, clip: Arc<dyn VideoClip>) -> Result<Arc<dyn VideoClip>> {
        validate_factor(self.factor)?;
        Ok(Arc::new(SpeedVideo {
            inner: clip,
            factor: self.factor,
        }))
    }
}

impl AudioEffect for Speed {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        validate_factor(self.factor)?;
        Ok(Arc::new(SpeedAudio {
            inner: clip,
            factor: self.factor,
        }))
    }
}

struct SpeedVideo {
    inner: Arc<dyn VideoClip>,
    factor: f64,
}

impl VideoClip for SpeedVideo {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() / self.factor)
    }

    fn size(&self) -> Size {
        self.inner.size()
    }

    fn fps(&self) -> Option<f64> {
        self.inner.fps().map(|f| f * self.factor)
    }

    fn frame_at(&self, t: Time) -> Result<Frame> {
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        let src_t = Time::from_secs(t.as_secs() * self.factor);
        // Clamp into source half-open range.
        let max_t = (self.inner.duration().as_secs() - f64::EPSILON).max(0.0);
        let src_t = Time::from_secs(src_t.as_secs().min(max_t));
        self.inner.frame_at(src_t)
    }
}

struct SpeedAudio {
    inner: Arc<dyn AudioClip>,
    factor: f64,
}

impl AudioClip for SpeedAudio {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() / self.factor)
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        if frame_count == 0 {
            return AudioBuffer::silence(self.format(), 0);
        }
        if !self.contains(t) {
            return Err(CoreError::TimeOutOfRange {
                time: t,
                range: (Time::ZERO, Time::from_secs(self.duration().as_secs())),
            });
        }
        // Map output time to source time; request same frame count (simple nearest remap).
        let src_t = Time::from_secs(t.as_secs() * self.factor);
        let max_t = (self.inner.duration().as_secs() - f64::EPSILON).max(0.0);
        let src_t = Time::from_secs(src_t.as_secs().min(max_t));
        // For speed != 1, true resampling would change pitch/duration of the window.
        // Sample at remapped start with the same frame_count (pitch-preserving stretch is approximate).
        self.inner.samples_at(src_t, frame_count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{ColorClip, Rgb8};

    #[test]
    fn speed_halves_duration() {
        let clip: Arc<dyn VideoClip> = Arc::new(ColorClip::new(
            Size::new(2, 2),
            Rgb8::RED,
            Duration::from_secs(4.0),
        ));
        let out = VideoEffect::apply(&Speed::new(2.0), clip).unwrap();
        assert!((out.duration().as_secs() - 2.0).abs() < 1e-9);
        let _ = out.frame_at(Time::from_secs(1.0)).unwrap();
    }
}

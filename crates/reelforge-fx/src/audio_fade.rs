//! Audio fade-in / fade-out gain ramps.

use reelforge_core::{
    AudioBuffer, AudioClip, AudioEffect, AudioFormat, CoreError, Duration, Result, Time,
};
use std::sync::Arc;

/// Linear gain ramp from 0 → 1 over `duration` at the start.
#[derive(Debug, Clone, Copy)]
pub struct AudioFadeIn {
    /// Fade length.
    pub duration: Duration,
}

impl AudioFadeIn {
    /// Construct.
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl AudioEffect for AudioFadeIn {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing("audio fade-in must be > 0"));
        }
        Ok(Arc::new(FadedAudio {
            inner: clip,
            duration: self.duration.as_secs(),
            kind: AudioFadeKind::In,
        }))
    }
}

/// Linear gain ramp from 1 → 0 over `duration` at the end.
#[derive(Debug, Clone, Copy)]
pub struct AudioFadeOut {
    /// Fade length.
    pub duration: Duration,
}

impl AudioFadeOut {
    /// Construct.
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self { duration }
    }
}

impl AudioEffect for AudioFadeOut {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        if !self.duration.is_positive() {
            return Err(CoreError::invalid_timing("audio fade-out must be > 0"));
        }
        Ok(Arc::new(FadedAudio {
            inner: clip,
            duration: self.duration.as_secs(),
            kind: AudioFadeKind::Out,
        }))
    }
}

#[derive(Clone, Copy)]
enum AudioFadeKind {
    In,
    Out,
}

struct FadedAudio {
    inner: Arc<dyn AudioClip>,
    duration: f64,
    kind: AudioFadeKind,
}

impl AudioClip for FadedAudio {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        let mut buf = self.inner.samples_at(t, frame_count)?;
        let total = self.inner.duration().as_secs();
        let rate = f64::from(self.inner.format().sample_rate);
        let ch = self.inner.format().channels() as usize;
        let samples = buf.samples_mut();
        for (i, frame) in samples.chunks_exact_mut(ch).enumerate() {
            #[allow(clippy::cast_precision_loss)]
            let tt = t.as_secs() + i as f64 / rate;
            #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
            let g = match self.kind {
                AudioFadeKind::In => (tt / self.duration).clamp(0.0, 1.0) as f32,
                AudioFadeKind::Out => {
                    let start = (total - self.duration).max(0.0);
                    if tt < start {
                        1.0_f32
                    } else {
                        (1.0 - (tt - start) / self.duration).clamp(0.0, 1.0) as f32
                    }
                }
            };
            for s in frame {
                *s *= g;
            }
        }
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{SampleLayout, SilenceClip};

    #[test]
    fn fade_in_applies() {
        let clip: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat {
                sample_rate: 8_000,
                layout: SampleLayout::Mono,
            },
            Duration::from_secs(1.0),
        ));
        // Silence stays silence; just ensure apply works.
        let out = AudioFadeIn::new(Duration::from_secs(0.2))
            .apply(clip)
            .unwrap();
        let b = out.samples_at(Time::ZERO, 100).unwrap();
        assert_eq!(b.frame_count(), 100);
    }
}

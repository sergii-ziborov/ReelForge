//! Delay audio by inserting leading silence.

use reelforge_core::{
    AudioBuffer, AudioClip, AudioEffect, AudioFormat, CoreError, Duration, Result, Time,
};
use std::sync::Arc;

/// Delay the audio by `delay` seconds (leading silence).
#[derive(Debug, Clone, Copy)]
pub struct AudioDelay {
    /// Delay length.
    pub delay: Duration,
}

impl AudioDelay {
    /// Delay by the given duration.
    #[must_use]
    pub const fn new(delay: Duration) -> Self {
        Self { delay }
    }
}

impl AudioEffect for AudioDelay {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        if self.delay.as_secs() < 0.0 {
            return Err(CoreError::invalid_timing("audio delay must be >= 0"));
        }
        Ok(Arc::new(DelayedAudio {
            inner: clip,
            delay: self.delay.as_secs(),
        }))
    }
}

struct DelayedAudio {
    inner: Arc<dyn AudioClip>,
    delay: f64,
}

impl AudioClip for DelayedAudio {
    fn duration(&self) -> Duration {
        Duration::from_secs(self.inner.duration().as_secs() + self.delay)
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        let format = self.inner.format();
        let rate = f64::from(format.sample_rate);
        let ch = format.channels() as usize;
        let mut out = AudioBuffer::silence(format, frame_count)?;
        if frame_count == 0 {
            return Ok(out);
        }

        // For each output frame i at time t + i/rate:
        // if time < delay → silence; else sample from inner at time - delay.
        let samples = out.samples_mut();
        let mut i = 0_usize;
        while i < frame_count {
            #[allow(clippy::cast_precision_loss)]
            let tt = t.as_secs() + i as f64 / rate;
            if tt < self.delay {
                i += 1;
                continue;
            }
            // contiguous run from inner
            let src_t = tt - self.delay;
            let remaining = frame_count - i;
            let buf = self.inner.samples_at(Time::from_secs(src_t), remaining)?;
            let src = buf.samples();
            let n = src.len() / ch;
            let dst_off = i * ch;
            let copy = (n * ch).min(samples.len().saturating_sub(dst_off));
            samples[dst_off..dst_off + copy].copy_from_slice(&src[..copy]);
            break;
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{SampleLayout, SilenceClip};

    #[test]
    fn extends_duration() {
        let clip: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat {
                sample_rate: 8_000,
                layout: SampleLayout::Mono,
            },
            Duration::from_secs(0.5),
        ));
        let out = AudioDelay::new(Duration::from_secs(0.25))
            .apply(clip)
            .unwrap();
        assert!((out.duration().as_secs() - 0.75).abs() < 1e-6);
    }
}

//! Peak-normalize audio to a target amplitude.

use reelforge_core::{AudioBuffer, AudioClip, AudioEffect, AudioFormat, Duration, Result, Time};
use std::sync::Arc;

/// Scale samples so peak absolute amplitude reaches `target` (default 1.0).
#[derive(Debug, Clone, Copy)]
pub struct AudioNormalize {
    /// Target peak (`1.0` = full scale).
    pub target: f32,
}

impl AudioNormalize {
    /// Peak-normalize to full scale.
    #[must_use]
    pub const fn peak() -> Self {
        Self { target: 1.0 }
    }

    /// Peak-normalize to a custom target.
    #[must_use]
    pub const fn to(target: f32) -> Self {
        Self { target }
    }
}

impl AudioEffect for AudioNormalize {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        /// ~1 s at 48 kHz per pull while scanning peak.
        const CHUNK: usize = 48_000;

        // Scan full clip once to find peak (clip duration limited by practical sizes).
        let format = clip.format();
        let total = format.frames_for_duration(clip.duration());
        let total = usize::try_from(total).unwrap_or(0);
        let mut peak = 0.0_f32;
        let mut pos = 0_usize;
        while pos < total {
            let n = (total - pos).min(CHUNK);
            #[allow(clippy::cast_precision_loss)]
            let t = Time::from_secs(pos as f64 / f64::from(format.sample_rate));
            let buf = clip.samples_at(t, n)?;
            for &s in buf.samples() {
                peak = peak.max(s.abs());
            }
            pos += n;
        }
        let gain = if peak > 1e-9 { self.target / peak } else { 1.0 };
        Ok(Arc::new(NormAudio { inner: clip, gain }))
    }
}

struct NormAudio {
    inner: Arc<dyn AudioClip>,
    gain: f32,
}

impl AudioClip for NormAudio {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        let mut buf = self.inner.samples_at(t, frame_count)?;
        buf.apply_gain(self.gain);
        Ok(buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{SampleLayout, SilenceClip};

    #[test]
    fn silence_stays() {
        let clip: Arc<dyn AudioClip> = Arc::new(SilenceClip::new(
            AudioFormat {
                sample_rate: 4_000,
                layout: SampleLayout::Mono,
            },
            Duration::from_secs(0.1),
        ));
        let out = AudioNormalize::peak().apply(clip).unwrap();
        let b = out.samples_at(Time::ZERO, 10).unwrap();
        assert!(b.samples().iter().all(|&s| s == 0.0));
    }
}

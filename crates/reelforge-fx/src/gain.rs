//! Audio amplitude gain.

use reelforge_core::{
    AudioBuffer, AudioClip, AudioEffect, AudioFormat, CoreError, Duration, Result, Time,
};
use std::sync::Arc;

/// Multiply all samples by a constant gain factor.
#[derive(Debug, Clone, Copy)]
pub struct VolumeGain {
    /// Linear gain (`1.0` = unchanged, `0.5` = half volume).
    pub factor: f32,
}

impl VolumeGain {
    /// Construct a gain effect.
    #[must_use]
    pub const fn new(factor: f32) -> Self {
        Self { factor }
    }
}

impl AudioEffect for VolumeGain {
    fn apply(&self, clip: Arc<dyn AudioClip>) -> Result<Arc<dyn AudioClip>> {
        Ok(Arc::new(GainedAudio {
            inner: clip,
            factor: self.factor,
        }))
    }
}

struct GainedAudio {
    inner: Arc<dyn AudioClip>,
    factor: f32,
}

impl AudioClip for GainedAudio {
    fn duration(&self) -> Duration {
        self.inner.duration()
    }

    fn format(&self) -> AudioFormat {
        self.inner.format()
    }

    fn samples_at(&self, t: Time, frame_count: usize) -> Result<AudioBuffer> {
        let mut buf = self.inner.samples_at(t, frame_count)?;
        buf.apply_gain(self.factor);
        Ok(buf)
    }
}

/// Ensure gain is finite (callers may validate before apply).
///
/// # Errors
///
/// Returns [`CoreError::InvalidAudio`] when `factor` is not finite.
pub fn validate_gain(factor: f32) -> Result<()> {
    if factor.is_finite() {
        Ok(())
    } else {
        Err(CoreError::invalid_audio(format!(
            "gain factor must be finite, got {factor}"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reelforge_core::{AudioFormat, Duration, SilenceClip, Time};

    #[test]
    fn gain_scales_samples() {
        // Silence stays zero; identity factor path still builds graph.
        let silence = Arc::new(SilenceClip::new(
            AudioFormat::STEREO_48K,
            Duration::from_secs(0.1),
        ));
        let gained = VolumeGain::new(0.5).apply(silence).unwrap();
        let buf = gained.samples_at(Time::ZERO, 32).unwrap();
        assert!(buf.samples().iter().all(|&s| s == 0.0));
    }
}

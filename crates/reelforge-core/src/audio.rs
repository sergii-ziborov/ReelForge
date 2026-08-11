//! PCM audio buffers and format descriptors.

use crate::error::{CoreError, Result};
use crate::time::Duration;

/// Channel packing of interleaved PCM samples.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SampleLayout {
    /// Single channel.
    #[default]
    Mono,
    /// Two channels, interleaved L-R.
    Stereo,
}

impl SampleLayout {
    /// Number of channels.
    #[must_use]
    pub const fn channels(self) -> u16 {
        match self {
            Self::Mono => 1,
            Self::Stereo => 2,
        }
    }
}

/// Sample rate and channel layout for a stream or buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioFormat {
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Channel layout.
    pub layout: SampleLayout,
}

impl AudioFormat {
    /// Common 48 kHz stereo master format.
    pub const STEREO_48K: Self = Self {
        sample_rate: 48_000,
        layout: SampleLayout::Stereo,
    };

    /// Common 44.1 kHz stereo format.
    pub const STEREO_44K: Self = Self {
        sample_rate: 44_100,
        layout: SampleLayout::Stereo,
    };

    /// Construct a format.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidAudio`] when `sample_rate` is zero.
    pub fn new(sample_rate: u32, layout: SampleLayout) -> Result<Self> {
        if sample_rate == 0 {
            return Err(CoreError::invalid_audio("sample_rate must be > 0"));
        }
        Ok(Self {
            sample_rate,
            layout,
        })
    }

    /// Number of channels.
    #[must_use]
    pub const fn channels(self) -> u16 {
        self.layout.channels()
    }

    /// Frame count covering `duration` (truncated toward zero).
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn frames_for_duration(self, duration: Duration) -> u64 {
        if duration.as_secs() <= 0.0 {
            return 0;
        }
        (duration.as_secs() * f64::from(self.sample_rate)) as u64
    }

    /// Duration of `frames` sample frames.
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn duration_of_frames(self, frames: u64) -> Duration {
        if self.sample_rate == 0 {
            return Duration::ZERO;
        }
        Duration::from_secs(frames as f64 / f64::from(self.sample_rate))
    }
}

/// Owned interleaved PCM buffer (`f32` samples in `-1.0..=1.0`).
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    format: AudioFormat,
    /// Interleaved samples: `channels` values per frame.
    samples: Vec<f32>,
}

impl AudioBuffer {
    /// Build from interleaved samples.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidAudio`] when the sample count is not a
    /// multiple of the channel count.
    pub fn from_interleaved(format: AudioFormat, samples: Vec<f32>) -> Result<Self> {
        let ch = format.channels() as usize;
        if !samples.len().is_multiple_of(ch) {
            return Err(CoreError::invalid_audio(format!(
                "sample count {} is not a multiple of {} channels",
                samples.len(),
                ch
            )));
        }
        Ok(Self { format, samples })
    }

    /// Silence of the given length in sample frames.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidAudio`] when allocation size overflows.
    pub fn silence(format: AudioFormat, frames: usize) -> Result<Self> {
        let ch = format.channels() as usize;
        let len = frames
            .checked_mul(ch)
            .ok_or_else(|| CoreError::invalid_audio("silence length overflow"))?;
        Ok(Self {
            format,
            samples: vec![0.0; len],
        })
    }

    /// Audio format.
    #[must_use]
    pub const fn format(&self) -> AudioFormat {
        self.format
    }

    /// Number of sample frames (not individual channel samples).
    #[must_use]
    pub fn frame_count(&self) -> usize {
        self.samples.len() / self.format.channels() as usize
    }

    /// Duration implied by the buffer length.
    #[must_use]
    pub fn duration(&self) -> Duration {
        self.format.duration_of_frames(self.frame_count() as u64)
    }

    /// Interleaved samples.
    #[must_use]
    pub fn samples(&self) -> &[f32] {
        &self.samples
    }

    /// Mutable interleaved samples.
    pub fn samples_mut(&mut self) -> &mut [f32] {
        &mut self.samples
    }

    /// Scale amplitude by `gain`.
    pub fn apply_gain(&mut self, gain: f32) {
        for s in &mut self.samples {
            *s *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_duration() {
        let fmt = AudioFormat::STEREO_48K;
        let buf = AudioBuffer::silence(fmt, 24_000).unwrap();
        assert_eq!(buf.frame_count(), 24_000);
        assert!((buf.duration().as_secs() - 0.5).abs() < 1e-9);
    }
}

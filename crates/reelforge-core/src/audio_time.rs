//! Sample-accurate audio timeline (`MediaTime` ↔ sample-frame index).

use crate::audio::AudioFormat;
use crate::error::{CoreError, Result};
use crate::media_time::MediaTime;

/// Maps media time onto PCM sample-frame indexes at a fixed rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AudioTimeline {
    /// Samples per second per channel.
    pub sample_rate: u32,
    /// Clock for [`MediaTime`] ticks (often equal to [`Self::sample_rate`]).
    pub timescale: u32,
}

impl AudioTimeline {
    /// Timeline whose ticks are sample indexes (`timescale == sample_rate`).
    ///
    /// # Errors
    ///
    /// Zero sample rate.
    pub fn from_format(format: AudioFormat) -> Result<Self> {
        Self::new(format.sample_rate, format.sample_rate)
    }

    /// Construct a timeline.
    ///
    /// # Errors
    ///
    /// Zero sample rate or timescale.
    pub fn new(sample_rate: u32, timescale: u32) -> Result<Self> {
        if sample_rate == 0 {
            return Err(CoreError::invalid_audio("sample_rate must be > 0"));
        }
        if timescale == 0 {
            return Err(CoreError::invalid_timing("audio timescale must be > 0"));
        }
        Ok(Self {
            sample_rate,
            timescale,
        })
    }

    /// Sample-frame index at `t` (`floor`, same math as [`MediaTime::frame_index`]).
    #[must_use]
    pub fn index_at(self, t: MediaTime) -> u64 {
        t.frame_index(f64::from(self.sample_rate))
    }

    /// Media time of sample-frame `index` on this clock.
    #[must_use]
    pub fn time_at(self, index: u64) -> MediaTime {
        let rate = i128::from(self.sample_rate.max(1));
        let scale = i128::from(self.timescale);
        let ticks = i128::from(index).saturating_mul(scale) / rate;
        let ticks = i64::try_from(ticks).unwrap_or(if ticks.is_positive() {
            i64::MAX
        } else {
            i64::MIN
        });
        MediaTime::new(ticks, self.timescale).unwrap_or_else(|_| MediaTime::zero(self.timescale))
    }

    /// Half-open sample-frame range `[start, end)` covering `[start_t, end_t)`.
    #[must_use]
    pub fn range(self, start_t: MediaTime, end_t: MediaTime) -> (u64, u64) {
        let a = self.index_at(start_t);
        let b = self.index_at(end_t);
        if b <= a { (a, a) } else { (a, b) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_and_time_roundtrip_at_48k() {
        let tl = AudioTimeline::from_format(AudioFormat::STEREO_48K).unwrap();
        let t = MediaTime::from_secs(0.5, 48_000).unwrap();
        assert_eq!(tl.index_at(t), 24_000);
        let back = tl.time_at(24_000);
        assert!((back.as_secs() - 0.5).abs() < 1e-9);
        assert_eq!(tl.range(MediaTime::zero(48_000), t), (0, 24_000));
    }

    #[test]
    fn rejects_zero_rate() {
        assert!(AudioTimeline::new(0, 48_000).is_err());
        assert!(AudioTimeline::new(48_000, 0).is_err());
    }
}

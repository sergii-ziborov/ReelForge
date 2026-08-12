//! Exact media time (`ticks / timescale`) for PTS-aligned pipelines.
//!
//! Prefer this over `floor(seconds × nominal_fps)` when indexing frames for
//! VFR sources or hybrid `SightLoom` / Capture contracts. The floating
//! [`crate::Time`] remains the ergonomic NLE surface; convert at I/O boundaries.

use crate::error::{CoreError, Result};
use crate::time::{Duration, Time};
use core::fmt;

/// Presentation / media time as rational seconds: `ticks / timescale`.
///
/// Aligned with `SightLoom` [`MediaTime`](MediaTime) shape (ticks + timescale).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MediaTime {
    /// Tick count (may be negative for offsets).
    pub ticks: i64,
    /// Ticks per second; must be non-zero.
    pub timescale: u32,
}

impl Default for MediaTime {
    fn default() -> Self {
        Self {
            ticks: 0,
            timescale: 1,
        }
    }
}

impl MediaTime {
    /// Common 90 kHz media clock.
    pub const HZ_90K: u32 = 90_000;
    /// 1 kHz (millisecond) clock.
    pub const HZ_1K: u32 = 1_000;
    /// 1 Hz (whole seconds).
    pub const HZ_1: u32 = 1;

    /// Origin at the given timescale.
    #[must_use]
    pub const fn zero(timescale: u32) -> Self {
        Self {
            ticks: 0,
            timescale: if timescale == 0 { 1 } else { timescale },
        }
    }

    /// Creates a media time when `timescale` is non-zero.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTiming`] when `timescale` is zero.
    pub fn new(ticks: i64, timescale: u32) -> Result<Self> {
        if timescale == 0 {
            return Err(CoreError::invalid_timing("media timescale must be > 0"));
        }
        Ok(Self { ticks, timescale })
    }

    /// From whole seconds at `timescale` ticks/sec.
    ///
    /// # Errors
    ///
    /// Zero timescale or non-finite seconds.
    #[allow(clippy::cast_possible_truncation)]
    pub fn from_secs(secs: f64, timescale: u32) -> Result<Self> {
        if timescale == 0 {
            return Err(CoreError::invalid_timing("media timescale must be > 0"));
        }
        if !secs.is_finite() {
            return Err(CoreError::invalid_timing(format!(
                "media time seconds must be finite, got {secs}"
            )));
        }
        let ticks = (secs * f64::from(timescale)).round() as i64;
        Ok(Self { ticks, timescale })
    }

    /// From floating [`Time`] at `timescale`.
    ///
    /// # Errors
    ///
    /// Zero timescale.
    pub fn from_time(t: Time, timescale: u32) -> Result<Self> {
        Self::from_secs(t.as_secs(), timescale)
    }

    /// Seconds as `f64` (may lose precision for huge tick counts).
    #[must_use]
    #[allow(clippy::cast_precision_loss)]
    pub fn as_secs(self) -> f64 {
        self.ticks as f64 / f64::from(self.timescale.max(1))
    }

    /// Convert to floating [`Time`].
    #[must_use]
    pub fn to_time(self) -> Time {
        Time::from_secs(self.as_secs())
    }

    /// Nanoseconds since origin (saturating).
    #[must_use]
    pub fn as_nanos(self) -> i64 {
        let ticks = i128::from(self.ticks);
        let scale = i128::from(self.timescale.max(1));
        let nanos = ticks
            .saturating_mul(1_000_000_000)
            .checked_div(scale)
            .unwrap_or(0);
        i64::try_from(nanos).unwrap_or(if nanos.is_positive() {
            i64::MAX
        } else {
            i64::MIN
        })
    }

    /// Frame index using **exact** tick math: `floor(ticks * fps / timescale)`.
    ///
    /// Prefer this over `floor(as_secs() * fps)` for constant-fps indexes.
    /// When `fps` is a whole number, uses integer arithmetic to avoid float
    /// intermediate rounding near frame boundaries.
    #[must_use]
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss
    )]
    pub fn frame_index(self, fps: f64) -> u64 {
        if !(fps.is_finite() && fps > 0.0) || self.ticks <= 0 {
            return 0;
        }
        let timescale = self.timescale.max(1);
        // Integer path for constant integer FPS (common CFR files).
        if fps.fract() == 0.0 && fps >= 1.0 && fps <= f64::from(u32::MAX) {
            let fps_u = fps as u32;
            let ticks = i128::from(self.ticks);
            let num = ticks.saturating_mul(i128::from(fps_u));
            let den = i128::from(timescale);
            let idx = num.div_euclid(den);
            return u64::try_from(idx).unwrap_or(0);
        }
        // ticks/timescale * fps = ticks * fps / timescale
        let v = (self.ticks as f64) * fps / f64::from(timescale);
        if v <= 0.0 { 0 } else { v.floor() as u64 }
    }

    /// Duration between two times on the **same** timescale (tick delta as Duration).
    ///
    /// # Errors
    ///
    /// Mismatched timescales.
    pub fn duration_since(self, earlier: Self) -> Result<Duration> {
        if self.timescale != earlier.timescale {
            return Err(CoreError::invalid_timing(
                "MediaTime duration_since requires matching timescale",
            ));
        }
        let dt = self.ticks.saturating_sub(earlier.ticks);
        #[allow(clippy::cast_precision_loss)]
        Ok(Duration::from_secs(
            dt as f64 / f64::from(self.timescale.max(1)),
        ))
    }
}

impl fmt::Display for MediaTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{} ({:.6}s)",
            self.ticks,
            self.timescale,
            self.as_secs()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_zero_timescale() {
        assert!(MediaTime::new(1, 0).is_err());
    }

    #[test]
    fn frame_index_exact() {
        let t = MediaTime::new(15, 30).unwrap(); // 0.5s
        assert_eq!(t.frame_index(30.0), 15);
        assert_eq!(t.frame_index(60.0), 30);
    }

    #[test]
    fn secs_roundtrip() {
        let t = MediaTime::from_secs(1.5, 1000).unwrap();
        assert_eq!(t.ticks, 1500);
        assert!((t.as_secs() - 1.5).abs() < 1e-9);
    }
}

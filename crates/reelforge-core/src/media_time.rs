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
    /// 1 MHz (microsecond) clock — default compile timescale for float seconds.
    pub const HZ_1M: u32 = 1_000_000;
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

    /// Interpret this instant as a **length** (ticks / timescale) → [`Duration`].
    #[must_use]
    pub fn to_duration(self) -> Duration {
        Duration::from_secs(self.as_secs())
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

    /// From a stream PTS and `time_base = num/den` (`FFmpeg` style, e.g. `1/90000`).
    ///
    /// Result timescale is `den` (ticks are PTS units).
    ///
    /// # Errors
    ///
    /// Zero denominator.
    pub fn from_pts(pts: i64, time_base_num: u32, time_base_den: u32) -> Result<Self> {
        if time_base_den == 0 {
            return Err(CoreError::invalid_timing(
                "time_base denominator must be > 0",
            ));
        }
        if time_base_num == 0 {
            return Err(CoreError::invalid_timing("time_base numerator must be > 0"));
        }
        // pts * num / den seconds → ticks at timescale=den: ticks = pts * num
        let ticks = pts.saturating_mul(i64::from(time_base_num));
        Ok(Self {
            ticks,
            timescale: time_base_den,
        })
    }

    /// Rebase onto another timescale (rounding half-away-from-zero).
    ///
    /// # Errors
    ///
    /// Zero target timescale.
    pub fn rebase(self, new_timescale: u32) -> Result<Self> {
        if new_timescale == 0 {
            return Err(CoreError::invalid_timing("media timescale must be > 0"));
        }
        if self.timescale == new_timescale {
            return Ok(self);
        }
        let ticks = i128::from(self.ticks);
        let old = i128::from(self.timescale.max(1));
        let new = i128::from(new_timescale);
        let num = ticks.saturating_mul(new);
        let half = old / 2;
        let adjusted = if num >= 0 { num + half } else { num - half };
        let t = adjusted.div_euclid(old);
        let ticks = i64::try_from(t).unwrap_or(if t.is_positive() { i64::MAX } else { i64::MIN });
        Ok(Self {
            ticks,
            timescale: new_timescale,
        })
    }

    /// Whether this instant is the origin (`ticks == 0`).
    #[must_use]
    pub const fn is_zero(self) -> bool {
        self.ticks == 0
    }

    /// Add `rhs` after rebasing it onto `self.timescale` (saturating ticks).
    ///
    /// # Errors
    ///
    /// Zero timescale on rebase.
    pub fn saturating_add(self, rhs: Self) -> Result<Self> {
        let rhs = rhs.rebase(self.timescale)?;
        Ok(Self {
            ticks: self.ticks.saturating_add(rhs.ticks),
            timescale: self.timescale,
        })
    }

    /// Subtract `rhs` after rebasing it onto `self.timescale` (saturating ticks).
    ///
    /// # Errors
    ///
    /// Zero timescale on rebase.
    pub fn saturating_sub(self, rhs: Self) -> Result<Self> {
        let rhs = rhs.rebase(self.timescale)?;
        Ok(Self {
            ticks: self.ticks.saturating_sub(rhs.ticks),
            timescale: self.timescale,
        })
    }

    /// Later of the two instants (rhs rebased onto `self.timescale`).
    ///
    /// # Errors
    ///
    /// Zero timescale on rebase.
    pub fn max_time(self, rhs: Self) -> Result<Self> {
        let rhs = rhs.rebase(self.timescale)?;
        Ok(if rhs.ticks > self.ticks { rhs } else { self })
    }

    /// Scale this duration by `1/factor` (speed-up shortens record time).
    ///
    /// # Errors
    ///
    /// Non-finite or non-positive `factor`.
    #[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]
    pub fn div_f64(self, factor: f64) -> Result<Self> {
        if !factor.is_finite() || factor <= 0.0 {
            return Err(CoreError::invalid_timing(format!(
                "invalid scale factor {factor}"
            )));
        }
        #[allow(clippy::cast_possible_truncation)]
        let ticks = (self.ticks as f64 / factor).round() as i64;
        Ok(Self {
            ticks,
            timescale: self.timescale,
        })
    }

    /// Multiply this duration by a positive integer (loop `times`).
    #[must_use]
    pub const fn saturating_mul_u32(self, n: u32) -> Self {
        Self {
            ticks: self.ticks.saturating_mul(n as i64),
            timescale: self.timescale,
        }
    }

    /// CFR half-open frame index range `[start, end)` for a media interval.
    ///
    /// Uses exact tick math via [`Self::frame_index`]. Empty when `end <= start`
    /// or `fps` is invalid.
    #[must_use]
    pub fn frame_range_cfr(start: Self, end: Self, fps: f64) -> (u64, u64) {
        if !(fps.is_finite() && fps > 0.0) {
            return (0, 0);
        }
        let a = start.frame_index(fps);
        let b = end.frame_index(fps);
        if b <= a { (a, a) } else { (a, b) }
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

    #[test]
    fn from_pts_90k() {
        // 1 second at 1/90000 → ticks=90000, timescale=90000
        let t = MediaTime::from_pts(90_000, 1, 90_000).unwrap();
        assert_eq!(t.ticks, 90_000);
        assert_eq!(t.timescale, 90_000);
        assert!((t.as_secs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn frame_range_cfr_half_open() {
        let s = MediaTime::new(0, 30).unwrap();
        let e = MediaTime::new(30, 30).unwrap(); // 1s
        assert_eq!(MediaTime::frame_range_cfr(s, e, 30.0), (0, 30));
        assert_eq!(
            MediaTime::frame_range_cfr(e, s, 30.0),
            (30, 30),
            "empty when inverted"
        );
    }

    #[test]
    fn rebase_preserves_seconds() {
        let t = MediaTime::new(1_000, 1_000).unwrap(); // 1s
        let r = t.rebase(90_000).unwrap();
        assert_eq!(r.ticks, 90_000);
        assert!((r.as_secs() - 1.0).abs() < 1e-12);
    }

    #[test]
    fn add_sub_across_timescales() {
        let a = MediaTime::new(1_000, 1_000).unwrap();
        let b = MediaTime::new(90_000, 90_000).unwrap();
        let sum = a.saturating_add(b).unwrap();
        assert_eq!(sum.timescale, 1_000);
        assert_eq!(sum.ticks, 2_000);
        let back = sum.saturating_sub(b).unwrap();
        assert_eq!(back.ticks, 1_000);
        assert!(MediaTime::zero(1_000).is_zero());
    }

    #[test]
    fn speed_and_loop_scale_ticks() {
        let d = MediaTime::new(4_000, 1_000).unwrap();
        assert_eq!(d.div_f64(2.0).unwrap().ticks, 2_000);
        assert_eq!(d.saturating_mul_u32(3).ticks, 12_000);
        assert!(d.div_f64(0.0).is_err());
    }
}

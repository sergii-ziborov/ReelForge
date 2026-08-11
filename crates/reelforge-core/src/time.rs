//! Media timeline: instants, durations, and half-open ranges.

use crate::error::{CoreError, Result};
use std::fmt;
use std::ops::{Add, AddAssign, Div, Mul, Sub, SubAssign};

/// Instant on a continuous media timeline, measured in seconds from an origin.
///
/// Values are IEEE-754 `f64` seconds, matching common NLE scripting APIs.
/// Callers that need frame-exact editorial should pair this with an FPS and
/// quantize at the I/O boundary.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Time(f64);

impl Time {
    /// Timeline origin.
    pub const ZERO: Self = Self(0.0);

    /// Create a time from seconds. Non-finite values are rejected by [`Time::try_from_secs`].
    #[must_use]
    pub const fn from_secs(secs: f64) -> Self {
        Self(secs)
    }

    /// Create a time from seconds, requiring a finite value.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTiming`] when `secs` is not finite.
    pub fn try_from_secs(secs: f64) -> Result<Self> {
        if secs.is_finite() {
            Ok(Self(secs))
        } else {
            Err(CoreError::invalid_timing(format!(
                "time must be finite, got {secs}"
            )))
        }
    }

    /// Seconds since origin.
    #[must_use]
    pub const fn as_secs(self) -> f64 {
        self.0
    }

    /// Convert to a [`Duration`] measured from zero (absolute value is not taken).
    #[must_use]
    pub const fn as_duration(self) -> Duration {
        Duration::from_secs(self.0)
    }

    /// Clamp into `[min, max]` using partial order on finite times.
    #[must_use]
    pub fn clamp(self, min: Self, max: Self) -> Self {
        if self.0 < min.0 {
            min
        } else if self.0 > max.0 {
            max
        } else {
            self
        }
    }

    /// Whether this instant is strictly before `other`.
    #[must_use]
    pub fn is_before(self, other: Self) -> bool {
        self.0 < other.0
    }

    /// Whether this instant is strictly after `other`.
    #[must_use]
    pub fn is_after(self, other: Self) -> bool {
        self.0 > other.0
    }
}

impl fmt::Display for Time {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}s", self.0)
    }
}

impl Add<Duration> for Time {
    type Output = Self;

    fn add(self, rhs: Duration) -> Self::Output {
        Self(self.0 + rhs.as_secs())
    }
}

impl AddAssign<Duration> for Time {
    fn add_assign(&mut self, rhs: Duration) {
        self.0 += rhs.as_secs();
    }
}

impl Sub<Duration> for Time {
    type Output = Self;

    fn sub(self, rhs: Duration) -> Self::Output {
        Self(self.0 - rhs.as_secs())
    }
}

impl SubAssign<Duration> for Time {
    fn sub_assign(&mut self, rhs: Duration) {
        self.0 -= rhs.as_secs();
    }
}

impl Sub for Time {
    type Output = Duration;

    fn sub(self, rhs: Self) -> Self::Output {
        Duration::from_secs(self.0 - rhs.0)
    }
}

/// Length of a media interval in seconds.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Duration(f64);

impl Duration {
    /// Zero-length interval.
    pub const ZERO: Self = Self(0.0);

    /// Create a duration from seconds.
    #[must_use]
    pub const fn from_secs(secs: f64) -> Self {
        Self(secs)
    }

    /// Create a duration from seconds, requiring a finite non-negative value.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTiming`] when `secs` is not finite or is negative.
    pub fn try_from_secs(secs: f64) -> Result<Self> {
        if secs.is_finite() && secs >= 0.0 {
            Ok(Self(secs))
        } else {
            Err(CoreError::invalid_timing(format!(
                "duration must be finite and >= 0, got {secs}"
            )))
        }
    }

    /// Seconds in this duration.
    #[must_use]
    pub const fn as_secs(self) -> f64 {
        self.0
    }

    /// Whether the duration is strictly positive.
    #[must_use]
    pub fn is_positive(self) -> bool {
        self.0 > 0.0
    }

    /// Whether the duration is zero.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.0 == 0.0
    }

    /// Scale by a finite factor.
    #[must_use]
    pub fn scale(self, factor: f64) -> Self {
        Self(self.0 * factor)
    }

    /// Maximum of two durations.
    #[must_use]
    pub fn max(self, other: Self) -> Self {
        if self.0 >= other.0 { self } else { other }
    }

    /// Minimum of two durations.
    #[must_use]
    pub fn min(self, other: Self) -> Self {
        if self.0 <= other.0 { self } else { other }
    }
}

impl fmt::Display for Duration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.6}s", self.0)
    }
}

impl Add for Duration {
    type Output = Self;

    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl AddAssign for Duration {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl Sub for Duration {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

impl SubAssign for Duration {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

impl Mul<f64> for Duration {
    type Output = Self;

    fn mul(self, rhs: f64) -> Self::Output {
        Self(self.0 * rhs)
    }
}

impl Div<f64> for Duration {
    type Output = Self;

    fn div(self, rhs: f64) -> Self::Output {
        Self(self.0 / rhs)
    }
}

/// Half-open media range `[start, end)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeRange {
    /// Inclusive start.
    pub start: Time,
    /// Exclusive end.
    pub end: Time,
}

impl TimeRange {
    /// Build a range from start and end instants.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTiming`] when `end` is before `start` or either value is non-finite.
    pub fn new(start: Time, end: Time) -> Result<Self> {
        if !start.as_secs().is_finite() || !end.as_secs().is_finite() {
            return Err(CoreError::invalid_timing("range bounds must be finite"));
        }
        if end.as_secs() < start.as_secs() {
            return Err(CoreError::invalid_timing(format!(
                "range end {end} is before start {start}"
            )));
        }
        Ok(Self { start, end })
    }

    /// Range spanning `[Time::ZERO, duration)`.
    ///
    /// # Errors
    ///
    /// Returns [`CoreError::InvalidTiming`] when `duration` is negative or non-finite.
    pub fn from_duration(duration: Duration) -> Result<Self> {
        Self::new(Time::ZERO, Time::from_secs(duration.as_secs()))
    }

    /// Length of the range.
    #[must_use]
    pub fn duration(self) -> Duration {
        self.end - self.start
    }

    /// Whether `t` lies in `[start, end)`.
    #[must_use]
    pub fn contains(self, t: Time) -> bool {
        t.as_secs() >= self.start.as_secs() && t.as_secs() < self.end.as_secs()
    }

    /// Map a time relative to this range into parent timeline coordinates:
    /// `start + local`.
    #[must_use]
    pub fn to_absolute(self, local: Time) -> Time {
        self.start + local.as_duration()
    }

    /// Map an absolute time into coordinates relative to `start`.
    #[must_use]
    pub fn to_local(self, absolute: Time) -> Time {
        Time::from_secs(absolute.as_secs() - self.start.as_secs())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_range_contains_half_open() {
        let range = TimeRange::new(Time::from_secs(1.0), Time::from_secs(3.0)).unwrap();
        assert!(range.contains(Time::from_secs(1.0)));
        assert!(range.contains(Time::from_secs(2.9)));
        assert!(!range.contains(Time::from_secs(3.0)));
        assert!(!range.contains(Time::from_secs(0.5)));
    }

    #[test]
    fn duration_rejects_negative() {
        assert!(Duration::try_from_secs(-0.1).is_err());
        assert!(Duration::try_from_secs(f64::NAN).is_err());
    }

    #[test]
    fn arithmetic() {
        let t = Time::from_secs(2.0) + Duration::from_secs(0.5);
        assert!((t.as_secs() - 2.5).abs() < f64::EPSILON);
        let d = Time::from_secs(5.0) - Time::from_secs(1.5);
        assert!((d.as_secs() - 3.5).abs() < f64::EPSILON);
    }
}

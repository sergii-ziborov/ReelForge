//! Exact half-open media interval (`MediaTime` start + length).

use crate::error::{CoreError, Result};
use crate::media_time::MediaTime;
use crate::time::{Duration, Time, TimeRange};

/// Editorial interval `[start, start + duration)` with exact ticks.
///
/// `duration` is a **length** (`ticks / timescale`), not an end instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MediaRange {
    /// Inclusive start.
    pub start: MediaTime,
    /// Positive length (same or independent timescale).
    pub duration: MediaTime,
}

impl MediaRange {
    /// Build a range when `duration` is strictly positive.
    ///
    /// # Errors
    ///
    /// Zero / negative duration ticks.
    pub fn new(start: MediaTime, duration: MediaTime) -> Result<Self> {
        if duration.ticks <= 0 {
            return Err(CoreError::invalid_timing(
                "MediaRange duration ticks must be > 0",
            ));
        }
        Ok(Self { start, duration })
    }

    /// Start as floating [`Time`] (I/O / effect boundary).
    #[must_use]
    pub fn start_time(self) -> Time {
        self.start.to_time()
    }

    /// Length as floating [`Duration`] (I/O / effect boundary).
    #[must_use]
    pub fn as_duration(self) -> Duration {
        self.duration.to_duration()
    }

    /// Exclusive end as floating [`Time`].
    #[must_use]
    pub fn end_time(self) -> Time {
        self.start_time() + self.as_duration()
    }

    /// Floating [`TimeRange`] for NLE surfaces.
    ///
    /// # Errors
    ///
    /// Non-finite converted bounds (should not happen for valid tick math).
    pub fn to_time_range(self) -> Result<TimeRange> {
        TimeRange::new(self.start_time(), self.end_time())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_positive_duration() {
        let start = MediaTime::zero(1_000);
        let dur = MediaTime::new(0, 1_000).unwrap();
        assert!(MediaRange::new(start, dur).is_err());
    }

    #[test]
    fn one_second_range() {
        let start = MediaTime::new(0, 1_000_000).unwrap();
        let dur = MediaTime::new(1_000_000, 1_000_000).unwrap();
        let r = MediaRange::new(start, dur).unwrap();
        assert!((r.as_duration().as_secs() - 1.0).abs() < 1e-12);
        assert!((r.end_time().as_secs() - 1.0).abs() < 1e-12);
    }
}

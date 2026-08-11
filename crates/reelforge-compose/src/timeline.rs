//! Shared timeline helpers for sequential media.

use reelforge_core::{CoreError, Duration, Time};

/// Map composite time `t` into `(segment_index, local_time)` using exclusive end times.
///
/// # Errors
///
/// Returns [`CoreError::TimeOutOfRange`] when `t` is outside `[0, duration)`.
pub fn map_concat_time(
    ends: &[Duration],
    duration: Duration,
    t: Time,
) -> reelforge_core::Result<(usize, Time)> {
    if t.as_secs() < 0.0 || t.as_secs() >= duration.as_secs() {
        return Err(CoreError::TimeOutOfRange {
            time: t,
            range: (Time::ZERO, Time::from_secs(duration.as_secs())),
        });
    }
    let t_secs = t.as_secs();
    let mut start = 0.0;
    for (i, end) in ends.iter().enumerate() {
        let end_secs = end.as_secs();
        if t_secs < end_secs {
            return Ok((i, Time::from_secs(t_secs - start)));
        }
        start = end_secs;
    }
    Err(CoreError::TimeOutOfRange {
        time: t,
        range: (Time::ZERO, Time::from_secs(duration.as_secs())),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_into_second_segment() {
        let ends = [Duration::from_secs(1.0), Duration::from_secs(2.5)];
        let (i, local) =
            map_concat_time(&ends, Duration::from_secs(2.5), Time::from_secs(1.25)).unwrap();
        assert_eq!(i, 1);
        assert!((local.as_secs() - 0.25).abs() < 1e-9);
    }
}

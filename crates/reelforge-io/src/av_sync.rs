//! Presentation drift between muxed video and audio streams.

/// One video frame plus AAC encoder delay (~23–46 ms).
#[must_use]
pub fn max_av_drift_secs(fps: f64) -> f64 {
    let frame = if fps.is_finite() && fps > 0.0 {
        1.0 / fps
    } else {
        1.0 / 24.0
    };
    frame + 0.05
}

/// Absolute difference between video and audio durations (seconds).
#[must_use]
pub fn av_duration_drift(video_secs: f64, audio_secs: f64) -> f64 {
    (video_secs - audio_secs).abs()
}

/// Whether muxed stream durations stay within [`max_av_drift_secs`].
#[must_use]
pub fn av_streams_aligned(video_secs: f64, audio_secs: f64, fps: f64) -> bool {
    if !video_secs.is_finite() || !audio_secs.is_finite() {
        return false;
    }
    av_duration_drift(video_secs, audio_secs) <= max_av_drift_secs(fps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_durations_align() {
        assert!(av_streams_aligned(1.0, 1.0, 24.0));
        assert!((av_duration_drift(1.0, 1.02) - 0.02).abs() < 1e-12);
    }

    #[test]
    fn one_frame_plus_aac_slack_is_ok() {
        // 10 fps → 0.10s frame + 0.05s AAC = 0.15s budget.
        assert!(av_streams_aligned(0.50, 0.62, 10.0));
        assert!(!av_streams_aligned(0.50, 0.70, 10.0));
    }

    #[test]
    fn rejects_non_finite() {
        assert!(!av_streams_aligned(f64::NAN, 1.0, 24.0));
        assert!(!av_streams_aligned(1.0, f64::INFINITY, 24.0));
    }
}
